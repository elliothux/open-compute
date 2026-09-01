# Queues

Queues 把消息从 producer Worker 投递给 consumer Worker。投递是 at-least-once。耐久性来自该节点上的 `scheduler.sqlite`。

例如：

- 解耦 producer 与 consumer Worker
- 缓冲异步处理
- 失败后重试投递

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

在 `open-compute.json` 中绑定 producer。普通产品 binding 为 `{ type, id, permissions? }`：

```json
{
  "name": "queue-app",
  "main": "src/index.ts",
  "bindings": {
    "QUEUE": { "type": "queue_producer", "id": "<queue-id>" }
  }
}
```

Consumer 是 Worker 的 `queue` handler。`open-compute.json` 不使用 Wrangler 的 `[[queues.consumers]]`。绑定语法见 [bindings](/workers/configuration/bindings)。CLI 为 `oc` / `oc run` / `oc types`。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| JavaScript API | [Queues JavaScript APIs](https://developers.cloudflare.com/queues/configuration/javascript-apis/) | 相同：`send` / `sendBatch`、`contentType`（json / text / bytes / v8）、`delaySeconds`、`metrics`、consumer `MessageBatch` / `ack` / `retry` |
| 耐久性 | 全球复制 | 该节点上的 `scheduler.sqlite` |
| 投递 | at-least-once | at-least-once |
| 全球 FIFO | 提供 | 不提供 |
| 未知 native dispatch | — | 可能保留 lease；后续投递可能重复同一 attempt number |
| Pull consumer | 提供 | 不提供 |
| Binding | wrangler `queues` | producer `{ type, id, permissions? }`；consumer 为 Worker `queue` handler |

## 本节

- [上手](/queues/get-started/)
- [概念](/queues/concepts/)
- [指南](/queues/guides/)
- [示例](/queues/examples/)
- [限制](/queues/platform/limits)
- [行为差异](/queues/platform/deviations)
