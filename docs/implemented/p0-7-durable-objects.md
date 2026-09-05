# P0.7：Durable Objects

> 状态：已实现并验证（2026-08-25）

已完成阶段的维护摘要；当前支持范围见[兼容矩阵](../references/cloudflare-compatibility.md)。

## 实现与不变量

- Namespace／object 身份与 lifecycle 属于平台 authority；用户对象执行和 storage 使用 workerd 原生 facet。
- Public object ID 的生成与解析在本地同步完成，路由只接受已验证 namespace、account 与 object identity。
- DoRouter／DoHost 维护 dispatch 和 generation fence；同一对象遵守串行语义，不同对象可以并行。
- 部署切换、回滚及 delete／recreate 不得复用失效 generation 或把旧存储暴露给新对象。
- Native storage、事务和 output gate 不能被异步 RPC facade 改写语义。
- 进程重启从持久 authority 和原生 storage 恢复；损坏、未知结果和 stale projection 均显式处理。
- 当前 WebSocket hibernation 能力见兼容矩阵；阶段 P1.8 的 No-Go 仅是历史调查结论。

## 源码入口

- [`crates/storage/src/durable_objects.rs`](../../crates/storage/src/durable_objects.rs)
- [`crates/workers/src/durable_objects.rs`](../../crates/workers/src/durable_objects.rs)
- [`packages/runtime/src/durable-objects`](../../packages/runtime/src/durable-objects)

## 验收依据

以下保留原阶段验证记录，命令与轮数不作为当前执行要求：

- migration 007 保存 namespace/object lifecycle authority，tenant bytes 仍只由 native facet SQLite 持有；
- production `DoRouter`/`DoHost`、单一 loaded-isolate wrapper、同步 ID codec 和 namespace facade 已进入
  static workerd config；
- control API、delete/recreate generation fence、startup reconciliation、storage marker、health/metrics 和
  runtime composition 已接通；
- `./test/test-p0-7.sh` 已连续三轮 fresh process 验证 P0.7，并递归跑通 P0.6 至 P0.2；
- `./poc/g0 test all` 的三轮 aggregate verdict 为 `Conditional Go`，唯一条件仍是既有、精确 allowlist
  `loader:D-abort`；
- workspace format、Clippy、unit/integration、no-default-features、Rust 1.98 MSRV、metadata、dependency
  boundary 和 coverage 均通过；Rust line coverage 为 90.03%。

P0.7 Gate 覆盖 public ID/HMAC 与 intrinsic tamper、fetch/RPC/binary、SQLite/KV/transaction、
`deleteAll()`、`blockConcurrencyWhile()`、`waitUntil()`、同 object ordering、跨 object overlap、
WebSocket text/binary、class validation、in-flight promotion、A -> B -> A rollback、stale generation、
restart、delete/recreate 和 Worker tombstone 后显式 purge。`localDisk` 仍是 pinned workerd 的
experimental config；alarms 和 WebSocket hibernation 仍属于明确非目标。

当前测试入口与规则见[测试手册](../references/testing.md)。
