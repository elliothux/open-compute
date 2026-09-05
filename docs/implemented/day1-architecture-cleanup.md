# Day1 架构清理

清理已完成；原清单 1–17、追加修复和 artifact GC 的实际验收见[完成记录](day1-architecture-cleanup-results.md)。

## 保留的实现边界

- 配置、schema、协议、ID、snapshot 和 artifact 使用当前唯一模型，不保留历史开发格式的兼容读写、回填或回退。
- Runtime pin、编译资产和发布物只有一份 authority；生成资产从源码构建，生产启动离线。
- 持久数据损坏、身份不匹配和未知提交结果必须显式失败；Day1 不授权重置数据或静默修复。
- 模块所有权沿 crate／runtime domain 划分，移除仅服务旧模型的 wrapper、参数、分支和测试。
- POC 与重复 Gate 已收敛；仍需保留的产品不变量归属见[布局记录](runtime-and-test-layout.md#poc-删除与断言归属)。
- Artifact GC 基于持久引用和受控 key scope，保留并发、取消、崩溃恢复与完整性约束。

当前规则统一维护在 [AGENTS.md](../../AGENTS.md)，当前测试执行方式见[测试手册](../references/testing.md)。
原报告中的轮数、性能和 PASS 只记录对应历史执行，不构成重复执行要求。
