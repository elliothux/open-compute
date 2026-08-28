import { RpcTarget, WorkerEntrypoint } from "cloudflare:workers";
import { PROFILE, bindingError, currentStartupGeneration, doPolicy, modulesFor, resolveSnapshot, stableCode, tenantEnv } from "../loader/host.js";
import { WorkflowRunControllerV2, finishWorkflowRun, closeWorkflowRun } from "./controller-v2.js";
import { readWorkflowStatus } from "./binding-v2.js";
import type { BindingEnv } from "../bindings/protocol.js";
import type { WorkflowBindingProps } from "./binding-protocol.js";
import type { LoaderEnv } from "../loader/protocol.js";
import type { LoadedWorkflow, WorkflowClaimReplyV1, WorkflowClaimV1, WorkflowCommitReplyV1, WorkflowControllerV1, WorkflowEventWire, WorkflowFailureV1, WorkflowRunIdentity } from "./execution-protocol.js";

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function assertActivation(value: Record<string, unknown>): asserts value is Record<string, unknown> & WorkflowRunIdentity & WorkflowEventWire {
  for (const name of ["instanceId", "runToken", "externalInstanceId", "definitionName", "payloadJson"]) {
    if (typeof value[name] !== "string") throw new Error("invalid activation");
  }
  if (typeof value.instanceGeneration !== "number" || !Number.isSafeInteger(value.instanceGeneration)
      || typeof value.createdAtMs !== "number" || !Number.isSafeInteger(value.createdAtMs)) throw new Error("invalid activation");
}

