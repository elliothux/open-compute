# P2.3 Queue Consumer / Cron Gate 结果

> 结论：Go
>
> 验证日期：2026-08-27（Asia/Shanghai）
>
> 基线 revision：`fd3362036c5e8e60cb9d5ad4ee2c55c6b7a8f542`，加当前 P2.3 worktree。

## 固定运行时与可复现输入

- workerd release：`v1.20260826.1`；版本输出：`workerd 2026-08-26`；host compatibility date：`2026-08-22`。
- `runtime/workerd.lock.json` SHA-256：`d3614a6394cf85e24704954d9e7a9585fb38a2107e8eb73a02519e39add14d2e`。
- 本机 `darwin-arm64` workerd SHA-256：`2d17da54d2671d6e9e7c776d56b934f60be8c140b9bac35ddf22f60d6cff9403`，与 lock 相同。
- 生产 Cap'n Proto 配置源 `runtime/config.capnp` SHA-256：`8086d0963ebc24becdd07af600bded28c216c396923ab3106d04ae1ebd50c380`。
- dynamic loader `runtime/system-workers/loader-host.js` SHA-256：`72a6c69ce7e799ba0a9e13e0d98dd16480652dc100f5da8dcc618b2fc4d6afe2`。
- isolate wrapper generator SHA-256：`ca5fdd28cf500239102d2e4f83d4fae9ac96080a1bf199a900c17170389c1e08`。
- Queue/Cron wire fixture 与真实进程 Gate 位于 `crates/service/tests/p0_2_runtime_gate.rs`；记录时 SHA-256 为 `e014caea7da51b94abedd6234b8d9ea408bd3c621c80383de8ec64bf6240ced1`。

每轮由生产 `RuntimeConfigCompiler` 从上述 Cap'n Proto、system workers、immutable RuntimeSource 和 loader key 编译临时二进制配置；临时目录仅用于该轮 fresh process，不作为另一套产品配置保留。请求/响应 DTO、边界值和期望 disposition 固化在上述 Rust Gate 与 checked-in loader 中。supervisor 对 stderr 做 bounded capture；三轮均没有非预期或含 secret 的 stderr。

## Hard Gate 观察

Queue custom event：

- 默认 module export 和 named `WorkerEntrypoint` 都经 native `queue()` dispatch 成功；warm/cold loader 及 supervisor restart 后结果一致。
- text、JSON、bytes body、`Date` timestamp 和一基 attempts 通过。
- `ack()`、`retry({delaySeconds})`、batch decision、单消息优先和 first-call-wins 通过；host 拒绝额外/重复 message ID、越界 delay、越界 message count/body/aggregate payload。
- handler throw 与 rejected `waitUntil()` 为 Known failure；真实 10 秒 handler 在 100ms host deadline 下为 Unknown，claim 不完成，等待 lease recovery。
- batch ID、consumer generation、claim token 和 execution generation 只存在于 host authority；tenant response 不能提供或覆盖这些字段。

Scheduled custom event：

- 默认 export 经 native `scheduled()` dispatch 成功；`controller.type === "scheduled"`、精确 cron 字符串、logical `scheduledTime`、env 和 `noRetry()` 通过。
- handler throw 与 rejected `waitUntil()` 为 Known failure；transport timeout/abort 保持 Unknown。
- warm/cold loader、restart 后 Queue 与 Scheduled custom event 均重新通过。

## 执行记录

复现命令：

```sh
OPEN_COMPUTE_TEST_WORKERD="$PWD/poc/.runtime-cache/v1.20260826.1/workerd" \
  ./scripts/test-p2-3.sh
```

结果：3/3 fresh-process rounds 通过；每轮启动并停止独立的 pinned stock workerd。随后通过 Cron parser、Queue claim/completion/DLQ、scheduler migrations 003/004、promotion/reconcile/idempotency 测试以及 production binary scenario-marker 检查。

最终验证同时通过：workspace 全 targets/features 单线程测试、`cargo fmt --all --check`、Clippy `-D warnings`、无默认 feature 检查、Rust 1.98 MSRV 检查、workspace metadata、依赖边界和 `git diff --check`。`./scripts/coverage.sh` 的 workspace Rust line coverage 为 **90.03%**（门槛 90.00%）；报告位于 `target/llvm-cov/`。

既有 G0 三轮回归也通过 aggregate acceptance；唯一失败仍是精确 allowlist `loader:D-abort`（每轮 `abortEvents 0 -> 0`），最终 verdict 为 `Conditional Go`。P2.3 没有扩大该 allowlist，也没有把 Unknown outcome 误判成可安全立即重试。
