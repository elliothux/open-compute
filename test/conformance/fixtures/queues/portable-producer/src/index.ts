interface Env {
  EVENTS: Queue<unknown>;
}

interface CapturedError {
  synchronous: boolean;
  name: string;
}

async function capture(operation: () => Promise<unknown>): Promise<CapturedError | null> {
  let promise: Promise<unknown>;
  try {
    promise = operation();
  } catch (error) {
    return { synchronous: true, name: error instanceof Error ? error.name : "unknown" };
  }
  try {
    await promise;
    return null;
  } catch (error) {
    return { synchronous: false, name: error instanceof Error ? error.name : "unknown" };
  }
}

function metadata(response: QueueSendResponse | QueueSendBatchResponse): boolean {
  const metrics = response.metadata.metrics;
  return Number.isSafeInteger(metrics.backlogCount)
    && Number.isSafeInteger(metrics.backlogBytes)
    && (metrics.oldestMessageTimestamp === undefined || metrics.oldestMessageTimestamp instanceof Date);
}

async function surface(queue: Queue<unknown>): Promise<Response> {
  const responses: Array<QueueSendResponse | QueueSendBatchResponse> = [];
  responses.push(await queue.send({ kind: "json" }));
  responses.push(await queue.send("text", { contentType: "text", delaySeconds: 0 }));
  responses.push(await queue.send(new Uint8Array([1, 2, 3]), { contentType: "bytes" }));
  responses.push(await queue.send({ when: new Date(0), values: new Map([["a", new Set([1, 2])]]) }, {
    contentType: "v8",
  }));
  responses.push(await queue.sendBatch([
    { body: { kind: "batch-json" }, contentType: "json" },
    { body: "batch-text", contentType: "text", delaySeconds: 1 },
  ], { delaySeconds: 0 }));
  const metrics = await queue.metrics();
  const counts = responses.map(response => response.metadata.metrics.backlogCount);
  return Response.json({
    responses: responses.map(metadata),
    metrics: {
      backlogCountPositive: metrics.backlogCount > 0,
      backlogBytesPositive: metrics.backlogBytes > 0,
      oldestValid: metrics.oldestMessageTimestamp === undefined
        || metrics.oldestMessageTimestamp instanceof Date,
    },
    monotonic: counts.every((count, index) => index === 0 || count >= counts[index - 1]!),
  });
}

async function errors(queue: Queue<unknown>): Promise<Response> {
  const invalidContentType = await capture(() => queue.send("x", { contentType: "invalid" as QueueContentType }));
  const negativeDelay = await capture(() => queue.send("x", { delaySeconds: -1 }));
  const highDelay = await capture(() => queue.send("x", { delaySeconds: 86_401 }));
  const emptyBatch = await capture(() => queue.sendBatch([]));
  const highBatch = await capture(() => queue.sendBatch(Array.from({ length: 101 }, () => ({ body: "x" }))));
  return Response.json({ invalidContentType, negativeDelay, highDelay, emptyBatch, highBatch });
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const path = new URL(request.url).pathname;
    if (path === "/surface" && request.method === "POST") return surface(env.EVENTS);
    if (path === "/errors" && request.method === "GET") return errors(env.EVENTS);
    if (path === "/cleanup" && request.method === "DELETE") return Response.json({ cleaned: true });
    return new Response("not found", { status: 404 });
  },
} satisfies ExportedHandler<Env>;
