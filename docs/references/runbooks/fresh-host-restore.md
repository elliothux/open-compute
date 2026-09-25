# 全新主机恢复

触发信号：实例数据目录丢失或整机灾备演练。恢复时 R2 看到所选 object authority 的当前状态。先恢复选定作用域的 `ocd.toml`、全局 key、Gateway 持久状态以及清单引用的各份 `compute.toml`；保持原路径、运行 UID 和私有权限。不要把 cache、tmp、run 或 `ocd.lock` 当成备份输入。OCD_DIR 必须已恢复且可由运行 UID 写入；`backup restore` 只会在其中补建缺失的作用域锁，不会初始化 daemon 或其它实例。

影响面：全局 Gateway 与清单须作为同一作用域恢复；每个实例的数据和外置配置独立归属。只恢复一个实例不能代替核对其它已登记实例的目录与 InstanceId，且不得用该实例快照覆盖共享证书、配置或其它实例数据。

S3 实例可按下文从快照恢复至其 `compute.toml` 的显式空 `[data].path`。恢复期间把同一 master key 的 operator 备份作为该配置的外部 file/env 引用，恢复后再以 `0600` 放回实例数据目录并更新引用。Local 实例的 `objects/` 固定在数据目录内，不能同时作为空目标外的快照来源；须从 operator 独立备份恢复**完整实例目录**（含 `objects/format.json`、密钥和业务数据），不用 `backup restore`。不接受部分目录或 Local↔S3 migration。

只读诊断：安装 snapshot 的 exact source release，核对选定实例的配置、同一 master key 及 S3 authority；目标配置必须已在所选 `ocd.toml` 清单登记，S3 的目标数据目录必须不存在或为空。以下为已恢复 system 作用域中登记的默认实例示例：

```bash
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml capabilities --json
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml backup inspect --snapshot 0198f000-0000-7000-8000-000000000001 --verify --json
```

允许的 mutation：

```bash
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml backup restore --snapshot 0198f000-0000-7000-8000-000000000001 --json
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml doctor --full --json
```

包含 Workflow 的 snapshot 必须同时保留 control/scheduler authority、restart/purge intent、
operation progress 和 GC receipt。waiting/paused 的原始 deadline、inbox 及冻结 retention 不重算。
恢复后先让 exact-release reconciler 完成合法中间态，再验证原版本 replay、暂停状态、事件和 due work；
不得单独复制一边数据库或通过删除 operation row 使诊断变绿。这些状态的历史验收见
[P2.5 实现与验证](../../implemented/p2-5-workflow-durable-waiting.md)；每次正式发布仍需对应源码、pin 和 schema 的单轮验收。

预期是 sibling staging 全量验证后一次原子安装。非空 target、与其他已登记且存在的实例重复的 InstanceId、wrong key/release/object authority、path、hash、schema 或 marker 错误都是停止条件；不得使用 force 或覆盖旧目录。若其他实例的数据尚未恢复，所有目录恢复后、启动前还必须统一核对 InstanceId 不重复。失败时 target 保持为空，目标父目录保留 bounded `restore-failure` receipt 和同一 UUIDv7 的 object staging。确认不再需要诊断字节后，只允许精确清理该 receipt 报告的 ID：

```bash
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml backup cleanup-restore --staging 0198f000-0000-7000-8000-000000000002 --json
```

回滚是保持 target 为空并修复 key/release/object-authority/config；清理命令拒绝 symlink、hardlink、非普通文件、非 manifest restore path 和超出 hard cap 的 tree。验证是启动 exact release，读取 KV/D1/DO/alarm sentinel、检查部署 pin、basic WebSocket 重连、新写入和二次重启。全部步骤通过后再次停止 service，并显式记录 operator attestation；该命令会重新验证 snapshot、release、master key、platform identity 和原始 restore receipt，不能替代前述产品 smoke：

```bash
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml backup attest-restore-smoke --snapshot 0198f000-0000-7000-8000-000000000001 --passed --json
```
