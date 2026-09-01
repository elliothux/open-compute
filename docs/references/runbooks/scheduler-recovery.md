# Scheduler 恢复

触发信号：due lag、expired lease、repair backlog 或 scheduler DB 无法检查。影响面包括 Alarm、Queue、Cron、Workflow；普通 Worker/DO fetch 可处于 degraded。

只读诊断：

```bash
/opt/open-compute/ocd --config /etc/open-compute/platform.toml doctor --json
```

优先等待 token/expiry recovery 和 bounded repair。先使用 `/v1/scheduler`、`/v1/operator/workflows` 检查各 pool；Workflow 的 Unknown dispatch 保留 lease，不能把它当成可立即重试的业务失败。

Queue consumer 和 Cron activation 的 dispatch epoch 冻结在 scheduler projection 中。添加或编辑
HTTP route 不会替换它；重试 promotion 或启动 reconcile 复用该 epoch，并继续严格校验 target、
descriptor 与产品 generation。不要把当前 Worker route revision 写回已创建的 projection 或 claim。

只要 control 中存在 Queue、Cron activation、Workflow instance（包括 released/terminal/retained）、Workflow operation 或 Workflow version，就不能通过空库重建恢复调度历史。Workflow purge 在释放 control 引用后，scheduler 仍可能留有 GC receipt；损坏文件无法证明这些记录不存在。此时停止服务并按 [fresh-host restore](./fresh-host-restore.md) 恢复整机 snapshot；这不会撤销已发生的外部副作用。不要手动删除 referrer、operation、receipt 或 step row 绕过检查。

Workflow 的 waiting/paused 不占执行并发；`/v1/operator/workflows` 包含等待、inbox、retention 和 operation 计数。`workflow_*_results`、`workflow_consumed_events` 等保留历史指标是 gauge，restart/purge 可以降低它们；`workflow_event_intake_total` 和 `workflow_lifecycle_total` 是本进程观察到的调用结果。固定 metrics 预算现在至少需要 567 条序列，默认仍为 1024。

运行时未能确认 callback drain 时，当前 Workflow 执行路径会隔离当前 workerd generation。operator resume 不能解除隔离；只有 supervisor 启动的新 generation 才能重新接纳 Workflow。已有 lease 保留给 Unknown recovery，不按业务 retry 增加 attempt。

允许的 mutation：以下命令仅用于 **control 可验证且没有上述产品 authority** 的 alarm-only 数据目录，并要求 scheduler DB 确认损坏、service 已停止。命令会在移动文件前拒绝持有产品 authority 的目录：

```bash
/opt/open-compute/ocd --config /etc/open-compute/platform.toml scheduler recover-corrupt --backup-name scheduler-corrupt-20260826
```

预期旧 DB 被精确隔离，空 projection 从 DO alarm authority repair，不伪造已投递。control/DO authority 也损坏、未知 token 重复 commit或 backlog 不收敛是停止条件，并转整机 restore。回滚是保留隔离副本并恢复整机 snapshot。验证是 repair dry-run、alarm sentinel、lease expiry、重启和 lag 回到界限。
