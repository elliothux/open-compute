# P7：Workers Logs 与 Realtime Tail

状态：**verified / Implementation GO（2026-09-04）**。

## 用户结果

- 当前用户通过 Dashboard Telemetry Live Tail 与 heartbeat 查看实时日志；[P20](p20-cf-cli-migration.md) 的 cf
  尚无等价终端流式 tail 入口。Script Tails API 与 `trace-v1` WebSocket 是服务端协议，其旧客户端 fixture 保留为历史 wire 证据。
- Workers Logs 支持 Telemetry `keys`、`values`、`query` 的 `events`／`invocations` 子集。
- Script Tails、SDK Telemetry 和 Dashboard 三个协议入口共用 canonical invocation/event、sampling、redaction、quota 和 metrics，但保持各自官方 wire contract。
- 高写入日志进入独立、有界的 `observability.sqlite`；control 只保存 setting、generation 和 audit，实时 session 只存在于进程内。
- 每个实际执行 target 独立归属日志；nested Service／DO／Workflow／Queue 不错误聚合到 caller tail。
- API token、tail ticket、generation token、secret header、URL credential 和 tenant 内容不进入错误、日志或持久 metadata。
- Retention、quota、慢客户端、restart、corrupt store 和 runtime unavailable 都有明确有界失败行为。

Tail Workers、Streaming Tail Workers、traces、非空 destinations、Logpush 和 saved queries 保持 unsupported。

## 历史验证与限制

Wrangler 4.127.1、Cloudflare SDK 7.1.0、Dashboard live wire、214 个 JS tests、14 个 conformance cases、静态检查均通过。
Coverage Gate 和最终单轮 workspace Gate 均为 49 targets／1,107 cases；Rust line coverage 为
106,499 / 118,313（90.0146%）。

Hosted nested-target attribution parity 与多 Tail list 的精确 wire shape 不在当前声明范围；实时 session
仍是 process-local、无 replay 的 best-effort 能力。
