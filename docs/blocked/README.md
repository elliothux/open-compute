# 阻塞中的设计

本目录保存因外部前置能力缺失而暂停实施的设计。`blocked` 不代表实现完成或验收通过；
每项记录须保留阻塞证据、恢复条件、已有局部实现及未完成验证。前置条件满足并重新核验后，
再恢复实施；不得仅凭上游接口或类型声明解除阻塞。

目前本目录没有独立设计。2026-09-05 用户选择维护 workerd fork，
[workerd P2 Workers Standard limits](../workerd/p2-workers-standard-limits.md) 已转入
[workerd 原生方案](../workerd/README.md)，不再等待 upstream 合并后才能开发。
原生 limits 仍未完成验收，`OC-WKR-LIMIT-001` 保持开放；迁移文档不代表运行时已支持该能力。

返回[文档索引](../README.md)。
