# 指南

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

单条最大 128 KiB，batch 最多 100 条 / 256 KiB 合计，delay 最大 86400 秒。无效 content type 与空 batch 是 `TypeError`；超大 batch / 非法 delay 是 `Error`。见 [Queues JavaScript APIs](https://developers.cloudflare.com/queues/configuration/javascript-apis/)。

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

Handler 成功且无显式 decision → ack；失败且无显式 decision → retry。`open-compute.json` 不使用 Wrangler consumers 数组。
