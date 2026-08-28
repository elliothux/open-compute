import { RpcTarget, WorkerEntrypoint } from "cloudflare:workers";
import { bindingError, currentStartupGeneration } from "./loader-host.js";

async function request(env, props, operation, body) {
  if (!props || typeof props.bindingId !== "string" || typeof props.deploymentId !== "string"
      || !/^[0-9a-f]{64}$/.test(props.descriptorSha256) || typeof props.durableObject !== "boolean") {
    throw bindingError("WORKFLOW_BINDING_STALE");
  }
  if (!["get", "status"].includes(operation) && props.durableObject) {
    throw bindingError("WORKFLOW_DO_OUTPUT_GATE_UNSUPPORTED");
  }
  let response;
  try {
    response = await env.BINDING_BACKEND.fetch(
      `http://binding-backend/internal/bindings/v1/workflow/${props.bindingId}/${operation}`,
      {
        method: "POST",
        headers: {
          "content-type": "application/json",
          "x-open-compute-binding-token": env.BINDING_BACKEND_TOKEN,
          "x-open-compute-startup-generation": currentStartupGeneration(),
          "x-open-compute-deployment-id": props.deploymentId,
          "x-open-compute-descriptor-sha256": props.descriptorSha256,
          "x-open-compute-workflow-do-context": props.durableObject ? "1" : "0",
        },
        body: JSON.stringify(body),
      },
    );
  } catch { throw bindingError("WORKFLOW_RUNTIME_UNAVAILABLE"); }
  if (!response.ok) {
    const code = response.headers.get("x-open-compute-error-code") || "WORKFLOW_RUNTIME_UNAVAILABLE";
    try { await response.body?.cancel(); } catch { /* already closed */ }
    throw bindingError(code);
  }
  try { return await response.json(); }
  catch { throw bindingError("WORKFLOW_RUNTIME_UNAVAILABLE"); }
}

class WorkflowInstanceTransportV2 extends RpcTarget {
  #env;
  #props;
  #instanceId;
  constructor(env, props, instanceId) {
    super();
    this.#env = env;
    this.#props = props;
    this.#instanceId = instanceId;
  }
  #request(operation, body = {}) {
    // The backend admits the current execution generation on each method. A
    // restart preserves this UUID; expiry and external-ID reuse never redirect it.
    return request(this.#env, this.#props, operation, { ...body, instanceId: this.#instanceId });
  }
  status() { return this.#request("status"); }
  pause() { return this.#request("pause"); }
  resume() { return this.#request("resume"); }
  terminate() { return this.#request("terminate"); }
  restart() { return this.#request("restart", { operationId: crypto.randomUUID() }); }
  sendEvent(body) { return this.#request("send-event", body); }
}

export class WorkflowBindingTransportV2 extends WorkerEntrypoint {
  async #resolve(operation, body) {
    const props = this.ctx.props;
    const result = await request(this.env, props, operation, body);
    if (typeof result?.instanceId !== "string"
        || !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(result.instanceId)
        || typeof result.id !== "string") {
      throw bindingError("WORKFLOW_RUNTIME_UNAVAILABLE");
    }
    return { id: result.id, handle: new WorkflowInstanceTransportV2(this.env, props, result.instanceId) };
  }
  create(body) { return this.#resolve("create", body); }
  get(id) { return this.#resolve("get", { id }); }
}
