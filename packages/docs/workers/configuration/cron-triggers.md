# Cron Triggers

Cron 在本平台上按 UTC 表达式触发 Worker 的 `scheduled()`。这不是 Cloudflare 控制台中的触发器界面。

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

本节点已文档化的 Quartz-like 扩展：`*` `,` `-` `/` `L` `W` `#`，以及大小写不敏感的三字母月份/星期名。weekday 数字按 Cloudflare fixture：`1=Sunday` … `7=Saturday`。

平台部署元数据字段是 `crons: string[]`。`open-compute.json` 没有 Wrangler `triggers` / `triggers.crons`；写入该键会作为未知字段被拒绝。Workflow 的 cron 走 binding 上的 `schedules`，不是 Worker `scheduled()`。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| `scheduled()` handler | 是，见 [scheduled()](https://developers.cloudflare.com/workers/runtime-apis/handlers/scheduled/) | 是 |
| 五字段 cron | 是，见 [Cloudflare Cron Triggers](https://developers.cloudflare.com/workers/configuration/cron-triggers/) | 是；仅 UTC |
| `noRetry()` | 是 | 是 |
| 项目文件中的 `triggers.crons` | Wrangler | 不允许；部署元数据字段为 `crons: string[]` |
| misfire 恢复 | 托管调度语义 | 最多投影 grace 内最新的一个 slot，不重放完整停机历史 |
| 已知失败重试 | 托管策略 | 配置中的有界本节点重试，除非调用了 `noRetry()` |
| 默认 misfire grace | 套餐相关 | `scheduler.cron_misfire_grace_ms = 300000`（五分钟）；精确值以 `ocd capabilities --json` 的 `limits` 为准 |

