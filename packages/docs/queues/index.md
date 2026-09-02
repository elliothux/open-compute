# Queues

Queues 将消息从生产者 Worker 投递给消费者 Worker。投递语义为 at-least-once：在崩溃或重试时，消息可能被重复处理。队列状态存储在本机 `scheduler.sqlite`。

例如：

- 解耦生产者与消费者 Worker
- 缓冲异步任务
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

在 `open-compute.json` 中绑定生产者：

```json
{
  "name": "queue-app",
  "main": "src/index.ts",
  "bindings": {
    "QUEUE": { "type": "queue_producer", "id": "<queue-id>" }
  }
}
```

消费者为 Worker 的 `queue` 处理函数。`open-compute.json` 不使用 Wrangler 的 `[[queues.consumers]]`。语法见 [绑定](/workers/configuration/bindings)。CLI：`oc` / `oc run` / `oc types`。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| JavaScript API | [Queues JavaScript APIs](https://developers.cloudflare.com/queues/configuration/javascript-apis/) | 相同：`send` / `sendBatch`、`contentType`（json / text / bytes / v8）、`delaySeconds`、`metrics`、消费者 `MessageBatch` / `ack` / `retry` |
| 存储位置 | 全球复制 | 本机 `scheduler.sqlite` |
| 投递语义 | at-least-once | at-least-once；不提供 exactly-once |
| 全局 FIFO | 提供 | 不提供 |
| 无法识别的 native dispatch | — | 可能保留消息 lease，后续投递可能使用同一 attempt 编号 |
| Pull consumer | 提供 | 不提供 |
| 绑定 | wrangler `queues` | 生产者 `{ type, id, permissions? }`；消费者为 Worker `queue` 处理函数 |

## 本节

- [上手](/queues/get-started/)
- [概念](/queues/concepts/)
- [指南](/queues/guides/)
- [示例](/queues/examples/)
- [限制](/queues/platform/limits)
- [行为差异](/queues/platform/deviations)
