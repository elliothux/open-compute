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

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| `fetch`、`scheduled`、`queue` 的参数和返回值 | 是，见 [Handlers](https://developers.cloudflare.com/workers/runtime-apis/handlers/) | 是 |
| `ctx.waitUntil`、`ctx.passThroughOnException` | 是 | 按 workerd 行为 |
| Email handler / Tail handler | 是 | 不提供 |
| Cron 触发 | 托管 Cron | UTC 五字段；错过触发后最多补宽限时间内最近一次，见 [Cron Triggers](/zh/workers/configuration/cron-triggers) |
| Queue 投递 | 全球队列语义 | 本机投递，可能重复，不是全局先进先出 |
| 项目文件中的 `triggers.crons` / queue consumer 数组 | Wrangler | 不允许；平台部署元数据接受 `crons` 与 `queue_consumers` |

