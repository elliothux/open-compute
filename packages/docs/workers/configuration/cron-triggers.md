# Cron Triggers

Cron 产品在这里：Worker 的 `scheduled()` 按 UTC 表达式触发。不是 Cloudflare dashboard 里的触发器 UI。

```ts
export default {
  async scheduled(controller: ScheduledController, env: Env, ctx: ExecutionContext): Promise<void> {
    if (shouldGiveUp()) controller.noRetry();
  },
} satisfies ExportedHandler<Env>;
```

`controller.cron` 是部署时声明的精确字符串。`controller.scheduledTime` 是该 logical slot 的时间。

## 表达式

只接受**五个 UTC 字段**：minute、hour、day-of-month、month、day-of-week。没有秒字段、没有 year、没有本地时区或 DST。

本机文档化的 Quartz-like 扩展：`*` `,` `-` `/` `L` `W` `#`，以及大小写不敏感的三字母月份/星期名。weekday 数字按 Cloudflare fixture：`1=Sunday` … `7=Saturday`。

平台部署元数据字段是 `crons: string[]`。`open-compute.json` 没有 Wrangler `triggers` / `triggers.crons`；写进去会当未知字段拒绝。Workflow 的 cron 走 binding 上的 `schedules`，不是 Worker `scheduled()`。

## 与 Cloudflare 相同

handler 与 [scheduled()](https://developers.cloudflare.com/workers/runtime-apis/handlers/scheduled/) 相同。五字段 cron 与 [Cloudflare Cron Triggers](https://developers.cloudflare.com/workers/configuration/cron-triggers/) 常用表面一致。`noRetry()` 可用。

## 故意不同：OC-CRON-001

Cron 只有 UTC、五个字段，以及已文档化的本机 Quartz-like 扩展。恢复时最多投影 misfire grace 内最新的一个 slot，不会重放完整停机历史。已知失败走配置里的有界本机重试，除非调用了 `noRetry()`。

默认 `scheduler.cron_misfire_grace_ms = 300000`（五分钟）。精确值以 `ocd capabilities --json` 的 `limits` 为准。停机五小时再起来，不会补跑五小时里的每一分钟。
