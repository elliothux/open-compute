# 阻塞中的设计

本目录保存因外部前置能力缺失而暂停实施的设计。`blocked` 不代表实现完成或验收通过；
每项记录须保留阻塞证据、恢复条件、已有局部实现及未完成验证。前置条件满足并重新核验后，
再恢复实施；不得仅凭上游接口或类型声明解除阻塞。

| 文档 | 阻塞原因与恢复条件 |
| --- | --- |
| [P9 Workers Standard limits](p9-workers-standard-limits.md) | 等待 upstream stock workerd 正式 release 提供 request/isolate 资源限制执行器；更新 formal pin 并继续实施、验收前保持 fail closed，`OC-WKR-LIMIT-001` 开放 |

返回[文档索引](../README.md)。
