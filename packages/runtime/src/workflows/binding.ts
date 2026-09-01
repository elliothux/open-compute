import { RpcTarget, WorkerEntrypoint } from "cloudflare:workers";
import { bindingError, currentStartupGeneration } from "../loader/host.js";
import type { BindingEnv } from "../bindings/protocol.js";
import type { WorkflowBindingProps, WorkflowCreateWire, WorkflowHandle, WorkflowStatus } from "./binding-protocol.js";
import { decodeWorkflowBase64 } from "./codec.js";

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function readWorkflowStatus(value: unknown): WorkflowStatus {
  if (!record(value) || typeof value.status !== "string") {
    throw bindingError("WORKFLOW_RUNTIME_UNAVAILABLE");
  }
  const result: WorkflowStatus = { status: value.status };
  if (value.outputBase64 !== undefined) result.output = decodeWorkflowBase64(value.outputBase64);
  const error = value.error;
  if (error !== undefined) {
    if (!record(error) || typeof error.name !== "string" || typeof error.message !== "string") {
      throw bindingError("WORKFLOW_RUNTIME_UNAVAILABLE");
    }
    result.error = { name: error.name, message: error.message };
  }
  return result;
}

async function request(
  env: BindingEnv,
  props: WorkflowBindingProps,
  operation: string,
  body: object,
  operationId?: string,
): Promise<Record<string, unknown>> {
  if (!props || typeof props.bindingId !== "string" || typeof props.deploymentId !== "string"
      || !/^[0-9a-f]{64}$/.test(props.descriptorSha256) || typeof props.durableObject !== "boolean") {
    throw bindingError("WORKFLOW_BINDING_STALE");
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
          "x-open-compute-request-id": operationId ?? crypto.randomUUID(),
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
  try {
    const result: unknown = await response.json();
    if (!record(result)) throw bindingError("WORKFLOW_RUNTIME_UNAVAILABLE");
    return result;
  }
  catch { throw bindingError("WORKFLOW_RUNTIME_UNAVAILABLE"); }
}

class WorkflowInstanceTransport extends RpcTarget implements WorkflowHandle {
  #env: BindingEnv;
  #props: WorkflowBindingProps;
  #instanceId: string;
  constructor(env: BindingEnv, props: WorkflowBindingProps, instanceId: string) {
    super();
    this.#env = env;
    this.#props = props;
    this.#instanceId = instanceId;
  }
  #request(operation: string, body: object = {}, operationId?: string) {
    // The backend admits the current execution generation on each method. A
    // restart preserves this UUID; expiry and external-ID reuse never redirect it.
    return request(this.#env, this.#props, operation, { ...body, instanceId: this.#instanceId }, operationId);
  }
  async status() { return readWorkflowStatus(await this.#request("status")); }
  pause(operationId: string) { return this.#request("pause", {}, operationId); }
  resume(operationId: string) { return this.#request("resume", {}, operationId); }
  terminate(options: { rollback?: boolean }, operationId: string) {
    return this.#request("terminate", options, operationId);
  }
  restart(options: { from?: { name: string; count?: number; type?: "do" | "sleep" | "waitForEvent" } }, operationId: string) {
    return this.#request("restart", options, operationId);
  }
  delete(operationId: string) { return this.#request("delete", {}, operationId); }
  sendEvent(body: { type: string; payloadBase64: string }, operationId: string) {
    return this.#request("send-event", body, operationId);
  }
}

class WorkflowPendingInstanceTransport extends RpcTarget implements WorkflowHandle {
  #env: BindingEnv;
  #props: WorkflowBindingProps;
  #externalId: string;
  #resolved?: Promise<WorkflowInstanceTransport>;
  constructor(env: BindingEnv, props: WorkflowBindingProps, externalId: string) {
    super();
    this.#env = env;
    this.#props = props;
    this.#externalId = externalId;
  }
  async #handle() {
    this.#resolved ??= request(this.#env, this.#props, "get", { id: this.#externalId }).then(result => {
      if (typeof result.instanceId !== "string") throw bindingError("WORKFLOW_RUNTIME_UNAVAILABLE");
      return new WorkflowInstanceTransport(this.#env, this.#props, result.instanceId);
    });
    return this.#resolved;
  }
  async status() { return (await this.#handle()).status(); }
  async pause(id: string) { return (await this.#handle()).pause(id); }
  async resume(id: string) { return (await this.#handle()).resume(id); }
  async terminate(options: { rollback?: boolean }, id: string) { return (await this.#handle()).terminate(options, id); }
  async restart(options: { from?: { name: string; count?: number; type?: "do" | "sleep" | "waitForEvent" } }, id: string) {
    return (await this.#handle()).restart(options, id);
  }
  async delete(id: string) { return (await this.#handle()).delete(id); }
  async sendEvent(body: { type: string; payloadBase64: string }, id: string) {
    return (await this.#handle()).sendEvent(body, id);
  }
}

export class WorkflowBindingTransport extends WorkerEntrypoint<BindingEnv, WorkflowBindingProps> {
  #resolved(result: Record<string, unknown>) {
    if (typeof result.instanceId !== "string"
        || !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(result.instanceId)
        || typeof result.id !== "string") {
      throw bindingError("WORKFLOW_RUNTIME_UNAVAILABLE");
    }
    return { id: result.id, handle: new WorkflowInstanceTransport(this.env, this.ctx.props, result.instanceId) };
  }
  async #resolve(operation: string, body: object, operationId?: string) {
    const props = this.ctx.props;
    return this.#resolved(await request(this.env, props, operation, body, operationId));
  }
  resolve(id: string) {
    return { id, handle: new WorkflowPendingInstanceTransport(this.env, this.ctx.props, id) };
  }
  create(body: WorkflowCreateWire, operationId: string) { return this.#resolve("create", body, operationId); }
  get(id: string) { return this.#resolve("get", { id }); }
  async createBatch(body: WorkflowCreateWire[], operationId: string) {
    const result = await request(this.env, this.ctx.props, "create-batch", { instances: body }, operationId);
    if (!Array.isArray(result.instances) || result.instances.length !== body.length) {
      throw bindingError("WORKFLOW_RUNTIME_UNAVAILABLE");
    }
    return result.instances.map(value => {
      if (!record(value)) throw bindingError("WORKFLOW_RUNTIME_UNAVAILABLE");
      return this.#resolved(value);
    });
  }
  async deleteBatch(instanceIds: string[], operationId: string) {
    const result = await request(this.env, this.ctx.props, "delete-batch", { instanceIds }, operationId);
    if (!Array.isArray(result.deleted) || !Array.isArray(result.errors)) {
      throw bindingError("WORKFLOW_RUNTIME_UNAVAILABLE");
    }
    return result as { deleted: { id: string }[]; errors: { id: string; code: number; message: string }[] };
  }
}
