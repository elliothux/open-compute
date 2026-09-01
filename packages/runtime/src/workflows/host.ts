import {
  doPolicy, lockWorkerCode, modulesFor, resolveSnapshot, stableCode,
  tenantEnv, tenantGlobalOutbound,
} from "../loader/host.js";
import { WorkflowRunController, finishWorkflowRun, closeWorkflowRun } from "./controller.js";
import type { LoaderEnv } from "../loader/protocol.js";
import type { LoadedWorkflow, WorkflowEventWire, WorkflowRunIdentity } from "./execution-protocol.js";

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function assertActivation(value: Record<string, unknown>): asserts value is Record<string, unknown> & WorkflowRunIdentity & WorkflowEventWire {
  for (const name of ["instanceId", "runToken", "externalInstanceId", "definitionName", "payloadBase64"]) {
    if (typeof value[name] !== "string") throw new Error("invalid activation");
  }
  if (typeof value.instanceGeneration !== "number" || !Number.isSafeInteger(value.instanceGeneration)
      || typeof value.createdAtMs !== "number" || !Number.isSafeInteger(value.createdAtMs)
      || typeof value.rollback !== "boolean") throw new Error("invalid activation");
  if (value.schedule !== undefined) {
    if (!record(value.schedule) || typeof value.schedule.cron !== "string"
        || value.schedule.cron.length < 1 || value.schedule.cron.length > 256
        || typeof value.schedule.scheduledTime !== "number"
        || !Number.isSafeInteger(value.schedule.scheduledTime) || value.schedule.scheduledTime < 0) {
      throw new Error("invalid activation");
    }
  }
}

export async function handleWorkflow(request: Request, env: LoaderEnv, ctx: ExecutionContext, validation: boolean) {
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
        || loaderKey.split("/").length !== 3
        || typeof body.versionDescriptorSha256 !== "string"
        || !/^[0-9a-f]{64}$/.test(body.versionDescriptorSha256)) {
      throw new Error("invalid workflow envelope");
    }
    const snapshot = await resolveSnapshot(env, { loaderKey, expected }, validation, true,
      request.headers.get("x-open-compute-internal-token"));
    const built = modulesFor(snapshot, false, className, false, true);
    const deploymentId = loaderKey.split("/")[2]!;
    let cold = false;
    const key = `workflow/${validation}/${loaderKey}/${expected}/${className}/${body.versionDescriptorSha256}`;
    const loaded = env.LOADER.get(key, () => {
      cold = true;
      return {
        ...lockWorkerCode(env),
        mainModule: built.mainModule, modules: built.modules,
        env: validation ? {} : tenantEnv(snapshot, ctx, deploymentId, doPolicy(env), false, false),
        globalOutbound: tenantGlobalOutbound(env, validation),
      };
    });
    const target = loaded.getEntrypoint<LoadedWorkflow>("__OpenComputeWorkflow");
    if (!await target.validate()) return new Response(null, { status: 422 });
    if (validation) return Response.json({ valid: true });
    assertActivation(body);
    if (typeof body.activationBudgetMs !== "number") throw new Error("invalid activation budget");
    const backend = new WorkflowRunController(env, {
      instanceId: body.instanceId, instanceGeneration: body.instanceGeneration, runToken: body.runToken,
    }, body.activationBudgetMs);
    try {
      const result = await target.execute({
        externalInstanceId: body.externalInstanceId, definitionName: body.definitionName,
        createdAtMs: body.createdAtMs, payloadBase64: body.payloadBase64, rollback: body.rollback,
        ...(body.schedule === undefined ? {} : { schedule: body.schedule }),
      }, backend);
      return Response.json({ ...await backend[finishWorkflowRun](result),
        loaderOutcome: cold ? "cold" : "warm" });
    } finally {
      backend[closeWorkflowRun]();
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
