# Queues

Queues deliver messages from a producer Worker to a consumer Worker. Delivery is at-least-once. Durability is single-node `scheduler.sqlite`, not Cloudflare global replication. There is no global FIFO.

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    await env.QUEUE.send({ hello: "world" });
    return new Response("queued");
  },
  async queue(batch: MessageBatch<{ hello: string }>, env: Env): Promise<void> {
    for (const message of batch.messages) {
      console.log(message.body);
      message.ack();
    }
  },
} satisfies ExportedHandler<{ QUEUE: Queue }>;
```

## Same as Cloudflare

Producer / consumer JavaScript APIs match [Queues JavaScript APIs](https://developers.cloudflare.com/queues/configuration/javascript-apis/): `send` / `sendBatch`, `contentType` (json / text / bytes / v8), `delaySeconds`, `metrics`, consumer `MessageBatch` / `ack` / `retry`. 63 target members are `supported_with_deviation`.

```json
{
  "name": "queue-app",
  "main": "src/index.ts",
  "bindings": {
    "QUEUE": { "type": "queue_producer", "id": "<queue-id>" }
  }
}
```

A producer is an ordinary product binding `{type, id, permissions?}`. A consumer is the Worker's `queue` handler (same as Cloudflare). Do not write Wrangler `[[queues.consumers]]`. Binding grammar: [bindings](/en/workers/configuration/bindings).

## Intentional differences

**`OC-QUEUE-001`**: Queue producers and push consumers are backed by single-node `scheduler.sqlite` durability, not Cloudflare global replication. Delivery is at-least-once without global FIFO. An unknown native dispatch retains its lease and does not consume the tenant retry budget, so a later delivery can repeat the same attempt number.

Full text: [Deviations](/en/queues/platform/deviations) and [Compatibility](/en/platform/compatibility).

## In this section

- [Get started](/en/queues/get-started/)
- [Concepts](/en/queues/concepts/)
- [Guides](/en/queues/guides/)
- [Examples](/en/queues/examples/)
- [Limits](/en/queues/platform/limits)
- [Deviations](/en/queues/platform/deviations)
