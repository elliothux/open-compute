---
title: "日志与实时 Tail"
description: "在选中的 open-compute instance 上持久化、查询并实时查看 Worker 日志。"
---

open-compute 在本机 instance 上实现 Cloudflare Workers observability 设置、telemetry query route，以及选定 script 的 live tail。`ocd wrangler tail` 使用相同的 Cloudflare-compatible `/client/v4` surface。

```sh
ocd wrangler tail --env staging
```

## 配置日志

使用 Wrangler 的 `observability` 配置。支持日志启用、sampling、invocation log 与持久化；不支持外部 log destination 与 trace。

```json
{
  "observability": {
    "enabled": true,
    "head_sampling_rate": 1,
    "logs": { "enabled": true, "invocation_logs": true, "persist": true },
    "traces": { "enabled": false }
  }
}
```

持久化 telemetry 支持 `events` 与 `invocations` query view、key/value discovery、filter，以及限定到选定 script 的 live-tail session。retention、database size、invocation log size、query timeframe 与 event count、ingest queue capacity、tail session 数与 tail client buffer 均受 instance 配置限制。

## 本机差异

Telemetry 存在选中的 instance，而不是 Cloudflare 全球 analytics service。hosted-only metadata 会省略，regex filter 使用有界 RE2-compatible subset，且不提供 account-wide live tail。Calculations、traces、agents、requests、saved queries、Tail Workers、Logpush 与 external destination 均不支持。

参见[行为差异](/zh/docs/platform/deviations/)、[限制](/zh/docs/platform/limits/)与[管理 SDK](/zh/docs/platform/reference/sdk/)。
