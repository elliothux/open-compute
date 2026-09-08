# P2.8：Day 1 架构清理

状态：**verified（2026-08-29）**。

## 最终结果

- 配置、schema、协议、ID、snapshot 和 artifact 只保留当前模型，不读取或回填历史开发格式。
- Runtime pin、生成资产和发行物各有一份 authority；生产启动离线。
- 持久数据损坏、身份不匹配和未知提交结果显式失败，不用重置或静默修复掩盖问题。
- 模块按 crate／runtime domain 持有职责；旧 wrapper、参数、分支、POC 和重复 Gate 已删除。
- Artifact GC 基于持久引用和受控 key scope，保留并发、取消、崩溃恢复和完整性边界。

当前 Day 1 规则以 [AGENTS.md](../../AGENTS.md) 为准，当前测试方式以[测试手册](../references/testing.md)为准。

## 历史验证

基线提交为 `f6a4ba47d6cafbb814a8fbf6b9e1d31a86d0d98e`，源码指纹为
`eb426598f28eeb353333d91b2dd81fbef6f8eeae657733407396f029cff68e71`，使用 workerd `v1.20260826.1`。
静态检查、构建和完整 Gate 均成功；coverage 为 54,260 / 60,273 Rust lines（90.02%）。最终报告位于
`.temp/gate-run/20260829T121329-f77e4f98/report.json`。当次三轮执行全部通过是历史事实，不构成当前重复运行要求。

未执行发布、打包、runtime 下载或特权 egress。
