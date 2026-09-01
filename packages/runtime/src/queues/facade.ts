import { durableValueErrorCode, encodeDurableValue } from "../serialization/codec.js";
import { unsafeBuffer } from "../serialization/format.js";

import {
  currentOutputGate, FINALIZE_OUTPUT, FLUSH_OUTPUT,
} from "../durable-objects/output-gate.js";

interface QueueRawTransport {
  send(frame: Uint8Array, operationId?: string): Promise<unknown>;
  sendBatch(frame: Uint8Array, operationId?: string): Promise<unknown>;
  finalize(operationId: string): Promise<void>;
  metrics(): Promise<unknown>;
}
interface QueueOptions { contentType?: unknown; delaySeconds?: number }
interface SerializedMessage {
  contentType: "json" | "text" | "bytes" | "v8";
  bytes: Uint8Array;
  delaySeconds?: number | undefined;
}
interface QueueMetrics {
  backlogCount: number;
  backlogBytes: number;
  oldestMessageTimestamp?: Date;
}
const producerState = new WeakMap<object, { raw: QueueRawTransport; durableObject: boolean; name: string }>();
const encoder = new TextEncoder();
const MAX_MESSAGE_BYTES = 128000;
const MAX_BATCH_MESSAGES = 100;
const MAX_BATCH_BODY_BYTES = 256000;
const MAX_DELAY_SECONDS = 86400;

function typeError(code: string): never {
  const error = Object.assign(new TypeError(code), { stableCode: code });
  throw error;
}

function queueError(code: string): never {
  const error = Object.assign(new Error(code), { stableCode: code });
  throw error;
}

function object(value: unknown, code: string): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) typeError(code);
  return value as Record<string, unknown>;
}

function delay(value: unknown): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value)) {
    typeError("QUEUE_DELAY_INVALID");
  }
  if (value < 0 || value > MAX_DELAY_SECONDS) queueError("QUEUE_DELAY_INVALID");
  return value;
}

function options(value: unknown, allowed: readonly string[]): QueueOptions {
  if (value === undefined) return {};
  const input = object(value, "QUEUE_INVALID_MESSAGE");
  if (Object.keys(input).some((key) => !allowed.includes(key))) {
    typeError("QUEUE_INVALID_MESSAGE");
  }
  const output: QueueOptions = {};
  if (Object.prototype.hasOwnProperty.call(input, "delaySeconds")
      && input.delaySeconds !== undefined) {
    output.delaySeconds = delay(input.delaySeconds);
  }
  if (Object.prototype.hasOwnProperty.call(input, "contentType")
      && input.contentType !== undefined) {
    output.contentType = input.contentType;
  }
  return output;
}

function copyBytes(body: ArrayBufferView, detach: boolean): Uint8Array {
  let buffer: ArrayBuffer;
  try { buffer = body.buffer as ArrayBuffer; }
  catch { typeError("QUEUE_INVALID_MESSAGE"); }
  if (!(buffer instanceof ArrayBuffer) || unsafeBuffer(buffer)) typeError("QUEUE_INVALID_MESSAGE");
  const bytes = new Uint8Array(body.byteLength);
  bytes.set(new Uint8Array(buffer, body.byteOffset, body.byteLength));
  if (detach) {
    try { structuredClone(buffer, { transfer: [buffer] }); } catch { /* not detachable */ }
  }
  return bytes;
}

function serialize(body: unknown, requested: unknown, detachBytes: boolean): SerializedMessage {
  if (body === undefined) typeError("QUEUE_INVALID_MESSAGE");
  const contentType = requested === undefined ? "json" : requested;
  let bytes;
  if (contentType === "json") {
    let text;
    try { text = JSON.stringify(body); } catch { typeError("QUEUE_INVALID_MESSAGE"); }
    if (text === undefined) typeError("QUEUE_INVALID_MESSAGE");
    bytes = encoder.encode(text);
  } else if (contentType === "text") {
    if (typeof body !== "string") typeError("QUEUE_INVALID_MESSAGE");
    bytes = encoder.encode(body);
  } else if (contentType === "bytes") {
    if (!ArrayBuffer.isView(body)) typeError("QUEUE_INVALID_MESSAGE");
    bytes = copyBytes(body, detachBytes);
  } else if (contentType === "v8") {
    try { bytes = encodeDurableValue(body, "queue-v8"); }
    catch (error) {
      if (durableValueErrorCode(error, "queue-v8")) throw error;
      typeError("QUEUE_V8_UNSUPPORTED");
    }
  } else {
    typeError("QUEUE_CONTENT_TYPE_UNSUPPORTED");
  }
  if (bytes.byteLength > MAX_MESSAGE_BYTES) typeError("QUEUE_MESSAGE_TOO_LARGE");
  return { contentType, bytes };
}

function contentCode(value: SerializedMessage["contentType"]): number {
  if (value === "json") return 1;
  if (value === "text") return 2;
  if (value === "bytes") return 3;
  if (value === "v8") return 4;
  typeError("QUEUE_CONTENT_TYPE_UNSUPPORTED");
}

function frame(messages: readonly SerializedMessage[], batchDelay: number | undefined, operation: number): Uint8Array {
  let length = 11;
  for (const message of messages) length += 9 + message.bytes.byteLength;
  const output = new Uint8Array(length);
  output.set([0x4f, 0x43, 0x51, 0x31], 0);
  const view = new DataView(output.buffer);
  view.setUint8(4, operation);
  view.setUint16(5, messages.length);
  view.setInt32(7, batchDelay === undefined ? -1 : batchDelay);
  let offset = 11;
  for (const message of messages) {
    view.setUint8(offset, contentCode(message.contentType));
    view.setInt32(offset + 1, message.delaySeconds === undefined ? -1 : message.delaySeconds);
    view.setUint32(offset + 5, message.bytes.byteLength);
    output.set(message.bytes, offset + 9);
    offset += 9 + message.bytes.byteLength;
  }
  return output;
}

