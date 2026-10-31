# P2.7：Runtime 与测试布局

状态：**implemented（2026-08-29）**。正式版本资格见[Release notes](../releases/README.md)。

## 最终结果

- TypeScript runtime 源码和测试按 domain 位于 `packages/runtime/`，生成的 `dist/` 不提交。
- Build manifest 固定源码、配置、依赖和输出集合；Cargo 只消费显式构建且校验通过的资产，生产启动离线。
- 唯一 Gate 调度入口是 `test/gate.py`，case registry 是 `test/gate_cases.py`；当前轮数、隔离与覆盖率规则只维护在[测试手册](../references/testing.md)。
- 退役 `poc/` 不再保留宿主、模拟协议、下载器或旧 Gate；仍有产品价值的断言已迁入对应 crate／process tests。
- G0 原始报告仅保留历史调查证据，不定义当前测试入口或 case 数。
- 临时运行、失败诊断和缓存统一位于 `.temp/`。

## 历史测量与验证

2026-08-28–29 在 macOS arm64、workerd `v1.20260826.1` 上，同一组六个 real-process targets 使用 4 个并发进程时，
执行时间由 148.10 秒降至 70.98 秒；6 并发仅降至 67.14 秒，因此当时选择默认最多 4。
只优化 `sha2` 与 `miniz_oxide` 后，runtime archive 验证样本中位数由 9.748 秒降至 1.038 秒。

最终 workspace 为 34 targets／690 cases，coverage 为 56,280 / 62,424 Rust lines（90.16%）；当日历史三轮
23 targets／63 cases 均通过。当前只要求一轮，以上数据不是跨平台性能承诺。
