interface QueueRawTransport {
  send(frame: Uint8Array): Promise<unknown>;
  sendBatch(frame: Uint8Array): Promise<unknown>;
  metrics(): Promise<unknown>;
}
interface QueueOptions { contentType?: unknown; delaySeconds?: number }
interface SerializedMessage {
  contentType: "json" | "text" | "bytes";
  bytes: Uint8Array;
  delaySeconds?: number | undefined;
}
interface QueueMetrics {
  backlogCount: number;
  backlogBytes: number;
  oldestMessageTimestamp?: Date;
}
const producerState = new WeakMap<object, { raw: QueueRawTransport; durableObject: boolean }>();
const encoder = new TextEncoder();
const MAX_MESSAGE_BYTES = 128000;
const MAX_BATCH_MESSAGES = 100;
const MAX_BATCH_BODY_BYTES = 256000;
const MAX_DELAY_SECONDS = 86400;

function typeError(code: string): never {
  const error = Object.assign(new TypeError(code), { stableCode: code });
  throw error;
}

function object(value: unknown, code: string): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value)) typeError(code);
  return value as Record<string, unknown>;
}

function delay(value: unknown): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0 || value > MAX_DELAY_SECONDS) {
    typeError("QUEUE_DELAY_INVALID");
  }
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

function serialize(body: unknown, requested: unknown): SerializedMessage {
  const contentType = requested === undefined ? "json" : requested;
  let bytes;
  if (contentType === "json") {
    if (body === undefined) typeError("QUEUE_INVALID_MESSAGE");
    let text;
    try { text = JSON.stringify(body); } catch { typeError("QUEUE_INVALID_MESSAGE"); }
    if (text === undefined) typeError("QUEUE_INVALID_MESSAGE");
    bytes = encoder.encode(text);
  } else if (contentType === "text") {
    if (typeof body !== "string") typeError("QUEUE_INVALID_MESSAGE");
    bytes = encoder.encode(body);
  } else if (contentType === "bytes") {
    if (!ArrayBuffer.isView(body)) typeError("QUEUE_INVALID_MESSAGE");
    try {
      bytes = new Uint8Array(body.byteLength);
      bytes.set(new Uint8Array(body.buffer, body.byteOffset, body.byteLength));
    } catch {
      typeError("QUEUE_INVALID_MESSAGE");
    }
  } else if (contentType === "v8") {
    typeError("QUEUE_CONTENT_TYPE_UNSUPPORTED");
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
      || typeof value.backlogBytes !== "number" || !Number.isSafeInteger(value.backlogBytes) || value.backlogBytes < 0
      || (value.oldestMessageTimestampMs !== null
        && (typeof value.oldestMessageTimestampMs !== "number" || !Number.isSafeInteger(value.oldestMessageTimestampMs)))) {
    typeError("QUEUE_INVARIANT_VIOLATION");
  }
  const output: QueueMetrics = {
    backlogCount: value.backlogCount,
    backlogBytes: value.backlogBytes,
  };
  if (value.oldestMessageTimestampMs !== null) {
    output.oldestMessageTimestamp = new Date(value.oldestMessageTimestampMs);
  }
  return output;
}

function response(value: unknown) {
  return { metadata: { metrics: publicMetrics(value) } };
}

/** P2.2 Queue producer facade; consumer methods are intentionally absent. */
export class QueueProducer {
  constructor(raw: unknown, durableObject = false) {
    if (!rawTransport(raw)) typeError("QUEUE_INVARIANT_VIOLATION");
    producerState.set(this, Object.freeze({ raw, durableObject: durableObject === true }));
  }

  #state() {
    const state = producerState.get(this);
    if (!state) typeError("QUEUE_INVARIANT_VIOLATION");
    if (state.durableObject) typeError("QUEUE_DO_OUTPUT_GATE_UNSUPPORTED");
    return state;
  }

  async send(body: unknown, rawOptions?: unknown) {
    const normalized = options(rawOptions, ["contentType", "delaySeconds"]);
    const message = serialize(body, normalized.contentType);
    message.delaySeconds = normalized.delaySeconds;
    return response(await this.#state().raw.send(
      frame([message], undefined, 1),
    ));
  }

  async sendBatch(iterable: Iterable<unknown>, rawOptions?: unknown) {
    const normalized = options(rawOptions, ["delaySeconds"]);
    if (iterable == null || typeof iterable[Symbol.iterator] !== "function") {
      typeError("QUEUE_INVALID_MESSAGE");
    }
    const messages: SerializedMessage[] = [];
    let total = 0;
    for (const value of iterable) {
      if (messages.length === MAX_BATCH_MESSAGES) typeError("QUEUE_BATCH_LIMIT_EXCEEDED");
      const input = object(value, "QUEUE_INVALID_MESSAGE");
      if (!Object.prototype.hasOwnProperty.call(input, "body")) {
        typeError("QUEUE_INVALID_MESSAGE");
      }
      if (Object.keys(input).some((key) => !["body", "contentType", "delaySeconds"].includes(key))) {
        typeError("QUEUE_INVALID_MESSAGE");
      }
      const message = serialize(input.body, input.contentType);
      if (input.delaySeconds !== undefined) message.delaySeconds = delay(input.delaySeconds);
      total += message.bytes.byteLength;
      if (total > MAX_BATCH_BODY_BYTES) typeError("QUEUE_BATCH_LIMIT_EXCEEDED");
      messages.push(message);
    }
    if (messages.length === 0) typeError("QUEUE_INVALID_MESSAGE");
    return response(await this.#state().raw.sendBatch(
      frame(messages, normalized.delaySeconds, 2),
    ));
  }

  async metrics() {
    return publicMetrics(await this.#state().raw.metrics());
  }
}

function rawTransport(raw: unknown): raw is QueueRawTransport {
  return raw !== null && typeof raw === "object"
    && "send" in raw && typeof raw.send === "function"
    && "sendBatch" in raw && typeof raw.sendBatch === "function"
    && "metrics" in raw && typeof raw.metrics === "function";
}
