# Queues

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

Bind a producer in `open-compute.json`. Ordinary product bindings are `{ type, id, permissions? }`:

```json
{
  "name": "queue-app",
  "main": "src/index.ts",
  "bindings": {
    "QUEUE": { "type": "queue_producer", "id": "<queue-id>" }
  }
}
```

A consumer is the Worker's `queue` handler. `open-compute.json` does not use Wrangler `[[queues.consumers]]`. Binding grammar: [bindings](/workers/configuration/bindings). The CLI is `oc` / `oc run` / `oc types`.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| JavaScript API | [Queues JavaScript APIs](https://developers.cloudflare.com/queues/configuration/javascript-apis/) | Same: `send` / `sendBatch`, `contentType` (json / text / bytes / v8), `delaySeconds`, `metrics`, consumer `MessageBatch` / `ack` / `retry` |
| Durability | Global replication | Local `scheduler.sqlite` on the node running ocd |
| Delivery | At-least-once | At-least-once |
| Global FIFO | Available | Not provided |
| Unknown native dispatch | — | May retain the lease; duplicate attempt numbers possible |
| Pull consumer | Available | Not provided |
| Binding | wrangler `queues` | Producer `{ type, id, permissions? }`; consumer is the Worker `queue` handler |

## Next

- [Get started](/queues/get-started/)
- [Concepts](/queues/concepts/)
- [Guides](/queues/guides/)
- [Examples](/queues/examples/)
- [Limits](/queues/platform/limits)
- [Behavior differences](/queues/platform/deviations)
