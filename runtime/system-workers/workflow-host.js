import { RpcTarget, WorkerEntrypoint } from "cloudflare:workers";
import { PROFILE, bindingError, currentStartupGeneration, doPolicy, modulesFor, resolveSnapshot, tenantEnv } from "./loader-host.js";

export class WorkflowBindingTransport extends WorkerEntrypoint {
  async #request(operation, body) {
    const props = this.ctx.props;
    if (!props || typeof props.bindingId !== "string" || typeof props.deploymentId !== "string"
        || !/^[0-9a-f]{64}$/.test(props.descriptorSha256) || typeof props.durableObject !== "boolean") {
      throw bindingError("WORKFLOW_BINDING_STALE");
    }
    if (operation === "create" && props.durableObject) throw bindingError("WORKFLOW_DO_OUTPUT_GATE_UNSUPPORTED");
    const response = await this.env.BINDING_BACKEND.fetch(
      `http://binding-backend/internal/bindings/v1/workflow/${props.bindingId}/${operation}`,
      {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "x-open-compute-binding-token": this.env.BINDING_BACKEND_TOKEN,
          "x-open-compute-startup-generation": currentStartupGeneration(),
          "x-open-compute-deployment-id": props.deploymentId,
          "x-open-compute-descriptor-sha256": props.descriptorSha256,
          "x-open-compute-workflow-do-context": props.durableObject ? "1" : "0",
        },
        body: JSON.stringify(body),
      },
    );
    if (!response.ok) {
      const code = response.headers.get("x-open-compute-error-code") || "WORKFLOW_RUNTIME_UNAVAILABLE";
      try { await response.body?.cancel(); } catch { /* already closed */ }
      throw bindingError(code);
    }
    return response.json();
  }
  create(body) { return this.#request("create", body); }
  get(id) { return this.#request("get", { id }); }
  status(id) { return this.#request("status", { id }); }
}

// Raw grants never cross into the tenant realm: even a closure-local async return
// can be observed there through modified Promise intrinsics. SQLite still owns
// the grant; this request-scoped object only retains the current step capability.
class WorkflowRunController extends RpcTarget {
  #env;
  #identity;
  #grant = null;
  #closed = false;

  constructor(env, identity) {
    super();
    this.#env = env;
    this.#identity = identity;
  }

  [Symbol.dispose]() {
    this.#closed = true;
    this.#grant = null;
  }

  async #request(operation, body) {
    if (this.#closed) throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
    const response = await this.#env.BINDING_BACKEND.fetch(
      `http://binding-backend/internal/workflows/v1/runs/${operation}`,
      {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "x-open-compute-binding-token": this.#env.BINDING_BACKEND_TOKEN,
          "x-open-compute-startup-generation": currentStartupGeneration(),
        },
        // Authority always follows tenant-independent method fields.
        body: JSON.stringify({ ...body, ...this.#identity }),
      },
    );
    if (!response.ok) {
      // Server errors/transport loss are Unknown: do not turn a possible commit
      // into a terminal tenant error or an immediate duplicate callback.
      const status = response.status;
      const code = response.headers.get("x-open-compute-error-code") || "WORKFLOW_RUN_STALE";
      try { await response.body?.cancel(); } catch { /* already closed */ }
      if (status >= 500) throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
      return { errorCode: code };
    }
    const reply = await response.json();
    if (this.#closed) throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
    return reply;
  }

  async claim(body) {
    const reply = await this.#request("claim", body);
    if (reply?.errorCode) return { errorCode: reply.errorCode };
    if (reply?.state === "complete" && typeof reply.outputJson === "string") {
      return { state: "complete", outputJson: reply.outputJson };
    }
    if (reply?.state === "failed") {
      return { state: "failed", errorCode: reply.errorCode, error: reply.error };
    }
    if (reply?.state !== "run" || typeof reply.stepToken !== "string"
        || !/^[0-9a-f]{64}$/.test(reply.stepToken)) {
      throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
    }
    this.#grant = { ordinal: body.ordinal, stepToken: reply.stepToken };
    return { state: "run" };
  }

  async #commit(operation, body) {
    const grant = this.#grant;
    if (!grant || grant.ordinal !== body.ordinal) return { errorCode: "WORKFLOW_STEP_STALE" };
    const reply = await this.#request(operation, { ...body, stepToken: grant.stepToken });
    if (reply?.errorCode) return { errorCode: reply.errorCode };
    if (reply?.ok !== true) throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
    this.#grant = null;
    return { ok: true };
  }
  success(body) { return this.#commit("success", body); }
  failure(body) { return this.#commit("failure", body); }
}

export async function handleWorkflow(request, env, ctx, validation) {
  try {
    // Consume the private request before entering RPC, including validation's
    // null body. An unread body can retain the reused HTTP/1 connection.
    const body = await request.json();
    const loaderKey = request.headers.get("x-open-compute-loader-key") || "";
    const expected = request.headers.get("x-open-compute-worker-code-sha256") || "";
    const className = request.headers.get("x-open-compute-entrypoint") || "";
    if (!/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(className)
        || !/^[0-9a-f]{64}$/.test(expected)
        || loaderKey.split("/").length !== 3) throw new Error("invalid envelope");
    const snapshot = await resolveSnapshot(env, { loaderKey, expected }, validation, true,
      request.headers.get("x-open-compute-internal-token"));
    const built = modulesFor(snapshot, false, className, false, true);
    const deploymentId = loaderKey.split("/")[2];
    let cold = false;
    const loaded = env.LOADER.get(`workflow/${validation}/${loaderKey}/${expected}/${className}`, () => {
      cold = true;
      return {
        compatibilityDate: snapshot.compatibilityDate,
        compatibilityFlags: snapshot.compatibilityFlags,
        mainModule: built.mainModule, modules: built.modules,
        env: validation ? {} : tenantEnv(snapshot, ctx, deploymentId, doPolicy(env)),
        globalOutbound: validation ? null : ctx.exports.OutboundGateway({ props: { deploymentId, policyVersion: 1 } }),
        limits: PROFILE,
      };
    });
    const target = loaded.getEntrypoint("__OpenComputeWorkflow");
    if (!await target.validate()) return new Response(null, { status: 422 });
    if (validation) return Response.json({ valid: true });
    const backend = new WorkflowRunController(env, {
      instanceId: body.instanceId, instanceGeneration: body.instanceGeneration, runToken: body.runToken,
    });
    try {
      const result = await target.execute({
        externalInstanceId: body.externalInstanceId, definitionName: body.definitionName,
        createdAtMs: body.createdAtMs, payloadJson: body.payloadJson,
      }, backend);
      return Response.json({ ...result, loaderOutcome: cold ? "cold" : "warm" });
    } finally {
      backend[Symbol.dispose]();
    }
  } catch (error) {
    if (error?.stableCode === "ARTIFACT_INTEGRITY_ERROR"
        || error?.stableCode === "DEPLOYMENT_INVARIANT_VIOLATION") {
      return new Response(null, { status: 422, headers: { "x-open-compute-error-code":
        error.stableCode === "ARTIFACT_INTEGRITY_ERROR" ? error.stableCode : "WORKFLOW_INVARIANT_VIOLATION" } });
    }
    return new Response(null, { status: 503 });
  }
}
