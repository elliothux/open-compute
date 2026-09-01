# 偏差

登记 ID：**`OC-QUEUE-001`**。

Queue producer 和 push consumer 的耐久性来自单节点 `scheduler.sqlite`，不是 Cloudflare 全球复制。投递是 at-least-once，没有全球 FIFO。未知的 native dispatch 会保留 lease，不消耗租户重试预算，所以后续投递可能重复同一 attempt number。

63 个目标成员因此是 `supported_with_deviation`。

见 [Compatibility](/platform/compatibility) 与 `docs/references/p1-deviations.md`。
