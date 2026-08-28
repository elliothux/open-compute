import type { WorkflowTransport } from "./binding-protocol.js";
import { workflowError, workflowJson } from "./json.js";

function instanceId(value: unknown): string {
  if (typeof value !== "string" || value.length > 100 || !/^[a-zA-Z0-9_][a-zA-Z0-9_-]*$/.test(value)) {
    throw workflowError("WORKFLOW_INSTANCE_ID_INVALID");
  }
  return value;
}

function unsupported(): never { throw workflowError("WORKFLOW_METHOD_UNSUPPORTED"); }

class WorkflowInstance {
  declare readonly id: string;
  #transport: WorkflowTransport;
  constructor(transport: WorkflowTransport, id: string) {
    this.#transport = transport;
    Object.defineProperty(this, "id", { value: instanceId(id), enumerable: true });
    Object.freeze(this);
  }
  status() { return this.#transport.status(this.id); }
  pause() { return unsupported(); }
  resume() { return unsupported(); }
  terminate() { return unsupported(); }
  restart() { return unsupported(); }
  sendEvent() { return unsupported(); }
  delete() { return unsupported(); }
}

export class WorkflowBinding {
  #transport: WorkflowTransport;
  #durableObject;
  constructor(transport: unknown, durableObject = false) {
    if (!rawTransport(transport)) throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
    this.#transport = transport;
    this.#durableObject = durableObject;
    Object.freeze(this);
  }
  async create(options: { id?: unknown; params?: unknown } = {}) {
    if (this.#durableObject) throw workflowError("WORKFLOW_DO_OUTPUT_GATE_UNSUPPORTED");
    if (!options || typeof options !== "object" || Array.isArray(options)
        || Object.keys(options).some(key => !["id", "params"].includes(key))) {
      throw workflowError("WORKFLOW_METHOD_UNSUPPORTED");
    }
    const id = options.id === undefined ? undefined : instanceId(options.id);
    const payloadJson = workflowJson(options.params, "WORKFLOW_PAYLOAD_TOO_LARGE");
    const result = await this.#transport.create({ id, payloadJson });
    return new WorkflowInstance(this.#transport, result.id);
  }
  async get(id: string) {
    instanceId(id);
    await this.#transport.get(id);
    return new WorkflowInstance(this.#transport, id);
  }
  createBatch() { return unsupported(); }
}

function rawTransport(raw: unknown): raw is WorkflowTransport {
  return raw !== null && typeof raw === "object"
    && "create" in raw && typeof raw.create === "function"
    && "get" in raw && typeof raw.get === "function"
    && "status" in raw && typeof raw.status === "function";
}
