import { bindingError } from "../loader/shared.js";

/** Encode bounded metadata followed by an optional one-shot body stream. */
export function framed(
  metadata: unknown,
  body: ReadableStream<Uint8Array> | null,
  limitCode: string,
): ReadableStream<Uint8Array> {
  let json: string;
  try {
    const encoded = JSON.stringify(metadata);
    if (encoded === undefined) throw bindingError(limitCode);
    json = encoded;
  } catch {
    throw bindingError(limitCode);
  }
  const value = new TextEncoder().encode(json);
  if (value.byteLength > 64 * 1024) throw bindingError(limitCode);
  const prefix = new Uint8Array(value.byteLength + 4);
  new DataView(prefix.buffer).setUint32(0, value.byteLength, false);
  prefix.set(value, 4);
  if (!body) {
    return new ReadableStream({
      start(controller) { controller.enqueue(prefix); controller.close(); },
    });
  }
  const reader = body.getReader();
  let sent = false;
  return new ReadableStream<Uint8Array>({
    async pull(controller) {
      if (!sent) { sent = true; controller.enqueue(prefix); return; }
      const part = await reader.read();
      if (part.done) controller.close(); else controller.enqueue(part.value);
    },
    cancel(reason) { return reader.cancel(reason); },
  });
}

/** Require one exact private-backend response status and release mismatched bodies. */
export async function expectBindingStatus(
  response: Response,
  status: number,
  code: string,
): Promise<void> {
  if (response.status === status) return;
  try { await response.body?.cancel(); } catch { /* best effort */ }
  throw bindingError(code);
}

/** Parse private JSON without leaking native parser or stream failures. */
export async function bindingJson(response: Response, code: string): Promise<unknown> {
  try {
    return await response.json();
  } catch {
    throw bindingError(code);
  }
}

/** Narrow an untrusted private payload to a non-array object. */
export function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
