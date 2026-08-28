// Generated from runtime/src/workflows/facade-v2.ts by Rolldown. Do not edit.
import { workflowError, workflowJson } from "./json-v2.js";
function instanceId(value) {
	if (typeof value !== "string" || value.length > 100 || !/^[a-zA-Z0-9_][a-zA-Z0-9_-]*$/.test(value)) {
		throw workflowError("WORKFLOW_INSTANCE_ID_INVALID");
	}
	return value;
}
function fields(value, allowed) {
	if (!value || typeof value !== "object" || Array.isArray(value) || Object.keys(value).some((key) => !allowed.includes(key))) {
		throw workflowError("WORKFLOW_METHOD_UNSUPPORTED");
	}
}
function unsupported() {
	throw workflowError("WORKFLOW_METHOD_UNSUPPORTED");
}
class WorkflowInstanceV2 {
	#handle;
	#durableObject;
	constructor(result, durableObject) {
		// The handle is a system-isolate RpcTarget. No UUID, generation or nonce
		// enters this facade; resolving by name happens only in binding.get/create.
		this.#handle = result.handle;
		this.#durableObject = durableObject;
		Object.defineProperty(this, "id", {
			value: instanceId(result.id),
			enumerable: true
		});
		Object.freeze(this);
	}
	status() {
		return this.#handle.status();
	}
	#mutation() {
		if (this.#durableObject) throw workflowError("WORKFLOW_DO_OUTPUT_GATE_UNSUPPORTED");
	}
	pause(options = {}) {
		this.#mutation();
		fields(options, []);
		return this.#handle.pause();
	}
	resume(options = {}) {
		this.#mutation();
		fields(options, []);
		return this.#handle.resume();
	}
	terminate(options = {}) {
		this.#mutation();
		fields(options, []);
		return this.#handle.terminate();
	}
	restart(options = {}) {
		this.#mutation();
		fields(options, []);
		return this.#handle.restart();
	}
	sendEvent(event) {
		this.#mutation();
		fields(event, ["type", "payload"]);
		if (typeof event.type !== "string" || event.type.length > 100 || !/^[a-zA-Z0-9_][a-zA-Z0-9_-]*$/.test(event.type)) {
			throw workflowError("WORKFLOW_EVENT_TYPE_INVALID");
		}
		return this.#handle.sendEvent({
			type: event.type,
			payloadJson: workflowJson(event.payload, "WORKFLOW_PAYLOAD_TOO_LARGE")
		});
	}
	delete() {
		return unsupported();
	}
}
export class WorkflowBindingV2 {
	#transport;
	#durableObject;
	constructor(transport, durableObject = false) {
		if (!rawTransport(transport)) throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
		this.#transport = transport;
		this.#durableObject = durableObject;
		Object.freeze(this);
	}
	async create(options = {}) {
		if (this.#durableObject) throw workflowError("WORKFLOW_DO_OUTPUT_GATE_UNSUPPORTED");
		fields(options, [
			"id",
			"params",
			"retention"
		]);
		const id = options.id === undefined ? undefined : instanceId(options.id);
		if (options.retention !== undefined) fields(options.retention, ["successRetention", "errorRetention"]);
		const result = await this.#transport.create({
			id,
			payloadJson: workflowJson(options.params, "WORKFLOW_PAYLOAD_TOO_LARGE"),
			retention: options.retention
		});
		return new WorkflowInstanceV2(result, this.#durableObject);
	}
	async get(id) {
		const result = await this.#transport.get(instanceId(id));
		return new WorkflowInstanceV2(result, this.#durableObject);
	}
	createBatch() {
		return unsupported();
	}
}
function rawTransport(raw) {
	return raw !== null && typeof raw === "object" && "create" in raw && typeof raw.create === "function" && "get" in raw && typeof raw.get === "function";
}
