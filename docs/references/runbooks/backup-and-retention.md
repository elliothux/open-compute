# 整机备份与保留

触发信号：计划维护窗口、当前 release 恢复演练或 RPO 要求到期。影响面是所选实例的本地 control/KV/D1/DO/scheduler authority 以及 Artifacts Git repositories；R2 与 immutable object 仍绑定当前 Local/S3 authority，不是第二份 point-in-time copy。Artifacts metadata、bare Git files 与 token metadata 进入同一个 authenticated snapshot，token plaintext 从不持久化或备份。以下 `dev` 为已登记实例名示例，系统级作用域需显式加 `--system`；备份前停止该实例或整个 daemon。外置 `compute.toml`、OCD 清单/全局密钥和 Gateway 持久状态均需另行备份。

Local snapshot 只提供一致性，不是异地备份。Local object root 固定在 `<data.path>/objects`；要覆盖磁盘/主机丢失，停机后必须把**完整实例数据目录**（含 master key 和 `objects/format.json`）复制到独立保护存储，并另备份实际 `compute.toml`。Local 的 fresh-host 恢复是完整目录恢复，不能把内嵌的 object root 放在空 target 外再调用 `backup restore`；该命令只支持 S3 快照恢复。不支持 Local↔S3 自动迁移或部分目录恢复。

执行 `backup restore` 前须停止整个所选 OCD daemon，并先恢复该作用域的 `ocd.toml` 与配置文件。恢复期间 CLI 持有作用域锁；目标由所选 `compute.toml` 的 `[data].path` 唯一决定，不得与其他已登记实例的数据根相同或嵌套。缺失的待恢复 control 数据库不会被清单校验当成新实例重新初始化。

只读诊断：停止 service 后检查当前 release、schema 和已有 committed manifest：

```bash
ocd --instance dev doctor --json
ocd --instance dev backup list --json
```

允许的 mutation：

```bash
ocd --instance dev backup create --name nightly-20260826 --json
ocd --instance dev backup inspect --snapshot 0198f000-0000-7000-8000-000000000001 --verify --json
```

预期输出包含 snapshot ID、精确 bytes/files 和 `verified=true`。data-dir/object-root lock 冲突、空间不足、MAC/hash、authority marker 或 immutable reference 失败都是停止条件。仅在另一份已验证快照满足 RPO 后，才允许用 `backup delete --snapshot` 删除一个精确 ID；manifest 最后删除。不要手删 Local envelope 或自行批量删除 S3 prefix。回滚是不删除旧 manifest。验证是重新 list/inspect，并确认 doctor 读取 `last-snapshot.json`。

启用公网 Gateway 时，`<OCD_DIR>/gateway/storage/` 中的 ACME account、证书、私钥及 `.ocd-storage-id`，以及 `<OCD_DIR>/gateway/config-state/` 的已确认配置、`storage.id`、`attempted/` 和 `certified/` 标记，属于**同一份全局备份**，不属于任何实例 snapshot。启动时两处 storage ID 必须一致；已初始化的 storage 目录丢失、身份不一致、已有站点的 `.crt/.key/.json` 任一文件缺失，或已尝试签发的站点整目录丢失，均拒绝启动，不自动重签。DNS-01 challenge 发布前会持久记录尝试；因此首次签发失败后重启也可能保守拒绝，operator 应核对 ACME 状态并恢复完整全局备份。若确定放弃旧证书状态，须在 daemon 停止后先保留旧 Gateway 目录证据，再明确重新初始化全新 Gateway 根。`<OCD_DIR>/run/gateway/` socket、PID 与短期 challenge token 不进入恢复 authority。`ocd.toml` 引用的 operator Caddyfile 位于 OCD_DIR 外时，必须由 operator 单独备份并以相同绝对路径恢复；平台不会复制或覆盖这些源文件。实例备份仍只以各自 `compute.toml` 的 `[data].path` 为边界，外置配置须另行备份。
