interface Env {
  EVENTS: Queue<{ marker: string } | string | Uint8Array | V8Body>;
}

interface V8Body {
  v8: true;
  when: Date;
  items: Map<string, Set<number>>;
}

export default {
  async fetch(_request: Request, env: Env): Promise<Response> {
    const send: QueueSendResponse = await env.EVENTS.send({ marker: "json" });
    const text: QueueSendResponse = await env.EVENTS.send("plain", { contentType: "text", delaySeconds: 0 });
    const bytes: QueueSendResponse = await env.EVENTS.send(new Uint8Array(1), { contentType: "bytes" });
    const v8: QueueSendResponse = await env.EVENTS.send({
      v8: true, when: new Date(), items: new Map([["k", new Set([1])]]),
    }, { contentType: "v8" });
    const batch: QueueSendBatchResponse = await env.EVENTS.sendBatch([
      { body: "a", contentType: "text", delaySeconds: 1 },
      { body: { marker: "json" } },
    ], { delaySeconds: 2 });
    const metrics: QueueMetrics = await env.EVENTS.metrics();
    const oldest: Date | undefined = metrics.oldestMessageTimestamp;
    const sendOldest: Date | undefined = send.metadata.metrics.oldestMessageTimestamp;
    const batchOldest: Date | undefined = batch.metadata.metrics.oldestMessageTimestamp;
    const backlog: number = metrics.backlogCount + metrics.backlogBytes
      + send.metadata.metrics.backlogCount + text.metadata.metrics.backlogBytes
      + bytes.metadata.metrics.backlogCount + v8.metadata.metrics.backlogBytes
      + batch.metadata.metrics.backlogCount;
    return new Response(JSON.stringify({ backlog, oldest, sendOldest, batchOldest }));
  },
  async queue(batch: MessageBatch<unknown>, _env: Env, ctx: ExecutionContext): Promise<void> {
    const queue: string = batch.queue;
    const metadata: MessageBatchMetadata = batch.metadata;
    const metrics: MessageBatchMetrics = metadata.metrics;
    const oldest: Date | undefined = metrics.oldestMessageTimestamp;
    ctx.waitUntil(Promise.resolve(queue));
    for (const message of batch.messages) {
      const id: string = message.id;
      const attempts: number = message.attempts;
      const timestamp: Date = message.timestamp;
      const body: unknown = message.body;
      message.retry({ delaySeconds: 1 });
      message.ack();
      void id;
      void attempts;
      void timestamp;
      void body;
    }
    batch.retryAll({ delaySeconds: 2 });
    batch.ackAll();
    void oldest;
    void metrics.backlogCount;
    void metrics.backlogBytes;
  },
} satisfies ExportedHandler<Env>;
