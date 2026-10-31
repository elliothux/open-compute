---
title: "Scheduler 恢复"
---

触发信号：due lag、expired lease、repair backlog 或 scheduler DB 无法检查。影响面包括 Alarm、Queue、Cron、Workflow；普通 Worker/DO fetch 可处于 degraded。

只读诊断：

```sh
/opt/open-compute/ocd --config /var/lib/open-compute/instances/default/compute.toml doctor --json
```

优先等待 token/expiry recovery 和 bounded repair。通过 `/client/v4/open-compute/scheduler` 检查 scheduler，通过官方 account Workflows API 检查实例；Workflow 的 Unknown dispatch 保留 lease，不能把它当成可立即重试的业务失败。

Cron claim 在 dispatch 前先消耗 one-based delivery attempt。transport-unknown outcome 保留 claim 到 lease 到期，之后只有同时剩余配置 retry budget 和固定 15 分钟 handler deadline 时才重试。activation 进入 draining 后会立即 terminalize ready run，并且不再 requeue 到期 claim。Cron inspection 报告最近 error、最近 unknown classification 与最早 live deadline，因此单个 unknown result 不会无限阻塞后续 deployment。

Queue consumer 和 Cron activation 的 dispatch epoch 冻结在 scheduler projection 中。添加或编辑 HTTP route 不会替换它；重试 promotion 或启动 reconcile 复用该 epoch，并继续严格校验 target、descriptor 与产品 generation。不要把当前 Worker route revision 写回已创建的 projection 或 claim。

调度历史损坏时，停止服务并恢复整机 snapshot。恢复不会撤销已发生的外部副作用。不要删除 referrer、operation、receipt 或 step 行绕过检查。

Workflow 的 waiting/paused 不占执行并发；官方 Workflow instance 响应提供 lifecycle state 与 steps。`workflow_*_results`、`workflow_consumed_events` 等保留历史指标是 gauge，restart/purge 可以降低它们；`workflow_event_intake_total` 和 `workflow_lifecycle_total` 是本进程观察到的调用结果。固定 metrics 预算现在至少需要 567 条序列，默认仍为 1024。

运行时未能确认 callback drain 时，当前 Workflow 执行路径会隔离当前 workerd generation。operator resume 不能解除隔离；只有 supervisor 启动的新 generation 才能重新接纳 Workflow。已有 lease 保留给 Unknown recovery，不按业务 retry 增加 attempt。

Scheduler 表与控制数据共用 `control.sqlite`。数据库损坏时，停止服务并恢复整机 snapshot。`scheduler recover-corrupt` 会拒绝替换文件，因为替换也会覆盖控制数据。

升级启动时，程序在一个事务中导入旧 `scheduler.sqlite`，并在 `platform_meta` 写入完成标记。程序将旧文件保留为 `scheduler.sqlite.imported`，同时保留仍存在的 WAL 和 SHM 文件。重启可继续未完成的导入或改名，不会重复导入。control 已有调度行但缺少标记时，启动失败，不合并数据。

新 snapshot 仅通过 `control.sqlite` 保存调度表。恢复旧 snapshot 时，先使用来源版本恢复，再升级；启动程序会导入恢复的 scheduler 文件。验证升级后的实例和 snapshot 后，再决定旧副本的保留期限。
