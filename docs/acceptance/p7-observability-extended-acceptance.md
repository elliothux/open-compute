# P7 Observability 扩展资格

状态：active，2026-09-04。核心实现、本地 49/49 targets、1,107/1,107 cases 和 90.0146% coverage 见
[P7 Logs/Tail](../implemented/p7-workers-logs-realtime-tail.md)。本文只追踪 hosted、性能和跨平台资格。

## 剩余 Gate

- [ ] 在唯一临时 Cloudflare Worker 上比较 Script Tail TTL、not-found、`debug=true`、expiry、
  overload、slow-consumer close code 和多 session list shape。
- [ ] 比较 Service Binding、DO、Workflow、Queue、scheduled lifecycle 的 root/target attribution；
  当前差异为 `OC-OBSERVABILITY-001`。
- [ ] 比较 Telemetry omitted/null、type mismatch、retention race 和 unsupported view 错误。
- [ ] 运行参数化 benchmark，记录典型/最大 invocation、10 个实时客户端、2,000-event query、
  retention cleanup、quota 水位、p50/p95/p99 和 control API 影响。
- [ ] 验证正式 Linux/macOS targets 的单文件离线启动和固定 workerd。

外部资源必须唯一命名、精确清理并脱敏。新能力需有官方来源、固定输入和成功/失败回归。
完成后将结果并入 P7 implemented 文档并删除本文；未运行项不扩大当前支持声明。
