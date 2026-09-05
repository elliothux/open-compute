# P0.8：Scheduler 与 DO Alarms

> 状态：已实现并验证（2026-08-25）

已完成阶段的维护摘要；当前支持范围见[兼容矩阵](../references/cloudflare-compatibility.md)。

## 实现与不变量

- Object-local alarm 是事实来源，scheduler.sqlite 保存可重建的调度投影。
- Set／delete alarm 与对象事务一致；投影同步必须处理提交、回滚、合并和响应丢失。
- Dispatch 通过租约、token 和 generation fence 拒绝重复／失效 completion，不能靠内存状态确认成功。
- Alarm 遵守 at-least-once；handler reschedule、retry、exhaustion 和取消拥有独立的状态转移。
- Read、activation、扫描与 reconciler 修复投影遗漏，但不得改变 object-local authority。
- 部署切换后按当前有效部署调用 pending alarm；delete/recreate 与 `deleteAll()` 清除或隔离旧代任务。
- 共享 scheduler kernel 管理时钟、唤醒、claim 和 shutdown；Queue、Cron、Workflow 的领域逻辑由各 workload 拥有。

## 源码入口

- [`crates/storage/src/scheduler.rs`](../../crates/storage/src/scheduler.rs)
- [`crates/service/src/scheduler`](../../crates/service/src/scheduler)
- [`packages/runtime/src/durable-objects`](../../packages/runtime/src/durable-objects)

## 验收依据

以下保留原阶段验证记录，命令与轮数不作为当前执行要求：

验证结果：

- `./test/test-p0-8.sh` 已连续三轮 fresh process通过 P0.8 stock-workerd Gate，并递归跑通
  P0.7 至 P0.2 的全部三轮 regression Gate；
- P0.8 Gate 覆盖 constructor/class field/fetch/RPC proxy、number/Date/invalid input、past due、
  overwrite/delete/token fence、async transaction commit/rollback/coalesce、`transactionSync()` fail closed、
  read/activation/scan repair、stale authority/projection、transport unknown lease retention、六次 retry 与
  2/4/8/16/32/64 秒 backoff、exhaustion、A -> B -> rollback A 和 KV/SQL/FK/alarm `deleteAll()`；
- `./poc/g0 test all` 三轮 aggregate verdict 为 `Conditional Go`，唯一条件仍是既有精确 allowlist
  `loader:D-abort`；
- workspace format、Clippy、unit/integration、no-default-features、Rust 1.98 MSRV、metadata、dependency
  boundary、diff whitespace 和 coverage 均通过；Rust line coverage 为 90.01%。

当前测试入口与规则见[测试手册](../references/testing.md)。
