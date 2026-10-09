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

## #146：活跃对象与 Loader 容量

原实现为每个激活的 tenant/facet class 占用共享 WorkerLoader 的 named cache；正式 pin 的同一
namespace 只有 64 个槽，活跃对象持有 class 时无法淘汰。私有 facet manager 又直接保留所属
DoHost 的 actor stub，形成回引用，阻止 native idle eviction。因此 alarm repair 或保持 hibernatable
WebSocket 的对象可以耗尽普通 Worker dispatch 和版本验证共用的 cache，而 process liveness 仍正常。

DoHost 现在通过 `LOADER.get(null, ...)` 创建 class，只在当前 host activation 内复用；generation
切换、facet abort/delete/clone 覆盖和对象删除会释放对应 class。私有 `FacetManager` 只保留 host ID，
每次调用重新取得 stub；对象数据仍属于原来的 native SQLite，没有迁移、数据重建或 workerd pin 更新。

冷启动的 RPC、fetch 和 CONNECT 使用实际 handler admission 确认保持同一 stub 的调用开始顺序，
确认后允许未完成请求继续重叠。fetch 继续走 native HTTP/WebSocket 通道；私有 token 在 tenant
handler 前移除，跨 RPC 保留的 callback 显式 `dup()`，使用、取消或超时后释放。

正式 pin `v1.20260930.0-open-compute-r4.e98a3e843` 的单轮 `p0-7` 已通过两个 case：原有完整矩阵，
以及真实 daemon/control API 上的 129 个冷启动对象、混合 RPC/fetch 顺序、129 条同时保持的
hibernatable WebSocket、邻接普通 Worker 的新版本验证/部署/调用、alarm 和 daemon 重启后的
SQLite/ID 恢复。runtime JS 测试通过 424 个 case。专项报告为
`.temp/gate-run/20261008T214716-b552b442/report.json`；完整 workspace 验收另行记录。

组件调查使用真实 system Workers/native SQLite，但 RuntimeSource/authority 是 fixture，不替代
daemon 验收。当前实现的 129 次 alarm repair 均成功，普通 dispatch/validation 保持 200/204；
静默 145 秒后再 repair 旧对象，持久 constructor 记录由 129 变为 130，证明 native idle eviction
可以释放 host activation，并保留原 SQLite 数据（`.temp/issue146-fix/idle-release-final.log`）。
该 fixture 的 public-fetch 调查在旧实现和新实现均出现 64 个对象之前的
`DO_RUNTIME_EXCEPTION`，未作为修复通过证据。尚未运行真实 Cloudflare differential 或 Linux
发行资格测试；129 个对象是本次回归规模，不是新的配额或无限容量承诺。
