# 全新主机恢复

触发信号：原 data-dir 丢失或整机灾备演练。影响面是整个平台；恢复时 R2 看到所选 object authority 的当前状态。Local 模式要求完整 object root（含 `format.json`）已从独立保护副本恢复到与空 target data-dir 不重叠的配置路径；不接受部分目录，也不执行 Local↔S3 migration。

只读诊断：安装 snapshot 的 exact source release，提供同一 master key 的 `/etc/open-compute/recovery-master.key`，并确认 `/var/lib/open-compute` 不存在或为空：

```bash
/opt/open-compute/ocd --config /etc/open-compute/recovery.toml capabilities --json
/opt/open-compute/ocd --config /etc/open-compute/recovery.toml backup inspect --snapshot 0198f000-0000-7000-8000-000000000001 --verify --json
```

允许的 mutation：

```bash
/opt/open-compute/ocd --config /etc/open-compute/recovery.toml backup restore --snapshot 0198f000-0000-7000-8000-000000000001 --json
/opt/open-compute/ocd --config /etc/open-compute/recovery.toml doctor --full --json
```

包含 Workflow 的 snapshot 必须同时保留 control/scheduler authority、restart/purge intent、
operation progress 和 GC receipt。waiting/paused 的原始 deadline、inbox 及冻结 retention 不重算。
恢复后先让 exact-release reconciler 完成合法中间态，再验证原版本 replay、暂停状态、事件和 due work；
不得单独复制一边数据库或通过删除 operation row 使诊断变绿。这些状态已通过 P2.5 的三轮 snapshot
回归，见 [验证记录](../../implemented/p2-5-gate-results.md)；每次正式发布仍需对应源码、pin 和 schema 的验收。

预期是 sibling staging 全量验证后一次原子安装。非空 target、wrong key/release/object authority、path、hash、schema 或 marker 错误都是停止条件；不得使用 force 或覆盖旧目录。失败时 target 保持为空，目标父目录保留 bounded `restore-failure` receipt 和同一 UUIDv7 的 object staging。确认不再需要诊断字节后，只允许精确清理该 receipt 报告的 ID：

```bash
/opt/open-compute/ocd --config /etc/open-compute/recovery.toml backup cleanup-restore --staging 0198f000-0000-7000-8000-000000000002 --json
```

回滚是保持 target 为空并修复 key/release/object-authority/config；清理命令拒绝 symlink、hardlink、非普通文件、非 manifest restore path 和超出 hard cap 的 tree。验证是启动 exact release，读取 KV/D1/DO/alarm sentinel、检查部署 pin、basic WebSocket 重连、新写入和二次重启。全部步骤通过后再次停止 service，并显式记录 operator attestation；该命令会重新验证 snapshot、release、master key、platform identity 和原始 restore receipt，不能替代前述产品 smoke：

```bash
/opt/open-compute/ocd --config /etc/open-compute/recovery.toml backup attest-restore-smoke --snapshot 0198f000-0000-7000-8000-000000000001 --passed --json
```
