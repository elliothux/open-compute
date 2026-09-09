# P2.4：Workflow Core

状态：**implemented / Conditional Go（2026-08-28）**。

## 最终结果

- Workflow definition、不可变 version、instance、step 和 deployment ref 由持久 authority 管理。
- Scheduler claim、run 和 step completion 使用 lease、attempt 与 generation fence；租户不能提供内部执行身份。
- 已提交 step result 在 replay 中复用，未提交 attempt 可以重跑；外部副作用不承诺 exactly-once。
- Claim、异步执行和 commit 不跨数据库事务；terminal、引用释放和删除可在重启后收敛。
- JSON payload、step 数、history、并发和 deadline 有界；snapshot/restore 保持版本、状态和 replay identity。
- 普通 Worker 可创建和控制 Workflow；Durable Object 内 mutation 因 output-gate 限制 fail closed。

当前支持面与 deviation 见[兼容矩阵](../references/cloudflare-compatibility.md)。

## 历史验证

本地 Hard／Product、crash matrix、P2 aggregate、workspace 静态检查和 coverage 全部成功；当时 coverage 为 90.16%。
验证覆盖 committed-result replay、Unknown response、版本切换、十个 SIGKILL 持久边界和 fresh-host restore。

接受限制：外部副作用需业务幂等；DO mutation 不支持；不提供跨产品 exactly-once 事务。
