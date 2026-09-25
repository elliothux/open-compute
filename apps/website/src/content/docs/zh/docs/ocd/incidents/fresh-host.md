---
title: "全新主机恢复"
---

触发信号：实例数据目录丢失或整机灾备演练。恢复时 R2 看到所选 object authority 的当前状态。先恢复选定作用域的 `ocd.toml`、全局 key、Gateway 持久状态以及清单引用的各份 `compute.toml`；保持原路径、运行 UID 和私有权限。不要把 cache、tmp、run 或 `ocd.lock` 当成恢复权威。OCD_DIR 必须已恢复；`backup restore` 只补建缺失的作用域锁。

S3 实例可按下文从快照恢复至已登记配置的显式空 `[data].path`；恢复期间在该配置中临时引用目标目录外的同一 master key 备份，完成后以 `0600` 放回实例目录并更新引用。Local 实例的 `objects/` 固定在数据目录内，须从独立备份恢复**完整实例目录**（含 `objects/format.json`、密钥和业务数据），不能用 `backup restore`。不接受部分目录或 Local↔S3 migration。

S3 只读诊断：安装 snapshot 的 exact source release，核对已登记的配置、密钥和 S3 authority，并确认目标实例数据目录不存在或为空。以下为已恢复 system 作用域中默认实例的示例：

```sh
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml capabilities --json
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml backup inspect --snapshot 0198f000-0000-7000-8000-000000000001 --verify --json
```

允许的 mutation：

```sh
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml backup restore --snapshot 0198f000-0000-7000-8000-000000000001 --json
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml doctor --full --json
```

包含 Workflow 的 snapshot 必须同时保留 control/scheduler authority、restart/purge intent、operation progress 和 GC receipt。waiting/paused 的原始 deadline、inbox 及冻结 retention 不重算。恢复后先让 exact-release reconciler 完成合法中间态，再验证原版本 replay、暂停状态、事件和 due work；不得单独复制一边数据库或通过删除 operation row 使诊断变绿。

预期是 sibling staging 全量验证后一次原子安装。非空 target、wrong key/release/object authority、path、hash、schema 或 marker 错误都是停止条件；不得使用 force 或覆盖旧目录。失败时 target 保持为空，目标父目录保留 bounded `restore-failure` receipt 和同一 UUIDv7 的 object staging。确认不再需要诊断字节后，只允许精确清理该 receipt 报告的 ID：

```sh
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml backup cleanup-restore --staging 0198f000-0000-7000-8000-000000000002 --json
```

回滚是保持 target 为空并修复 key/release/object-authority/config。清理命令拒绝 symlink、hardlink、非普通文件、非 manifest restore path 和超出 hard cap 的 tree。

验证是启动 exact release，读取 KV/D1/DO/alarm sentinel、检查部署 pin、basic WebSocket 重连、新写入和二次重启。全部步骤通过后再次停止 service，并显式记录 operator attestation；该命令会重新验证 snapshot、release、master key、platform identity 和原始 restore receipt，不能替代前述产品 smoke：

```sh
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml backup attest-restore-smoke --snapshot 0198f000-0000-7000-8000-000000000001 --passed --json
```
