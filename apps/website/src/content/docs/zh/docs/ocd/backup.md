---
title: "备份与保留"
---

触发信号：计划维护窗口、当前 release 恢复演练或 RPO 要求到期。影响面是本地 control / KV / D1 / DO / scheduler 数据。R2 与其他 immutable reference 仍绑定选定 object authority；snapshot 会认证这些引用，但不是所有对象正文的第二份 point-in-time copy。runtime 解压缓存不属于 snapshot authority。

使用 Local 时，同一磁盘上的 platform snapshot 只是 consistency snapshot，不是 off-host backup。Local object root 固定在 `<data.path>/objects`；要覆盖磁盘或主机丢失，必须停机后独立备份**完整实例数据目录**（含 `objects/format.json`）、实际 `compute.toml`，以及配置引用的 master key 文件（若它位于数据目录外）。Local 的全新主机恢复需还原完整目录和密钥；`backup restore` 只支持 S3 快照。不提供 Local↔S3 migration，也不接受部分目录恢复。

备份对所选实例是离线操作：先停止该实例（或整个 daemon），再取得其实例数据锁。恢复则须停止所选作用域的整个 OCD daemon，CLI 在恢复期间持有作用域锁；目标只由所选 `compute.toml` 的 `[data].path` 决定，不能与其他已登记实例的数据根重叠。以下 `dev` 是已登记实例名示例；系统级安装还需加 `--system`。若 `compute.toml` 位于 `[data].path` 外，须另行备份；`OCD_DIR/ocd.toml`、全局密钥，以及完整的 `gateway/storage/` 和 `gateway/config-state/` 目录须作为同一份全局状态另行备份。重启时两处 storage 身份标记必须一致；丢失的 ACME storage 不会被静默重建。实例快照不包含这些文件或其他实例。

## 只读诊断

```sh
ocd --instance dev doctor --json
ocd --instance dev backup list --json
```

## 创建与校验

```sh
ocd --instance dev backup create --name nightly-20260826 --json
ocd --instance dev backup inspect --snapshot 0198f000-0000-7000-8000-000000000001 --verify --json
```

`--name` 是有界的人工审计标签。`--snapshot` 是 UUIDv7。`--verify` 会流式校验每个自有对象和 immutable reference。

预期输出包含 snapshot ID、精确 bytes/files 和 `verified=true`。data-dir/object-root lock 冲突、空间不足、MAC/hash、authority marker 或 immutable reference 失败都是停止条件。

`backup inspect` 不带 `--verify` 只看已认证的 committed snapshot 元数据，不替代一次完整校验。

## 保留与删除

仅在**另一份已验证快照已满足 RPO** 之后，才允许用精确 ID 删除：

```sh
ocd --instance dev backup delete --snapshot 0198f000-0000-7000-8000-000000000001 --json
```

manifest 最后删除。回滚是不删除旧 manifest。

生成删除计划、不实际删对象：

```sh
ocd --instance dev backup retention-plan --keep-last 7 --json
```

可选 `--max-age-seconds` 和可重复的 `--keep-label`。看完计划再对列出的 ID 逐个 `backup delete`。不要手删 Local envelope，也不要写自己的 S3 批量删除去清 snapshot 前缀。

超过配置 grace 的不完整上传：

```sh
ocd --instance dev backup cleanup-incomplete --json
```

## 验证

重新 `backup list` / `backup inspect --verify`，并确认 doctor 能读 `last-snapshot.json`。未经实际执行的验证不要记为成功。

恢复步骤见 [故障手册](/zh/docs/ocd/incidents/)：当前 release 恢复、全新主机恢复。恢复不会撤销快照之后已发生的外部副作用（包括 R2 当前状态）。
