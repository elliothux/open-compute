# 整机备份与保留

触发信号：升级前、计划维护窗口或 RPO 要求到期。影响面是本地 control/KV/D1/DO/scheduler authority；R2 只绑定当前外部 S3，不是 point-in-time copy。

只读诊断：停止 service 后检查当前 release、schema 和已有 committed manifest：

```bash
/opt/open-compute/bin/platformd --config /etc/open-compute/platform.toml doctor --json
/opt/open-compute/bin/platformd --config /etc/open-compute/platform.toml backup list --json
```

允许的 mutation：

```bash
/opt/open-compute/bin/platformd --config /etc/open-compute/platform.toml backup create --name nightly-20260826 --json
/opt/open-compute/bin/platformd --config /etc/open-compute/platform.toml backup inspect --snapshot 0198f000-0000-7000-8000-000000000001 --verify --json
```

预期输出包含 snapshot ID、精确 bytes/files 和 `verified=true`。data-dir lock 冲突、空间不足、MAC/hash、bucket marker 或 immutable reference 失败都是停止条件。仅在另一份已验证快照满足 RPO 后，才允许用 `backup delete --snapshot` 删除一个精确 ID；manifest 最后删除。回滚是不删除旧 manifest。验证是重新 list/inspect，并确认 doctor 读取 `last-snapshot.json`。
