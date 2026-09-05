# 配置

`--config` 指定唯一一份常规文件。相对值只按进程启动时的 cwd 解析一次，绝对值保持绝对语义；文件 leaf 以 no-follow 方式打开，不搜索 parent 或 `$HOME`。TOML 内的相对文件系统路径以实际打开配置文件的 canonical parent 为基准，`.`/`..` 会规范化，`~`、环境变量文本、glob 与 URI 不展开。解析阶段不读 `.env`、不解析密钥值，未知字段直接拒绝。

本页路径示例使用 `/etc/open-compute/config.toml`。部分内嵌 runbook 写成 `platform.toml`；命令行只有 `--config`，没有按文件名切换的第二套格式。

```sh
ocd config init --data-dir /var/lib/open-compute > /etc/open-compute/config.toml
ocd --config /etc/open-compute/config.toml config check
```

`config init` 先按启动 cwd 解析 `data-dir`，再把绝对路径写进模板并打印到 stdout；不创建目录、不写密钥。`config check` 只做静态解析与校验。

内嵌默认模板与 `share/default-config.toml` 同结构。运行中的数值上限以 `ocd --config /abs/config.toml capabilities --json` 的 `limits` 为准。

## 密钥

密钥只走引用，不要写进 unit、镜像、仓库或配置明文。

- `server.admin_auth`、`server.deployer_auth`、`server.read_only_auth`：三类必填且解析后互不相同的 Bearer token reference，使用 `env` 和/或 `file` 路径。
- 仅 S3 后端：`storage.access_key_id_env` / `storage.access_key_id_file` 与 `storage.secret_access_key_env` / `storage.secret_access_key_file`，每对至少提供一种；Local 不读取这些环境变量。
- master key：`data.master_key_file`；可选 `data.master_key_env`。
- 环境变量名必须是非空的大写 ASCII、数字和下划线，且不能以数字开头。
- 租户 binding 名不得以 `OPEN_COMPUTE_` 开头；那是平台保留前缀，不是让你把密钥写进配置正文的借口。

所有 admin listener（包括 loopback）都必须配置三类角色 token；解析后 token 值相同会拒绝启动，不按匹配顺序降权。

## `[data]`：平台状态与锁

`[data]`：

| 字段 | 作用 |
| --- | --- |
| `path` | 数据根。SQLite、身份、master key、runtime 解压、缓存都在这 |
| `master_key_file` | master key 路径 |
| `sqlite_busy_timeout_ms` | SQLite `busy_timeout` |
| `free_space_soft_bytes` | 低于此值健康降级 |
| `free_space_hard_bytes` | 低于此值拒绝 mutation；必须 ≤ soft |

一个 `ocd` 对应一个 data-dir。排他锁是 `<data_dir>/platform.lock`。第二实例会得到 `DATA_DIR_IN_USE`，不要绕过。data-dir 必须可写且可执行（解压后的 workerd 须在此执行）。

## `[storage]`：对象正文

`storage.backend` 必填且只能是 `local` 或 `s3`。两种 variant 互斥，不回退、不双写、不自动迁移；两者都使用 canonical 且互不重叠的 `prefix` / `r2_prefix`。

Local 字段：

| 字段 | 约束 |
| --- | --- |
| `path` | 安全本地对象根，只能是 `<data.path>/objects` 或与 `data.path` 完全分离 |
| `free_space_soft_bytes` | 低于此值对象存储健康降级 |
| `free_space_hard_bytes` | 低于此值拒绝对象写入；必须 ≤ soft |
| `partial_grace_ms` | 回收可证明归属的 crash 残留前的最短等待 |

Local root 必须是受支持本地文件系统上的 mode-0700 目录；symlink、特殊文件、未知 entry、不安全权限及 network/FUSE filesystem 均 fail closed。Local 直接访问文件系统，不启动 S3 server 或 rclone。

S3 使用 AWS SDK SigV4：

| 字段 | 约束 |
| --- | --- |
| `endpoint` | 服务 URL |
| `region` | 非空；`auto` 可接受 |
| `bucket` | 非空 |
| `force_path_style` | 默认 `true` |
| `verify_tls` | 不能关 |
| `prefix` / `r2_prefix` | 必须 canonical 且互不重叠 |

失败 upload 不是 committed。平台初始化后会绑定 backend kind 与 authority fingerprint；不要临时切换 backend、root、provider、bucket 或 prefix 来「先启动」。

## 其它段

模板还包含 `[server]`、`[runtime]`、`[cache]`、`[response_cache]`、`[images]`、`[metrics]`、`[hardening]`、`[workers]`、`[kv]`、`[r2]`、`[d1]`、`[queues]`、`[durable_objects]`、`[scheduler]`（含 pool）和 `[workflows]`。这些是本机配额与超时，不是 Cloudflare 套餐。改之前用 `config check`，改完用 `capabilities --json` 看实际 `limits`。

`hardening.emergency_reserve_bytes` 必须低于 `[data]` 的 hard reserve。
