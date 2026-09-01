import type {
  WorkflowCreateWire,
  WorkflowHandle,
  WorkflowResolvedInstance,
  WorkflowTransport,
} from "./binding-protocol.js";
import { currentOutputGate, FLUSH_OUTPUT } from "../durable-objects/output-gate.js";
import {
  decodeWorkflowValue,
  encodeWorkflowBase64,
  encodeWorkflowValue,
  workflowError,
  workflowString,
} from "./codec.js";

const LOCATION_HINTS = new Set([
  "wnam", "enam", "sam", "weur", "eeur", "apac", "apac-ne", "apac-se", "oc", "afr", "me",
]);
const scheduledTriggers = new WeakMap<object, (
  schedule: { cron: string; scheduledTime: number },
) => Promise<void>>();

function instanceId(value: unknown): string {
  if (typeof value !== "string" || value.length > 100 || !/^[a-zA-Z0-9_][a-zA-Z0-9_-]*$/.test(value)) {
    throw workflowError("WORKFLOW_INSTANCE_ID_INVALID");
  }
  return value;
}

function fields(value: unknown, allowed: readonly string[]): asserts value is Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)
      || Object.keys(value).some(key => !allowed.includes(key))) {
    throw workflowError("WORKFLOW_METHOD_UNSUPPORTED");
  }
}

function noArguments(values: readonly unknown[]): void {
  if (values.length !== 0) throw workflowError("WORKFLOW_METHOD_UNSUPPORTED");
}

function operationId(): string { return crypto.randomUUID(); }

function createWire(value: unknown): WorkflowCreateWire {
  fields(value, ["id", "params", "retention", "locationHint"]);
  const id = value.id === undefined ? crypto.randomUUID() : instanceId(value.id);
  if (value.retention !== undefined) fields(value.retention, ["successRetention", "errorRetention"]);
  if (value.locationHint !== undefined
      && (typeof value.locationHint !== "string" || !LOCATION_HINTS.has(value.locationHint))) {
    throw workflowError("WORKFLOW_METHOD_UNSUPPORTED");
  }
  return {
    ...(id === undefined ? {} : { id }),
    payloadBase64: encodeWorkflowBase64(value.params, "WORKFLOW_PAYLOAD_TOO_LARGE"),
    ...(value.retention === undefined ? {} : { retention: value.retention }),
    ...(value.locationHint === undefined ? {} : { locationHint: value.locationHint }),
  };
}

function terminateOptions(value: unknown): { rollback?: boolean } {
  fields(value, ["rollback"]);
  if (value.rollback !== undefined && typeof value.rollback !== "boolean") {
    throw workflowError("WORKFLOW_METHOD_UNSUPPORTED");
  }
  return value.rollback === undefined ? {} : { rollback: value.rollback };
}

function restartOptions(value: unknown): { from?: { name: string; count?: number; type?: "do" | "sleep" | "waitForEvent" } } {
  fields(value, ["from"]);
  if (value.from === undefined) return {};
  fields(value.from, ["name", "count", "type"]);
  const name = workflowString(value.from.name, 256, "WORKFLOW_METHOD_UNSUPPORTED");
  if (name.length === 0) throw workflowError("WORKFLOW_METHOD_UNSUPPORTED");
  if (value.from.count !== undefined
      && (typeof value.from.count !== "number" || !Number.isSafeInteger(value.from.count)
        || value.from.count < 1 || value.from.count > 1024)) {
    throw workflowError("WORKFLOW_METHOD_UNSUPPORTED");
  }
  if (value.from.type !== undefined && !["do", "sleep", "waitForEvent"].includes(String(value.from.type))) {
    throw workflowError("WORKFLOW_METHOD_UNSUPPORTED");
  }
  return { from: {
    name,
    ...(value.from.count === undefined ? {} : { count: value.from.count }),
    ...(value.from.type === undefined ? {} : { type: value.from.type as "do" | "sleep" | "waitForEvent" }),
  } };
}

type WorkflowIntent =
  | { op: "create"; create: WorkflowCreateWire }
  | { op: "createBatch"; creates: WorkflowCreateWire[] }
  | { op: "deleteBatch"; instanceIds: string[] }
  | { op: "pause" | "resume" | "delete"; instanceId: string }
  | { op: "terminate"; instanceId: string; options: { rollback?: boolean } }
  | { op: "restart"; instanceId: string; options: ReturnType<typeof restartOptions> }
  | { op: "sendEvent"; instanceId: string; event: { type: string; payloadBase64: string } };

