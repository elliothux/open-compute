# G0：workerd 可行性调查

调查已结束，结论为 **Conditional Go**。固定输入、逐用例结果、实际命令与限制完整保留在 [G0 原始报告](g0-results.md)。

## 保留结论

- 已验证 bootstrap、dynamic Worker 装载、binding、原生 Durable Object/facet、存储与进程恢复路径。
- `D-abort` 是明确接受的限制：客户端断开不能作为保证取消 Worker 执行的原语。
- 结果只证明报告中的 runtime pin 与测试输入；后续 runtime 升级必须重新验证受到影响的产品合同。
- POC 已退役；存续断言和产品回归的归属见 [Runtime 与测试布局](runtime-and-test-layout.md#poc-删除与断言归属)。

当前架构见[平台总览](open-compute-workerd-platform.md)，测试要求见[测试手册](../references/testing.md)。
