# 概念

一条 Queue 在 control catalog 里有身份，消息行在 `scheduler.sqlite`。Producer binding 把 `send` 写进权威；push consumer 从权威 claim batch，dispatch 到 Worker 的 `queue` handler。

每个 Queue 同时最多一个 active push consumer。投递是 at-least-once：handler 在 ack 之前崩溃会重投。不提供全球 FIFO。retry、并发和 crash 都可以打乱相对顺序。

## 未知 dispatch

若 native dispatch 结果未知（进程 / workerd 在确认前消失），lease 保留，**不**消耗租户 `max_retries`。恢复后可能用**同一个 attempt number**再投一次。这不是 exactly-once。

[Queues JavaScript APIs](https://developers.cloudflare.com/queues/configuration/javascript-apis/) 的 send / batch / delay / content types / ack / retry / metrics 与 Cloudflare 对齐。单条消息 first-call-wins。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| JavaScript API | [Queues JavaScript APIs](https://developers.cloudflare.com/queues/configuration/javascript-apis/) | 相同：send / batch / delay / content types / ack / retry / metrics |
| 耐久性 | 全球复制 | 该节点上的 `scheduler.sqlite` |
| 投递 | at-least-once | at-least-once；未知 dispatch 保留 lease |
| 全球 FIFO | 提供 | 不提供 |
| Pull consumer | 提供 | 不提供 |
