# 整机备份与保留

触发信号：计划维护窗口、当前 release 恢复演练或 RPO 要求到期。影响面是本地 control/KV/D1/DO/scheduler authority；R2 与 immutable object 仍绑定当前 Local/S3 authority，不是第二份 point-in-time copy。

Local snapshot 只提供一致性，不是异地备份。要覆盖磁盘/主机丢失，停机后必须把带 `format.json` 的完整 Local object root 复制到独立保护存储；不支持 Local↔S3 自动迁移或部分 root 恢复。若 Local root 位于 `<data.path>/objects`，fresh-host restore 需要先把完整 root 保存在目标 data-dir 之外，再对空 target 执行恢复。

只读诊断：停止 service 后检查当前 release、schema 和已有 committed manifest：

```bash
/opt/open-compute/ocd --config /etc/open-compute/platform.toml doctor --json
/opt/open-compute/ocd --config /etc/open-compute/platform.toml backup list --json
```

允许的 mutation：

```bash
/opt/open-compute/ocd --config /etc/open-compute/platform.toml backup create --name nightly-20260826 --json
/opt/open-compute/ocd --config /etc/open-compute/platform.toml backup inspect --snapshot 0198f000-0000-7000-8000-000000000001 --verify --json
```

预期输出包含 snapshot ID、精确 bytes/files 和 `verified=true`。data-dir/object-root lock 冲突、空间不足、MAC/hash、authority marker 或 immutable reference 失败都是停止条件。仅在另一份已验证快照满足 RPO 后，才允许用 `backup delete --snapshot` 删除一个精确 ID；manifest 最后删除。不要手删 Local envelope 或自行批量删除 S3 prefix。回滚是不删除旧 manifest。验证是重新 list/inspect，并确认 doctor 读取 `last-snapshot.json`。
