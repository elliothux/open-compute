# Cron Triggers

Cron 在 open-compute 上按 UTC 表达式触发 Worker 的 `scheduled()`。这不是 Cloudflare 控制台中的触发器界面。

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

本机已文档化的 Quartz-like 扩展：`*` `,` `-` `/` `L` `W` `#`，以及大小写不敏感的三字母月份/星期名。weekday 数字按 Cloudflare fixture：`1=Sunday` … `7=Saturday`。

平台部署元数据字段是 `crons: string[]`。`open-compute.json` 没有 Wrangler `triggers` / `triggers.crons`；写入该键将作为未知字段被拒绝。Workflow 的 cron 走 binding 上的 `schedules`，不是 Worker `scheduled()`。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| `scheduled()` handler | 是，见 [scheduled()](https://developers.cloudflare.com/workers/runtime-apis/handlers/scheduled/) | 是 |
| 五字段 cron | 是，见 [Cloudflare Cron Triggers](https://developers.cloudflare.com/workers/configuration/cron-triggers/) | 是；仅 UTC |
| `noRetry()` | 是 | 是 |
| 项目文件中的 `triggers.crons` | Wrangler | 不允许；部署元数据字段为 `crons: string[]` |
| 错过触发后的恢复 | 托管调度语义 | 宽限时间内最多补最近一次，不回放停机期间的全部触发 |
| 已知失败重试 | 托管策略 | 按配置有限次重试；调用 `noRetry()` 除外 |
| 错过触发的默认宽限 | 套餐相关 | `scheduler.cron_misfire_grace_ms = 300000`（五分钟）；精确值以 `ocd capabilities --json` 的 `limits` 为准 |