class WorkflowInstance {
  declare readonly id: string;
  #handle: WorkflowHandle;
  #binding: WorkflowBinding;
  constructor(result: WorkflowResolvedInstance, binding: WorkflowBinding) {
    this.#handle = result.handle;
    this.#binding = binding;
    Object.defineProperty(this, "id", { value: instanceId(result.id), enumerable: true });
    Object.freeze(this);
  }
  status(...args: unknown[]) { noArguments(args); return this.#handle.status(); }
  #mutate(intent: WorkflowIntent, run: (id: string) => Promise<unknown> | unknown) {
    return this.#binding.dispatchIntent(intent, run, () => undefined);
  }
  pause(...args: unknown[]) {
    noArguments(args);
    return this.#mutate({ op: "pause", instanceId: this.id }, id => this.#handle.pause(id));
  }
  resume(...args: unknown[]) {
    noArguments(args);
    return this.#mutate({ op: "resume", instanceId: this.id }, id => this.#handle.resume(id));
  }
  terminate(options: unknown = {}) {
    const normalized = terminateOptions(options);
    return this.#mutate(
      { op: "terminate", instanceId: this.id, options: normalized },
      id => this.#handle.terminate(normalized, id),
    );
  }
  restart(options: unknown = {}) {
    const normalized = restartOptions(options);
    return this.#mutate(
      { op: "restart", instanceId: this.id, options: normalized },
      id => this.#handle.restart(normalized, id),
    );
  }
  sendEvent(event: unknown) {
    fields(event, ["type", "payload"]);
    if (typeof event.type !== "string" || event.type.length > 100
        || !/^[a-zA-Z0-9_][a-zA-Z0-9_-]*$/.test(event.type)) {
      throw workflowError("WORKFLOW_EVENT_TYPE_INVALID");
    }
    const body = {
      type: event.type,
      payloadBase64: encodeWorkflowBase64(event.payload, "WORKFLOW_PAYLOAD_TOO_LARGE"),
    };
    return this.#mutate(
      { op: "sendEvent", instanceId: this.id, event: body },
      id => this.#handle.sendEvent(body, id),
    );
  }
  delete(...args: unknown[]) {
    noArguments(args);
    return this.#mutate({ op: "delete", instanceId: this.id }, id => this.#handle.delete(id));
  }
}