export class WorkflowBindingTransport extends WorkerEntrypoint<BindingEnv, WorkflowBindingProps> {
  async #request(operation: string, body: object): Promise<Record<string, unknown>> {
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
    const result: unknown = await response.json();
    if (!record(result)) throw bindingError("WORKFLOW_RUNTIME_UNAVAILABLE");
    return result;
  }
  async create(body: { id: string | undefined; payloadJson: string }) {
    const result = await this.#request("create", body);
    if (typeof result.id !== "string") throw bindingError("WORKFLOW_RUNTIME_UNAVAILABLE");
    return { id: result.id };
  }
  get(id: string) { return this.#request("get", { id }); }
  async status(id: string) { return readWorkflowStatus(await this.#request("status", { id })); }
}

// Raw grants never cross into the tenant realm: even a closure-local async return
// can be observed there through modified Promise intrinsics. SQLite still owns
// the grant; this request-scoped object only retains the current step capability.
class WorkflowRunController extends RpcTarget implements WorkflowControllerV1 {
  #env: BindingEnv;
  #identity: WorkflowRunIdentity;
  #grant: { ordinal: number; stepToken: string } | null = null;
  #closed = false;

  constructor(env: BindingEnv, identity: WorkflowRunIdentity) {
    super();
    this.#env = env;
    this.#identity = identity;
  }

  [Symbol.dispose]() {
    this.#closed = true;
    this.#grant = null;
  }

  async #request(operation: string, body: object): Promise<Record<string, unknown>> {
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
    const reply: unknown = await response.json();
    if (this.#closed) throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
    if (!record(reply)) throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
    return reply;
  }

  async claim(body: WorkflowClaimV1): Promise<WorkflowClaimReplyV1> {
    const reply = await this.#request("claim", body);
    if (typeof reply.errorCode === "string" && reply.errorCode) return { errorCode: reply.errorCode };
    if (reply?.state === "complete" && typeof reply.outputJson === "string") {
      return { state: "complete", outputJson: reply.outputJson };
    }
    if (reply?.state === "failed") {
      const failure = reply.error;
      if (failure === undefined) return { state: "failed" };
      if (!record(failure) || typeof failure.name !== "string" || typeof failure.message !== "string") {
        throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
      }
      return { state: "failed", error: { name: failure.name, message: failure.message } };
    }
    if (reply?.state !== "run" || typeof reply.stepToken !== "string"
        || !/^[0-9a-f]{64}$/.test(reply.stepToken)) {
      throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
    }
    this.#grant = { ordinal: body.ordinal, stepToken: reply.stepToken };
    return { state: "run" };
  }

  async #commit(operation: string, body: { ordinal: number } & object): Promise<WorkflowCommitReplyV1> {
    const grant = this.#grant;
    if (!grant || grant.ordinal !== body.ordinal) return { errorCode: "WORKFLOW_STEP_STALE" };
    const reply = await this.#request(operation, { ...body, stepToken: grant.stepToken });
    if (typeof reply.errorCode === "string" && reply.errorCode) return { errorCode: reply.errorCode };
    if (reply?.ok !== true) throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
    this.#grant = null;
    return { ok: true };
  }
  success(body: { ordinal: number; outputJson: string }) { return this.#commit("success", body); }
  failure(body: WorkflowFailureV1) { return this.#commit("failure", body); }
}

export async function handleWorkflow(request: Request, env: LoaderEnv, ctx: ExecutionContext, validation: boolean, capability = 1) {
  try {
    // Consume the private request before entering RPC, including validation's
    // null body. An unread body can retain the reused HTTP/1 connection.
    const input: unknown = await request.json();
    const body = record(input) ? input : {};
    const loaderKey = request.headers.get("x-open-compute-loader-key") || "";
    const expected = request.headers.get("x-open-compute-worker-code-sha256") || "";
    const className = request.headers.get("x-open-compute-entrypoint") || "";
    if (!/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(className)
        || !/^[0-9a-f]{64}$/.test(expected)
        || loaderKey.split("/").length !== 3) throw new Error("invalid envelope");
    const snapshot = await resolveSnapshot(env, { loaderKey, expected }, validation, true,
      request.headers.get("x-open-compute-internal-token"));
    if (capability === 2 && (typeof body.versionDescriptorSha256 !== "string" || !/^[0-9a-f]{64}$/.test(body.versionDescriptorSha256))) {
      throw new Error("invalid version descriptor");
    }
    const built = modulesFor(snapshot, false, className, false, true, capability);
    const deploymentId = loaderKey.split("/")[2]!;
    let cold = false;
    const key = capability === 1 ? `workflow/${validation}/${loaderKey}/${expected}/${className}`
      : `workflow-v2/${validation}/${loaderKey}/${expected}/${className}/${body.versionDescriptorSha256}`;
    const loaded = env.LOADER.get(key, () => {
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
    const target = loaded.getEntrypoint<LoadedWorkflow>("__OpenComputeWorkflow");
    if (!await target.validate()) return new Response(null, { status: 422 });
    if (validation) return Response.json({ valid: true });
    assertActivation(body);
    const identity = {
      instanceId: body.instanceId, instanceGeneration: body.instanceGeneration, runToken: body.runToken,
    };
    let backend: WorkflowRunControllerV2 | WorkflowRunController;
    if (capability === 2) {
      if (typeof body.activationBudgetMs !== "number") throw new Error("invalid activation budget");
      backend = new WorkflowRunControllerV2(env, identity, body.activationBudgetMs);
    } else {
      backend = new WorkflowRunController(env, identity);
    }
    try {
      const result = await target.execute({
        externalInstanceId: body.externalInstanceId, definitionName: body.definitionName,
        createdAtMs: body.createdAtMs, payloadJson: body.payloadJson,
      }, backend);
      if (backend instanceof WorkflowRunControllerV2) {
        return Response.json({ ...await backend[finishWorkflowRun](result),
          loaderOutcome: cold ? "cold" : "warm" });
      }
      return Response.json({ ...result, loaderOutcome: cold ? "cold" : "warm" });
    } finally {
      if (backend instanceof WorkflowRunControllerV2) backend[closeWorkflowRun]();
      else backend[Symbol.dispose]();
    }
  } catch (error) {
    const code = stableCode(error);
    if (code === "ARTIFACT_INTEGRITY_ERROR" || code === "DEPLOYMENT_INVARIANT_VIOLATION") {
      return new Response(null, { status: 422, headers: { "x-open-compute-error-code":
        code === "ARTIFACT_INTEGRITY_ERROR" ? code : "WORKFLOW_INVARIANT_VIOLATION" } });
    }
    return new Response(null, { status: 503 });
  }
}
