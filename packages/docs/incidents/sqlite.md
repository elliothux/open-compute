# SQLite 损坏

触发信号：doctor 报 quick_check、migration checksum、foreign-key 或 schema tuple 失败。影响面取决于 control、scheduler、KV 或 D1 文件；control 损坏视为整机故障。

只读诊断：停止 service，运行：

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml doctor --json
/opt/open-compute/ocd --config /etc/open-compute/config.toml backup list --json
```

不得直接编辑 SQLite、WAL、SHM、migration 表。允许的 mutation 是从最近已验证整机 snapshot 做 [fresh-host restore](/incidents/fresh-host)；只有 control 可验证且没有 Queue、Cron activation、Workflow history、operation 或 version 的 alarm-only 目录，才能使用 `scheduler recover-corrupt --backup-name scheduler-corrupt-20260826` 重建 projection。Workflow catalog 也会阻止重建，因为损坏文件可能仍持有 purge receipt，详见 [Scheduler 恢复](/incidents/scheduler)。

预期是损坏文件保持隔离且 authority 不被自愈改写。停止条件是没有 committed snapshot、master key 或同一 S3 authority；此时立即保全文件。回滚是回到恢复前的只读 evidence。验证是 doctor full、P0 smoke、schema checksum 和一次重启。
