# 故障手册

出事时按症状走，不要先翻源码或内部 crate。本章按症状分页：停止条件、允许的 mutation、回滚与验证都写在对应页面。命令与内嵌 `ocd docs <name>` 一致。

路径示例用 `/etc/open-compute/config.toml`。部分内嵌 runbook 写成 `platform.toml`；`--config` 只要绝对路径。全新主机恢复和 master key 恢复用单独的 `recovery.toml` / `recovery-master.key`，与日常配置分开。

除非该节写明允许，否则不要：覆盖已有 data-dir、force、自愈改 SQLite、PATH 搜索或下载 workerd、把失败 upload 当 committed、生成新 master key 盖住旧平台。

按症状打开对应页面：

- [当前 release 恢复](/zh/ocd/incidents/current-release)
- [全新主机恢复](/zh/ocd/incidents/fresh-host)
- [磁盘压力](/zh/ocd/incidents/disk)
- [SQLite 损坏](/zh/ocd/incidents/sqlite)
- [S3 故障](/zh/ocd/incidents/s3)
- [workerd 崩溃循环](/zh/ocd/incidents/workerd)
- [Master key 丢失](/zh/ocd/incidents/master-key)
- [Scheduler 恢复](/zh/ocd/incidents/scheduler)
- [收集 support bundle](/zh/ocd/incidents/support-bundle)
