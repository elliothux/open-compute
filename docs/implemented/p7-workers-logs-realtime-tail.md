# P7：Workers Logs 与 Realtime Tail

状态：**verified / Implementation GO（2026-09-04）**。扩展 hosted、性能和跨平台资格见
[P7 验收](../acceptance/p7-observability-extended-acceptance.md)。

## 用户结果

- 固定 Wrangler `tail` 使用 Script Tails API 和 `trace-v1` WebSocket；Dashboard 使用 Telemetry Live Tail 与 heartbeat。
- Workers Logs 支持 Telemetry `keys`、`values`、`query` 的 `events`／`invocations` 子集。
- 三个入口共用 canonical invocation/event、sampling、redaction、quota 和 metrics，但保持各自官方 wire contract。
- 高写入日志进入独立、有界的 `observability.sqlite`；control 只保存 setting、generation 和 audit，实时 session 只存在于进程内。
- 每个实际执行 target 独立归属日志；nested Service／DO／Workflow／Queue 不错误聚合到 caller tail。
- API token、tail ticket、generation token、secret header、URL credential 和 tenant 内容不进入错误、日志或持久 metadata。
- Retention、quota、慢客户端、restart、corrupt store 和 runtime unavailable 都有明确有界失败行为。

Tail Workers、Streaming Tail Workers、traces、非空 destinations、Logpush 和 saved queries 保持 unsupported。

## 历史验证与限制

Wrangler 4.127.1、Cloudflare SDK 7.1.0、Dashboard live wire、214 个 JS tests、14 个 conformance cases、静态检查均通过。
Coverage Gate 和最终单轮 workspace Gate 均为 49 targets／1,107 cases；Rust line coverage 为
106,499 / 118,313（90.0146%）。

Hosted Script Tail 长尾、nested target attribution differential、参数化性能和跨平台发行仍未完成。
