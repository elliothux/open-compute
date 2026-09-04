# P7 observability 扩展差分与发行验收

状态：active；只追踪 hosted differential、性能与跨平台发行资格，不阻塞已归档的 P7 Day 1 支持面

日期：2026-09-04

[`P7 Workers Logs 与 realtime tail`](../implemented/p7-workers-logs-realtime-tail.md) 已实现固定 Wrangler 4.127.1 Script
Tails、官方 Cloudflare SDK 7.1.0 Telemetry、真实 Dashboard Live Tail wire、独立有界日志存储、权限/审计、重启恢复
与明确 unsupported 边界。本文只追踪没有被核心 capability 声明覆盖的扩展资格；未完成项不得被解释为相应能力已支持。

仓库级验收已经关闭：canonical Clippy 通过，完整 coverage Gate 为 49/49 targets、1,107/1,107 cases，production Rust
行覆盖率为 106,499 / 118,313（90.0146%），最终非插桩 workspace Gate 为 49/49 targets、1,107/1,107 cases。以下仅为
剩余资格：

- 在唯一临时 Cloudflare Worker 上补齐 Script Tail 的 hosted 精确 TTL、delete-not-found、`debug=true`、expiry、overload
  与 slow-consumer close code，并确认 GET List Tails 在多 session 下的实际 result shape；完成后按精确名称删除并复查
  absent，不读取或修改账号既有服务。
- 对 Service Binding、Durable Object、Workflow、Queue 与 scheduled lifecycle 做 hosted root/target attribution
  differential。当前本地 topology 以 pinned stock workerd probe 为 authority：每个 target 独立归属，caller/root tail
  不聚合 nested target 日志；该差异登记为 `OC-OBSERVABILITY-001`。
- 补齐 Telemetry query 的 omitted/null、type mismatch、retention-race 与全部 unsupported view 的 hosted error differential；
  只扩大已经取得固定 fixture、实现和回归覆盖的字段。
- 建立参数化 synthetic benchmark：persistence disabled、典型/最大 invocation、10 个实时客户端含 1 个 slow consumer、
  2,000-event query、retention cleanup 和 database quota 水位；记录 p50/p95/p99 与 control API 影响，不把本机数值写成
  Cloudflare plan limit。
- 在正式发行矩阵验证 Linux/macOS 的受支持架构、single-binary offline startup 和 stock-workerd pin；特权 egress fixture、
  release packaging、发布、签名或账号 mutation 仍分别需要明确授权。

## 完成条件

- 所有 hosted fixture 脱敏，不含 account ID、token、tail ticket、URL credential、tenant secret 或既有资源名称；
- 新证据只更新现有 `workersObservability` authority 与 `OC-OBSERVABILITY-001`，不建立第二份 capability manifest；
- 新增支持面具备官方来源、固定版本/hash、真实客户端或 Dashboard wire、成功/失败回归和 fail-closed unsupported 行为；
- benchmark 与跨平台结果记录实际 revision、pin、命令、平台和限制，失败或未运行不得写成 PASS。
