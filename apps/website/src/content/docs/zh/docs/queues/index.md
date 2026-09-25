---
title: "Queues"
---

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

在 `wrangler.jsonc` 中绑定生产者：

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

消费者通过 `queues.consumers` 指向 Worker 的 `queue` handler。语法见[绑定](/zh/docs/workers/configuration/bindings/)。固定 Wrangler 负责 queue provisioning 与 consumer 配置。

管理 SDK 也暴露 Cloudflare 的官方 producer 路径：

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

这些调用与 Worker `send()` / `sendBatch()` 写入同一 durable Queue authority。enqueue 后超时属于 result-unknown，应用可能需要自行去重。管理面 `pull`、`ack`、`peek` 与 `purge` 仍不支持，因为 open-compute 没有 HTTP-pull lease 协议。

## 兼容性

| 主题                       | Cloudflare                                                                                        | open-compute                                                                                                                              |
| -------------------------- | ------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| JavaScript API             | [Queues JavaScript APIs](https://developers.cloudflare.com/queues/configuration/javascript-apis/) | 相同：`send` / `sendBatch`、`contentType`（json / text / bytes / v8）、`delaySeconds`、`metrics`、消费者 `MessageBatch` / `ack` / `retry` |
| 存储位置                   | 全球复制                                                                                          | 本机 `scheduler.sqlite`                                                                                                                   |
| 投递语义                   | at-least-once                                                                                     | at-least-once；不提供 exactly-once                                                                                                        |
| 全局 FIFO                  | 提供                                                                                              | 不提供                                                                                                                                    |
| 无法识别的 native dispatch | —                                                                                                 | 可能保留消息 lease，后续投递可能使用同一 attempt 编号                                                                                     |
| Pull consumer              | 提供                                                                                              | 不提供                                                                                                                                    |
| 绑定                       | Wrangler `queues`                                                                                 | 标准 `producers` 与 `consumers` 条目                                                                                                      |

下一步：[使用 bindings 开发](/zh/docs/develop/) · [兼容性与限制](/zh/docs/reference/)
