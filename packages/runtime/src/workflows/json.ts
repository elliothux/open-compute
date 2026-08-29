// The current codec privately brands serializer errors so tenant getters cannot
// forge a platform size verdict or inject their exception text into status.
const serializationFailures = new WeakMap<object, string>();
const rememberSerializationFailure = serializationFailures.set.bind(serializationFailures);
const readSerializationFailure = serializationFailures.get.bind(serializationFailures);
function serializationError(code: string): Error {
  const error = workflowError(code);
  rememberSerializationFailure(error, code);
  return error;
}
export function workflowSerializationCode(error: unknown): string {
  return error !== null && (typeof error === "object" || typeof error === "function")
    && readSerializationFailure(error) === "WORKFLOW_RESULT_TOO_LARGE"
    ? "WORKFLOW_RESULT_TOO_LARGE" : "WORKFLOW_SERIALIZATION_UNSUPPORTED";
}
const encoder = new TextEncoder();
export const MAX_WORKFLOW_JSON_BYTES = 1024 * 1024;

export function workflowError(code: string): Error {
  const error = new Error(code);
  error.stack = `Error: ${code}`;
  return error;
}

export function workflowString(value: unknown, maximum: number, code: string): string {
  if (typeof value !== "string" || !value.isWellFormed()
      || encoder.encode(value).byteLength > maximum) throw workflowError(code);
  return value;
}

export function workflowJson(value: unknown, tooLarge = "WORKFLOW_RESULT_TOO_LARGE") {
  const parts: string[] = [];
  const seen = new Set<object>();
  let bytes = 0;
  const push = (part: string): void => {
    bytes += encoder.encode(part).byteLength;
    if (bytes > MAX_WORKFLOW_JSON_BYTES) throw serializationError(tooLarge);
    parts.push(part);
  };
  const string = (value: string): string => {
    if (!value.isWellFormed()) throw serializationError("WORKFLOW_SERIALIZATION_UNSUPPORTED");
    if (encoder.encode(value).byteLength > MAX_WORKFLOW_JSON_BYTES) throw serializationError(tooLarge);
    return JSON.stringify(value);
  };
  const omitted = (value: unknown): boolean => ["undefined", "function", "symbol"].includes(typeof value);
  const write = (value: unknown, depth: number): void => {
    if (omitted(value) || value === null) { push("null"); return; }
    if (typeof value === "string") { push(string(value)); return; }
    if (typeof value === "number" || typeof value === "boolean") {
      push(JSON.stringify(value)); return;
    }
    if (typeof value !== "object" || seen.has(value) || depth >= 127) {
      throw serializationError("WORKFLOW_SERIALIZATION_UNSUPPORTED");
    }
    const array = Array.isArray(value);
    if (!array && ![Object.prototype, null].includes(Object.getPrototypeOf(value))) {
      throw serializationError("WORKFLOW_SERIALIZATION_UNSUPPORTED");
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
      const keys = Object.keys(value).map((key) => ({ key, bytes: encoder.encode(
        workflowString(key, MAX_WORKFLOW_JSON_BYTES, "WORKFLOW_SERIALIZATION_UNSUPPORTED"),
      ) }));
      keys.sort((a, b) => {
        for (let i = 0; i < Math.min(a.bytes.length, b.bytes.length); i++) {
          if (a.bytes[i] !== b.bytes[i]) return a.bytes[i]! - b.bytes[i]!;
        }
        return a.bytes.length - b.bytes.length;
      });
      for (const { key } of keys) {
        const entry: unknown = Reflect.get(value, key);
        if (omitted(entry)) continue;
        if (!first) push(",");
        first = false;
        push(string(key)); push(":"); write(entry, depth + 1);
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
  return { name: "Error", message: "Workflow execution failed" };
}
