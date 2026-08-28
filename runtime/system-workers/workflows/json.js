// Generated from runtime/src/workflows/json.ts by Rolldown. Do not edit.
// Capability V1 canonical JSON. No toJSON hooks or structured-clone extensions.
const encoder = new TextEncoder();
export const MAX_WORKFLOW_JSON_BYTES = 1024 * 1024;
export function workflowError(code) {
	const error = new Error(code);
	error.stack = `Error: ${code}`;
	return error;
}
export function workflowString(value, maximum, code) {
	if (typeof value !== "string" || !value.isWellFormed() || encoder.encode(value).byteLength > maximum) throw workflowError(code);
	return value;
}
export function workflowJson(value, tooLarge = "WORKFLOW_RESULT_TOO_LARGE") {
	const parts = [];
	const seen = new Set();
	let bytes = 0;
	const push = (part) => {
		bytes += encoder.encode(part).byteLength;
		if (bytes > MAX_WORKFLOW_JSON_BYTES) throw workflowError(tooLarge);
		parts.push(part);
	};
	const string = (value) => {
		if (!value.isWellFormed()) throw workflowError("WORKFLOW_SERIALIZATION_UNSUPPORTED");
		if (encoder.encode(value).byteLength > MAX_WORKFLOW_JSON_BYTES) throw workflowError(tooLarge);
		return JSON.stringify(value);
	};
	const omitted = (value) => [
		"undefined",
		"function",
		"symbol"
	].includes(typeof value);
	const write = (value, depth) => {
		if (omitted(value) || value === null) {
			push("null");
			return;
		}
		if (typeof value === "string") {
			push(string(value));
			return;
		}
		if (typeof value === "number" || typeof value === "boolean") {
			push(JSON.stringify(value));
			return;
		}
		if (typeof value !== "object" || seen.has(value) || depth >= 127) {
			throw workflowError("WORKFLOW_SERIALIZATION_UNSUPPORTED");
		}
		const array = Array.isArray(value);
		if (!array && ![Object.prototype, null].includes(Object.getPrototypeOf(value))) {
			throw workflowError("WORKFLOW_SERIALIZATION_UNSUPPORTED");
		}
		seen.add(value);
		push(array ? "[" : "{");
		let first = true;
		if (array) {
			for (let index = 0; index < value.length; index++) {
				if (!first) push(",");
				first = false;
				write(value[index], depth + 1);
			}
		} else {
			// Unicode scalar order equals UTF-8 byte order, unlike UTF-16 sort().
			const keys = Object.keys(value).map((key) => ({
				key,
				bytes: encoder.encode(workflowString(key, MAX_WORKFLOW_JSON_BYTES, "WORKFLOW_SERIALIZATION_UNSUPPORTED"))
			}));
			keys.sort((a, b) => {
				for (let i = 0; i < Math.min(a.bytes.length, b.bytes.length); i++) {
					if (a.bytes[i] !== b.bytes[i]) return a.bytes[i] - b.bytes[i];
				}
				return a.bytes.length - b.bytes.length;
			});
			for (const { key } of keys) {
				const entry = Reflect.get(value, key);
				if (omitted(entry)) continue;
				if (!first) push(",");
				first = false;
				push(string(key));
				push(":");
				write(entry, depth + 1);
			}
		}
		push(array ? "]" : "}");
		seen.delete(value);
	};
	write(value, 0);
	return parts.join("");
}
// Tenant exceptions can contain bindings, secrets, SQL, or private URLs. Persist
// only a stable category; never copy arbitrary exception text into status/logs.
export function workflowFailure() {
	return {
		name: "Error",
		message: "Workflow execution failed"
	};
}
