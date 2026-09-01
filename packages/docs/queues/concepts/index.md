# 概念

Queue 在 control catalog 中有身份，消息行存储在 `scheduler.sqlite`。生产者绑定将 `send` 写入该存储；push consumer 从中领取 batch，并 dispatch 到 Worker 的 `queue` 处理函数。

每个 Queue 同时最多一个 active push consumer。投递语义为 at-least-once：处理函数在 ack 之前崩溃时会重新投递。不提供全局 FIFO。retry、并发与崩溃都可能打乱相对顺序。

## 无法识别的 native dispatch

若 native dispatch 结果未知（进程 / workerd 在确认前退出），lease 保留，**不**消耗租户 `max_retries`。恢复后可能以**同一 attempt number**再次投递。这不是 exactly-once。

[Queues JavaScript APIs](https://developers.cloudflare.com/queues/configuration/javascript-apis/) 的 send / batch / delay / content types / ack / retry / metrics 与 Cloudflare 对齐。单条消息 first-call-wins。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| JavaScript API | [Queues JavaScript APIs](https://developers.cloudflare.com/queues/configuration/javascript-apis/) | 相同：send / batch / delay / content types / ack / retry / metrics |
| 耐久性 | 全球复制 | 本机 `scheduler.sqlite` |
| 投递 | at-least-once | at-least-once；无法识别的 native dispatch 不释放该消息的 lease |
| 全局 FIFO | 提供 | 不提供 |
| Pull consumer | 提供 | 不提供 |