function publicMetrics(input: unknown): QueueMetrics {
  const value = object(input, "QUEUE_INVARIANT_VIOLATION");
  if (typeof value.backlogCount !== "number" || !Number.isSafeInteger(value.backlogCount) || value.backlogCount < 0
      || typeof value.backlogBytes !== "number" || !Number.isSafeInteger(value.backlogBytes) || value.backlogBytes < 0) {
    typeError("QUEUE_INVARIANT_VIOLATION");
  }
  const output: QueueMetrics = {
    backlogCount: value.backlogCount,
    backlogBytes: value.backlogBytes,
  };
  const oldest = value.oldestMessageTimestampMs;
  if (oldest !== undefined && oldest !== null && oldest !== 0) {
    if (typeof oldest !== "number" || !Number.isSafeInteger(oldest)) {
      typeError("QUEUE_INVARIANT_VIOLATION");
    }
    output.oldestMessageTimestamp = new Date(oldest);
  }
  return output;
}

function response(value: unknown) {
  return { metadata: { metrics: publicMetrics(value) } };
}

async function stagedResponse(raw: QueueRawTransport, bytes: Uint8Array) {
  const current = publicMetrics(await raw.metrics());
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const count = view.getUint16(5);
  let offset = 11;
  let addedBytes = 0;
  for (let index = 0; index < count; index += 1) {
    const length = view.getUint32(offset + 5);
    addedBytes += length;
    offset += 9 + length;
  }
  return {
    metadata: {
      metrics: {
        backlogCount: current.backlogCount + count,
        backlogBytes: current.backlogBytes + addedBytes,
        oldestMessageTimestamp: current.oldestMessageTimestamp ?? new Date(),
      },
    },
  };
}

/** Queue producer facade. Durable Object mutations wait for the object output gate. */
export class QueueProducer {
  constructor(raw: unknown, durableObject = false, name = "") {
    if (!rawTransport(raw)) typeError("QUEUE_INVARIANT_VIOLATION");
    producerState.set(this, Object.freeze({
      raw, durableObject: durableObject === true, name: typeof name === "string" ? name : "",
    }));
  }

  #state() {
    const state = producerState.get(this);
    if (!state) typeError("QUEUE_INVARIANT_VIOLATION");
    return state;
  }

  async #publish(bytes: Uint8Array, batch: boolean) {
    const state = this.#state();
    const send = (operationId?: string) => batch
      ? state.raw.sendBatch(bytes, operationId)
      : state.raw.send(bytes, operationId);
    if (!state.durableObject) return response(await send());
    const gate = currentOutputGate();
    if (!gate) typeError("QUEUE_INVARIANT_VIOLATION");
    return gate.schedule(
      "queue",
      state.name,
      bytes,
      async operationId => response(await send(operationId)),
      () => stagedResponse(state.raw, bytes),
      operationId => state.raw.finalize(operationId),
    );
  }

  [FLUSH_OUTPUT](payload: Uint8Array, operationId: string) {
    const state = this.#state();
    const send = payload[4] === 2
      ? state.raw.sendBatch(payload, operationId)
      : state.raw.send(payload, operationId);
    return send.then(response);
  }

  [FINALIZE_OUTPUT](operationId: string) {
    return this.#state().raw.finalize(operationId);
  }

  async send(body: unknown, rawOptions?: unknown) {
    const normalized = options(rawOptions, ["contentType", "delaySeconds"]);
    const message = serialize(body, normalized.contentType, true);
    message.delaySeconds = normalized.delaySeconds;
    return this.#publish(frame([message], undefined, 1), false);
  }

  async sendBatch(iterable: Iterable<unknown>, rawOptions?: unknown) {
    const normalized = options(rawOptions, ["delaySeconds"]);
    if (iterable == null || typeof iterable[Symbol.iterator] !== "function") {
      typeError("QUEUE_INVALID_MESSAGE");
    }
    const messages: SerializedMessage[] = [];
    let total = 0;
    for (const value of iterable) {
      if (messages.length === MAX_BATCH_MESSAGES) queueError("QUEUE_BATCH_LIMIT_EXCEEDED");
      const input = object(value, "QUEUE_INVALID_MESSAGE");
      if (!Object.prototype.hasOwnProperty.call(input, "body")) {
        typeError("QUEUE_INVALID_MESSAGE");
      }
      if (Object.keys(input).some((key) => !["body", "contentType", "delaySeconds"].includes(key))) {
        typeError("QUEUE_INVALID_MESSAGE");
      }
      const message = serialize(input.body, input.contentType, false);
      if (input.delaySeconds !== undefined) message.delaySeconds = delay(input.delaySeconds);
      total += message.bytes.byteLength;
      if (total > MAX_BATCH_BODY_BYTES) queueError("QUEUE_BATCH_LIMIT_EXCEEDED");
      messages.push(message);
    }
    if (messages.length === 0) typeError("QUEUE_INVALID_MESSAGE");
    return this.#publish(frame(messages, normalized.delaySeconds, 2), true);
  }

  async metrics() {
    return publicMetrics(await this.#state().raw.metrics());
  }
}

function rawTransport(raw: unknown): raw is QueueRawTransport {
  return raw !== null && typeof raw === "object"
    && "send" in raw && typeof raw.send === "function"
    && "sendBatch" in raw && typeof raw.sendBatch === "function"
    && "finalize" in raw && typeof raw.finalize === "function"
    && "metrics" in raw && typeof raw.metrics === "function";
}
