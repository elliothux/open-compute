# Handlers

模块 Worker 导出对象上的方法。

```ts
export default {
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    return new Response("ok");
  },
  async scheduled(controller: ScheduledController, env: Env, ctx: ExecutionContext): Promise<void> {},
  async queue(batch: MessageBatch, env: Env, ctx: ExecutionContext): Promise<void> {},
} satisfies ExportedHandler<Env>;
```

Durable Object 的 `alarm()` 在 DO 类上，不是 default export。见 [DO Alarms](https://developers.cloudflare.com/durable-objects/api/alarms/)。

## 与 Cloudflare 相同

`fetch`、`scheduled`、`queue` 的参数和返回值与 [Handlers](https://developers.cloudflare.com/workers/runtime-apis/handlers/) 相同。`ctx.waitUntil`、`ctx.passThroughOnException` 按 workerd 行为。没有 Email handler、没有 Tail handler 产品。

## 故意不同

Cron 触发语义见 [`OC-CRON-001`](/workers/configuration/cron-triggers)。Queue 投递是单节点 at-least-once（`OC-QUEUE-001`），不是全球 FIFO。`open-compute.json` 目前没有 `triggers.crons` 或 queue consumer 数组；未知字段会失败。平台部署元数据接受 `crons` 与 `queue_consumers`。
