# P2.2：Queue Producer

状态：**implemented / Conditional Go（2026-08-27）**。

## 最终结果

- Queue catalog 和 lifecycle 属于 control authority，消息、配额、retention 与调度属于 queue／scheduler authority。
- Deployment binding 固定 Queue 身份和 producer 权限，private transport 重新验证 account、generation 和 capability。
- 普通 Worker 与 named `WorkerEntrypoint` 支持 `send()`、`sendBatch()` 和 `metrics()`；JSON、text、bytes、delay 和 batch 有明确上限。
- Producer 只有在持久提交后确认；提交后响应丢失保持 result-unknown，不自动重放。
- 删除、重建、snapshot/restore、stale generation 和跨库遗漏由持久 fence 与 reconciler 收敛。
- Durable Object producer 因 stock workerd output-gate 限制稳定返回 `QUEUE_DO_OUTPUT_GATE_UNSUPPORTED`，且不写入消息。

当前支持面见[兼容矩阵](../references/cloudflare-compatibility.md)。Consumer、ack/retry、DLQ 与 Cron 属于 P2.3。

## 历史验证

在 workerd `v1.20260826.1` 上，P2.2 单轮 Exit Gate、workspace real-runtime regression、静态检查和 coverage 成功；
coverage 为 43,685 / 48,521 Rust lines（90.03%）。未执行发行、部署或长期 RC soak。
