---
title: "Queues"
---

Queues deliver messages from a producer Worker to a consumer Worker. Delivery is at-least-once. Durability comes from `scheduler.sqlite` on the node running ocd.

For example, you can use Queues for:

- Decoupling producer and consumer Workers
- Buffering work for asynchronous processing
- Retrying failed deliveries

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

Bind a producer with Wrangler's standard Queues field:

```json
{
  "name": "queue-app",
  "main": "src/index.ts",
  "queues": {
    "producers": [{ "binding": "QUEUE", "queue": "jobs" }],
    "consumers": [{ "queue": "jobs", "max_batch_size": 10 }]
  }
}
```

A consumer targets the Worker's `queue` handler through `queues.consumers`. Binding grammar: [bindings](/docs/workers/configuration/bindings/). Pinned Wrangler owns queue provisioning and consumer configuration.

The management SDK also exposes Cloudflare's official producer routes:

```ts
await client.queues.messages.push(queueId, {
  account_id,
  body: { job: 42 },
  content_type: "json",
});
await client.queues.messages.bulkPush(queueId, {
  account_id,
  messages: [{ body: "one", content_type: "text" }],
});
```

These calls commit to the same durable queue authority as Worker `send()` / `sendBatch()`. A timeout after enqueue is result-unknown and may require application-level deduplication. Management `pull`, `ack`, `peek`, and `purge` remain unsupported because open-compute does not provide the HTTP-pull lease protocol.

## Compatibility

| Topic                   | Cloudflare                                                                                        | open-compute                                                                                                                               |
| ----------------------- | ------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| JavaScript API          | [Queues JavaScript APIs](https://developers.cloudflare.com/queues/configuration/javascript-apis/) | Same: `send` / `sendBatch`, `contentType` (json / text / bytes / v8), `delaySeconds`, `metrics`, consumer `MessageBatch` / `ack` / `retry` |
| Durability              | Global replication                                                                                | Local `scheduler.sqlite` on the node running ocd                                                                                           |
| Delivery                | At-least-once                                                                                     | At-least-once                                                                                                                              |
| Global FIFO             | Available                                                                                         | Not provided                                                                                                                               |
| Unknown native dispatch | —                                                                                                 | May retain the lease; duplicate attempt numbers possible                                                                                   |
| Pull consumer           | Available                                                                                         | Not provided                                                                                                                               |
| Binding                 | Wrangler `queues`                                                                                 | Standard `producers` and `consumers` entries                                                                                               |

Next: [Develop with bindings](/docs/develop/) · [Compatibility and limits](/docs/reference/)
