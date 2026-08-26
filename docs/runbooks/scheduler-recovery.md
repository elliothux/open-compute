# Scheduler 恢复

触发信号：alarm due lag、expired lease、repair backlog 或 scheduler DB 无法检查。影响面是 DO alarms；普通 Worker/DO fetch 可处于 degraded。

只读诊断：

```bash
/opt/open-compute/bin/platformd --config /etc/open-compute/platform.toml doctor --json
```

优先等待 token/expiry recovery 和 bounded repair。允许的 mutation 仅限 scheduler DB 确认损坏且 service 已停止时执行：

```bash
/opt/open-compute/bin/platformd --config /etc/open-compute/platform.toml scheduler recover-corrupt --backup-name scheduler-corrupt-20260826
```

预期旧 DB 被精确隔离，空 projection 从 DO alarm authority repair，不伪造已投递。control/DO authority 也损坏、未知 token 重复 commit或 backlog 不收敛是停止条件，并转整机 restore。回滚是保留隔离副本并恢复整机 snapshot。验证是 repair dry-run、alarm sentinel、lease expiry、重启和 lag 回到界限。
