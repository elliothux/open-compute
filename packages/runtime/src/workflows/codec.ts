import {
  decodeDurableValue,
  durableValueErrorCode,
  encodeDurableValue,
} from "../serialization/codec.js";

const encoder = new TextEncoder();

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

export function encodeWorkflowValue(
  value: unknown,
  tooLarge = "WORKFLOW_RESULT_TOO_LARGE",
): Uint8Array<ArrayBuffer> {
  try {
    return encodeDurableValue(value, "workflow");
  } catch (error) {
    const code = durableValueErrorCode(error, "workflow");
    if (code === "WORKFLOW_RESULT_TOO_LARGE" && tooLarge !== code) {
      throw workflowError(tooLarge);
    }
    throw error;
  }
}

export function decodeWorkflowValue(value: unknown): unknown {
  return decodeDurableValue(value, "workflow");
}

export function workflowSerializationCode(error: unknown): string {
  const code = durableValueErrorCode(error, "workflow");
  return code === "WORKFLOW_RESULT_TOO_LARGE"
    ? code : "WORKFLOW_SERIALIZATION_UNSUPPORTED";
}

export function bytesBase64(bytes: Uint8Array): string {
  let output = "";
  for (let offset = 0; offset < bytes.byteLength; offset += 0x8000) {
    output += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  }
  return btoa(output);
}

export function base64Bytes(encoded: unknown): Uint8Array<ArrayBuffer> {
  if (typeof encoded !== "string" || encoded.length > 1_398_112
      || encoded.length % 4 !== 0 || !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(encoded)) {
    throw workflowError("WORKFLOW_SERIALIZATION_MALFORMED");
  }
  let binary: string;
  try { binary = atob(encoded); }
  catch { throw workflowError("WORKFLOW_SERIALIZATION_MALFORMED"); }
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

export function encodeWorkflowBase64(value: unknown, tooLarge?: string): string {
  return bytesBase64(encodeWorkflowValue(value, tooLarge));
}

export function decodeWorkflowBase64(value: unknown): unknown {
  return decodeWorkflowValue(base64Bytes(value));
}

// Tenant exceptions can contain bindings, secrets, SQL, or private URLs. Persist
// only a stable category; never copy arbitrary exception text into status/logs.
export function workflowFailure() {
  return { name: "Error", message: "Workflow execution failed" };
}
