# P1 剩余发行验收

状态：active，2026-08-28。P1.0–P1.7 的核心实现和本地证据见
[P1 平台加固](../implemented/p1-platform-hardening.md)；本文只验证单机发行物的长期稳定性和恢复。

## 剩余 Gate

- [ ] 1 小时 developer mixed soak：固定源码、workerd pin、配置和主机，记录负载、故障恢复、资源增长和错误。
- [ ] 24 小时 release-candidate mixed soak：记录运行身份、故障计划、稳定性指标、恢复时间和失败证据。
- [ ] 在已授权的正式单文件 `ocd` 上验证隔离首启、service/container 运行、备份、fresh-host restore、
  当前版本替换及失败恢复。
- [ ] 汇总实际 revision、命令、持续时间、结果和限制。

长时验收只在实现冻结后运行；下载、打包、部署、提权和数据变更仍需相应授权。历史
[P1.8 No-Go](../implemented/p1-8-results.md)不代表当前 hibernatable WebSocket 能力，也不替代本计划。
完成后将结果并入 P1 implemented 文档并删除本文。
