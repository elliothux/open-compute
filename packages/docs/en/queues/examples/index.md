# Examples

The producer uses `send` on a `queue_producer` binding; the consumer is the `queue` handler on the same Worker. Put the platform `queue.id` in the binding, then `oc types` / `oc run`.

```ts
export default {
  async fetch(_request: Request, env: Env): Promise<Response> {
    await env.QUEUE.send({ hello: "world" });
    await env.QUEUE.sendBatch([{ body: { hello: "batch" } }]);
    return new Response("queued");
  },
  async queue(batch: MessageBatch<{ hello: string }>): Promise<void> {
    for (const message of batch.messages) {
      console.log(message.body);
      message.ack();
    }
  },
} satisfies ExportedHandler<{ QUEUE: Queue }>;
```

```json
{
  "name": "queue-app",
  "main": "src/index.ts",
  "bindings": {
    "QUEUE": { "type": "queue_producer", "id": "<queue.id>" }
  }
}
```

## Same as Cloudflare

`send` / `sendBatch`, `contentType`, `delaySeconds`, `ack` / `retry` match the [Queues JavaScript APIs](https://developers.cloudflare.com/queues/configuration/javascript-apis/). Do not write Wrangler `[[queues.consumers]]`.

## Intentional differences: OC-QUEUE-001

Durability is single-node `scheduler.sqlite`. Delivery is at-least-once; there is no global FIFO or exactly-once. Create the queue: [Get started](/en/queues/get-started/).
