const producerState = new WeakMap();
const encoder = new TextEncoder();
const MAX_MESSAGE_BYTES = 128000;
const MAX_BATCH_MESSAGES = 100;
const MAX_BATCH_BODY_BYTES = 256000;
const MAX_DELAY_SECONDS = 86400;

function typeError(code) {
  const error = new TypeError(code);
  error.stableCode = code;
  throw error;
}

function object(value, code) {
  if (!value || typeof value !== "object" || Array.isArray(value)) typeError(code);
  return value;
}

function delay(value) {
  if (!Number.isSafeInteger(value) || value < 0 || value > MAX_DELAY_SECONDS) {
    typeError("QUEUE_DELAY_INVALID");
  }
  return value;
}

function options(value, allowed) {
  if (value === undefined) return {};
  const input = object(value, "QUEUE_INVALID_MESSAGE");
  if (Object.keys(input).some((key) => !allowed.includes(key))) {
    typeError("QUEUE_INVALID_MESSAGE");
  }
  const output = {};
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

function serialize(body, requested) {
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

function contentCode(value) {
  if (value === "json") return 1;
  if (value === "text") return 2;
  if (value === "bytes") return 3;
  typeError("QUEUE_CONTENT_TYPE_UNSUPPORTED");
}

function frame(messages, batchDelay, operation) {
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

function publicMetrics(value) {
  if (!value || !Number.isSafeInteger(value.backlogCount) || value.backlogCount < 0
      || !Number.isSafeInteger(value.backlogBytes) || value.backlogBytes < 0
      || (value.oldestMessageTimestampMs !== null
        && !Number.isSafeInteger(value.oldestMessageTimestampMs))) {
    typeError("QUEUE_INVARIANT_VIOLATION");
  }
  const output = {
    backlogCount: value.backlogCount,
    backlogBytes: value.backlogBytes,
  };
  if (value.oldestMessageTimestampMs !== null) {
    output.oldestMessageTimestamp = new Date(value.oldestMessageTimestampMs);
  }
  return output;
}

function response(value) {
  return { metadata: { metrics: publicMetrics(value) } };
}

/** P2.2 Queue producer facade; consumer methods are intentionally absent. */
export class QueueProducer {
  constructor(raw, durableObject = false) {
    if (!raw || typeof raw.send !== "function" || typeof raw.sendBatch !== "function"
        || typeof raw.metrics !== "function") typeError("QUEUE_INVARIANT_VIOLATION");
    producerState.set(this, Object.freeze({ raw, durableObject: durableObject === true }));
  }

  #state() {
    const state = producerState.get(this);
    if (!state) typeError("QUEUE_INVARIANT_VIOLATION");
    if (state.durableObject) typeError("QUEUE_DO_OUTPUT_GATE_UNSUPPORTED");
    return state;
  }

  async send(body, rawOptions) {
    const normalized = options(rawOptions, ["contentType", "delaySeconds"]);
    const message = serialize(body, normalized.contentType);
    message.delaySeconds = normalized.delaySeconds;
    return response(await this.#state().raw.send(
      frame([message], undefined, 1),
    ));
  }

  async sendBatch(iterable, rawOptions) {
    const normalized = options(rawOptions, ["delaySeconds"]);
    if (iterable == null || typeof iterable[Symbol.iterator] !== "function") {
      typeError("QUEUE_INVALID_MESSAGE");
    }
    const messages = [];
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
