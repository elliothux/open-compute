# 示例

Producer 用 `queue_producer` binding 的 `send`；consumer 是同一 Worker 上的 `queue` handler。把平台返回的 `queue.id` 填进 binding，然后 `oc types` / `oc run`。

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

`send` / `sendBatch`、`contentType`、`delaySeconds`、`ack` / `retry` 与 [Queues JavaScript APIs](https://developers.cloudflare.com/queues/configuration/javascript-apis/) 对齐。耐久性来自本机 `scheduler.sqlite`。消息可能被处理多次。不提供全局先进先出 或 exactly-once。创建 Queue 见[上手](/zh/queues/get-started/)。
