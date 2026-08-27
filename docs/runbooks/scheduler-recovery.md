# Scheduler 恢复

触发信号：due lag、expired lease、repair backlog 或 scheduler DB 无法检查。影响面包括 Alarm、Queue、Cron、Workflow；普通 Worker/DO fetch 可处于 degraded。

只读诊断：

```bash
/opt/open-compute/bin/platformd --config /etc/open-compute/platform.toml doctor --json
```

优先等待 token/expiry recovery 和 bounded repair。先使用 `/v1/scheduler`、`/v1/operator/workflows` 检查各 pool；Workflow 的 Unknown dispatch 保留 lease，不能把它当成可立即重试的业务失败。

只要 control 中存在 Queue、Cron activation 或任何 Workflow instance（包括 released/terminal），就不能通过空库重建恢复调度历史。此时停止服务并按 [fresh-host restore](./fresh-host-restore.md) 恢复整机 snapshot；这不会撤销已发生的外部副作用。不要手动删除 referrer 或 step row 绕过检查。

允许的 mutation：以下命令仅用于 **control 可验证且没有上述产品 authority** 的 alarm-only 数据目录，并要求 scheduler DB 确认损坏、service 已停止。命令会在移动文件前拒绝持有产品 authority 的目录：

```bash
/opt/open-compute/bin/platformd --config /etc/open-compute/platform.toml scheduler recover-corrupt --backup-name scheduler-corrupt-20260826
```

预期旧 DB 被精确隔离，空 projection 从 DO alarm authority repair，不伪造已投递。control/DO authority 也损坏、未知 token 重复 commit或 backlog 不收敛是停止条件，并转整机 restore。回滚是保留隔离副本并恢复整机 snapshot。验证是 repair dry-run、alarm sentinel、lease expiry、重启和 lag 回到界限。
