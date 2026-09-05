# P2.4：Workflow Core

> 状态：已实现并完成最终验收；P2.4 为 Conditional Go（DO 内 create 按 output-gate 结论 fail closed）。最终结论与证据见 [P2.4 Gate 结果](./p2-4-gate-results.md)。

已完成阶段的维护摘要；当前支持范围见[兼容矩阵](../references/cloudflare-compatibility.md)。

## 实现与不变量

- Definition、不可变 version、instance 和 deployment 引用由持久化 authority 管理。
- Workflow scheduler claim 与 trusted dispatcher 通过 lease／generation 验证，租户不能自行指定内部执行身份。
- Step identity 在 replay 中保持稳定；已提交结果被复用，未提交 attempt 可重新执行。外部副作用不因此成为 exactly-once。
- Step claim／execute／commit 分离事务与异步 I/O；completion 只接受当前 attempt 的 fence。
- 结果 codec、payload、并发和持久空间有界；过大或非法输入在对应 authority 边界失败。
- Terminal completion 与资源引用释放可以跨重启收敛，删除不能留下仍能继续执行的旧 generation。
- 跨库 reconciler、snapshot、重启和未知结果保持 instance／step 的持久状态一致。

## 源码入口

- [`crates/storage/src/workflows.rs`](../../crates/storage/src/workflows.rs)
- [`crates/workers/src/workflows.rs`](../../crates/workers/src/workflows.rs)
- [`crates/workers/src/workflow_lifecycle.rs`](../../crates/workers/src/workflow_lifecycle.rs)
- [`packages/runtime/src/workflows`](../../packages/runtime/src/workflows)

## 验收依据

历史结论、实际命令与未验证项见[验收记录](p2-4-gate-results.md)。

当前测试入口与规则见[测试手册](../references/testing.md)。
