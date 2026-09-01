# Queues

Queues 把消息从 producer Worker 投递给 consumer Worker。投递是 at-least-once。耐久性来自单节点 `scheduler.sqlite`，不是 Cloudflare 全球复制。没有全球 FIFO。

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

## 与 Cloudflare 相同

Producer / consumer JavaScript API 与 [Queues JavaScript APIs](https://developers.cloudflare.com/queues/configuration/javascript-apis/) 相同：`send` / `sendBatch`、`contentType`（json / text / bytes / v8）、`delaySeconds`、`metrics`、consumer `MessageBatch` / `ack` / `retry`。63 个目标成员为 `supported_with_deviation`。

```json
{
  "name": "queue-app",
  "main": "src/index.ts",
  "bindings": {
    "QUEUE": { "type": "queue_producer", "id": "<queue-id>" }
  }
}
```

Producer 是普通产品 binding `{type, id, permissions?}`。Consumer 是 Worker 的 `queue` handler（与 Cloudflare 相同）。不要写 Wrangler 的 `[[queues.consumers]]`。绑定语法见 [bindings](/workers/configuration/bindings)。

## 故意不同

**`OC-QUEUE-001`**：Queue producer 和 push consumer 的耐久性来自单节点 `scheduler.sqlite`，不是 Cloudflare 全球复制。投递是 at-least-once，没有全球 FIFO。未知的 native dispatch 会保留 lease，不消耗租户重试预算，所以后续投递可能重复同一 attempt number。

全文见 [偏差](/queues/platform/deviations) 和 [Compatibility](/platform/compatibility)。

## 本节

- [上手](/queues/get-started/)
- [概念](/queues/concepts/)
- [指南](/queues/guides/)
- [示例](/queues/examples/)
- [限制](/queues/platform/limits)
- [偏差](/queues/platform/deviations)
