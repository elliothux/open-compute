# 备份与保留

触发信号：计划维护窗口、当前 release 恢复演练或 RPO 要求到期。影响面是本地 control / KV / D1 / DO / scheduler 权威数据。R2 只绑定当前外部 S3，不是对象存储的时间点拷贝。runtime 解压缓存不属于 snapshot authority。

备份和恢复是离线操作：先停 service，再拿 data-dir 锁。

## 只读诊断

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml doctor --json
/opt/open-compute/ocd --config /etc/open-compute/config.toml backup list --json
```

## 创建与校验

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml backup create --name nightly-20260826 --json
/opt/open-compute/ocd --config /etc/open-compute/config.toml backup inspect --snapshot 0198f000-0000-7000-8000-000000000001 --verify --json
```

`--name` 是有界的人工审计标签。`--snapshot` 是 UUIDv7。`--verify` 会流式校验每个自有对象和 immutable reference。

预期输出包含 snapshot ID、精确 bytes/files 和 `verified=true`。data-dir lock 冲突、空间不足、MAC/hash、bucket marker 或 immutable reference 失败都是停止条件。

`backup inspect` 不带 `--verify` 只看已认证的 committed snapshot 元数据，不替代一次完整校验。

## 保留与删除

仅在**另一份已验证快照已满足 RPO** 之后，才允许用精确 ID 删除：

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml backup delete --snapshot 0198f000-0000-7000-8000-000000000001 --json
```

manifest 最后删除。回滚是不删除旧 manifest。

生成删除计划、不实际删对象：

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml backup retention-plan --keep-last 7 --json
```

可选 `--max-age-seconds` 和可重复的 `--keep-label`。看完计划再对列出的 ID 逐个 `backup delete`。不要写自己的 S3 批量删除去清 snapshot 前缀。

超过配置 grace 的不完整上传：

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml backup cleanup-incomplete --json
```

## 验证

重新 `backup list` / `backup inspect --verify`，并确认 doctor 能读 `last-snapshot.json`。没有实际跑过的验证不要记成成功。

恢复步骤见 [故障手册](/ocd/incidents/)：当前 release 恢复、全新主机恢复。恢复不会撤销快照之后已发生的外部副作用（包括 R2 当前状态）。
