# Guides

## Producer

```ts
await env.QUEUE.send({ hello: "world" }, { delaySeconds: 5 });
await env.QUEUE.sendBatch(
  [
    { body: { n: 1 } },
    { body: new TextEncoder().encode("raw"), contentType: "bytes" },
  ],
  { delaySeconds: 0 },
);
const m = await env.QUEUE.metrics();
```

Max 128 KiB per message, 100 messages / 256 KiB per batch, delay at most 86400 seconds. Invalid content type and empty batch are `TypeError`; oversized batch / illegal delay are `Error`. See [Queues JavaScript APIs](https://developers.cloudflare.com/queues/configuration/javascript-apis/).

## Consumer

```ts
export default {
  async queue(batch: MessageBatch): Promise<void> {
    for (const message of batch.messages) {
      try {
        // ...
        message.ack();
      } catch {
        message.retry({ delaySeconds: 10 });
      }
    }
  },
} satisfies ExportedHandler;
```

Handler success with no explicit decision → ack; failure with no explicit decision → retry. `open-compute.json` does not use a Wrangler consumers array.
