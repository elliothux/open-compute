---
title: "故障手册"
---

出事时按症状走，不要先翻源码或内部 crate。本章按症状分页：停止条件、允许的 mutation、回滚与验证都写在对应页面。命令与内嵌 `ocd docs <name>` 一致。

路径示例用 `/var/lib/open-compute/instances/default/compute.toml`；`--config` 选择这份显式实例配置，`--system` 选择其 OCD 作用域。恢复时先恢复清单和配置，再临时将配置中的 master key 引用指向空实例目标以外的 operator 备份。

除非该节写明允许，否则不要：覆盖已有 data-dir、force、自愈改 SQLite、PATH 搜索或下载 workerd、把失败 upload 当 committed、生成新 master key 盖住旧平台。

按症状打开对应页面：

- [当前 release 恢复](/zh/docs/ocd/incidents/current-release/)
- [全新主机恢复](/zh/docs/ocd/incidents/fresh-host/)
- [磁盘压力](/zh/docs/ocd/incidents/disk/)
- [SQLite 损坏](/zh/docs/ocd/incidents/sqlite/)
- [S3 故障](/zh/docs/ocd/incidents/s3/)
- [workerd 崩溃循环](/zh/docs/ocd/incidents/workerd/)
- [Master key 丢失](/zh/docs/ocd/incidents/master-key/)
- [Scheduler 恢复](/zh/docs/ocd/incidents/scheduler/)
- [收集 support bundle](/zh/docs/ocd/incidents/support-bundle/)