export class WorkflowBinding {
  #transport: WorkflowTransport;
  #durableObject;
  #name;
  constructor(transport: unknown, durableObject = false, name = "") {
    if (!rawTransport(transport)) throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
    this.#transport = transport;
    this.#durableObject = durableObject;
    this.#name = typeof name === "string" ? name : "";
    scheduledTriggers.set(this, schedule => this.#triggerSchedule(schedule));
    Object.freeze(this);
  }

  async #triggerSchedule(schedule: { cron: string; scheduledTime: number }) {
    if (!schedule || typeof schedule.cron !== "string" || schedule.cron.length < 1
        || schedule.cron.length > 256 || !Number.isSafeInteger(schedule.scheduledTime)
        || schedule.scheduledTime < 0) {
      throw workflowError("WORKFLOW_METHOD_UNSUPPORTED");
    }
    const digest = new Uint8Array(await crypto.subtle.digest(
      "SHA-256",
      new TextEncoder().encode(`${this.#name}\0${schedule.cron}\0${schedule.scheduledTime}`),
    ));
    const hex = Array.from(digest, byte => byte.toString(16).padStart(2, "0")).join("");
    const id = `schedule-${hex}`;
    const uuid = digest.slice(0, 16);
    uuid[6] = (uuid[6]! & 0x0f) | 0x40;
    uuid[8] = (uuid[8]! & 0x3f) | 0x80;
    const operation = Array.from(uuid, byte => byte.toString(16).padStart(2, "0")).join("");
    const operationId = `${operation.slice(0, 8)}-${operation.slice(8, 12)}-${operation.slice(12, 16)}-${operation.slice(16, 20)}-${operation.slice(20)}`;
    await this.#transport.create({
      id,
      payloadBase64: encodeWorkflowBase64(undefined, "WORKFLOW_PAYLOAD_TOO_LARGE"),
      schedule,
    }, operationId);
  }

  dispatchIntent(intent: WorkflowIntent, run: (id: string) => Promise<unknown> | unknown, staged?: () => unknown) {
    if (!this.#durableObject) return Promise.resolve(run(operationId()));
    const gate = currentOutputGate();
    if (!gate) throw workflowError("WORKFLOW_INVARIANT_VIOLATION");
    return gate.schedule<unknown>("workflow", this.#name, encodeWorkflowValue(intent), id => Promise.resolve(run(id)), staged);
  }

  async [FLUSH_OUTPUT](payload: Uint8Array, id: string) {
    const intent = decodeWorkflowValue(payload) as WorkflowIntent;
    if (!intent || typeof intent !== "object" || typeof intent.op !== "string") {
      throw workflowError("WORKFLOW_INVARIANT_VIOLATION");
    }
    if (intent.op === "create") {
      return new WorkflowInstance(await this.#transport.create(intent.create, id), this);
    }
    if (intent.op === "createBatch") {
      return (await this.#transport.createBatch(intent.creates, id)).map(result => new WorkflowInstance(result, this));
    }
    if (intent.op === "deleteBatch") return this.#transport.deleteBatch(intent.instanceIds, id);
    const resolved = await this.#transport.get(intent.instanceId);
    if (intent.op === "pause") return resolved.handle.pause(id);
    if (intent.op === "resume") return resolved.handle.resume(id);
    if (intent.op === "terminate") return resolved.handle.terminate(intent.options, id);
    if (intent.op === "restart") return resolved.handle.restart(intent.options, id);
    if (intent.op === "delete") return resolved.handle.delete(id);
    if (intent.op === "sendEvent") return resolved.handle.sendEvent(intent.event, id);
    throw workflowError("WORKFLOW_INVARIANT_VIOLATION");
  }

  async create(options: unknown = {}) {
    const body = createWire(options);
    const result = await this.dispatchIntent(
      { op: "create", create: body },
      id => this.#transport.create(body, id),
      () => this.#transport.resolve(body.id!),
    );
    return new WorkflowInstance(result as WorkflowResolvedInstance, this);
  }
  async get(id: string) {
    const result = await this.#transport.get(instanceId(id));
    return new WorkflowInstance(result, this);
  }
  async createBatch(batch: unknown) {
    if (!Array.isArray(batch) || batch.length < 1 || batch.length > 100) {
      throw workflowError("WORKFLOW_METHOD_UNSUPPORTED");
    }
    const creates = batch.map(value => createWire(value));
    const results = await this.dispatchIntent(
      { op: "createBatch", creates },
      id => this.#transport.createBatch(creates, id),
      () => creates.map(create => this.#transport.resolve(create.id!)),
    ) as WorkflowResolvedInstance[];
    return results.map(result => new WorkflowInstance(result, this));
  }
  deleteBatch(ids: unknown) {
    if (!Array.isArray(ids) || ids.length < 1 || ids.length > 100) {
      throw workflowError("WORKFLOW_METHOD_UNSUPPORTED");
    }
    const instanceIds = ids.map(instanceId);
    return this.dispatchIntent(
      { op: "deleteBatch", instanceIds },
      id => this.#transport.deleteBatch(instanceIds, id),
    );
  }
}

/** Trigger a configured direct Workflow schedule from the system scheduled adapter. */
export async function triggerWorkflowSchedule(
  value: unknown,
  schedule: { cron: string; scheduledTime: number },
): Promise<void> {
  if (!value || typeof value !== "object") throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
  const trigger = scheduledTriggers.get(value);
  if (!trigger) throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
  await trigger(schedule);
}

function rawTransport(raw: unknown): raw is WorkflowTransport {
    return raw !== null && typeof raw === "object"
    && "resolve" in raw && typeof raw.resolve === "function"
    && "create" in raw && typeof raw.create === "function"
    && "get" in raw && typeof raw.get === "function"
    && "createBatch" in raw && typeof raw.createBatch === "function"
    && "deleteBatch" in raw && typeof raw.deleteBatch === "function";
}
