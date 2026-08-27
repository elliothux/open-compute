import { workflowError, workflowJson } from "./__open_compute_workflow_json__.js";

function instanceId(value) {
  if (typeof value !== "string" || value.length > 100 || !/^[a-zA-Z0-9_][a-zA-Z0-9_-]*$/.test(value)) {
    throw workflowError("WORKFLOW_INSTANCE_ID_INVALID");
  }
  return value;
}

function unsupported() { throw workflowError("WORKFLOW_METHOD_UNSUPPORTED"); }

class WorkflowInstance {
  #transport;
  constructor(transport, id) {
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
  #transport;
  #durableObject;
  constructor(transport, durableObject = false) {
    this.#transport = transport;
    this.#durableObject = durableObject;
    Object.freeze(this);
  }
  async create(options = {}) {
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
  async get(id) {
    instanceId(id);
    await this.#transport.get(id);
    return new WorkflowInstance(this.#transport, id);
  }
  createBatch() { return unsupported(); }
}
