# 配置

`--config` 必须是绝对路径的常规文件，不从 cwd、`$HOME` 搜索，不跟随符号链接，路径里不能有 `..`。解析阶段不读 `.env`、不解析密钥值。未知字段将被拒绝。`runtime.binary`、`runtime.lock_file`、`runtime.assets_dir` 是未知配置项，启动前即拒绝。

本页路径示例使用 `/etc/open-compute/config.toml`。部分内嵌 runbook 写成 `platform.toml`；命令行只有 `--config`，没有按文件名切换的第二套格式。

```sh
ocd config init --data-dir /var/lib/open-compute > /etc/open-compute/config.toml
ocd --config /etc/open-compute/config.toml config check
```

`config init` 把指定绝对 `data-dir` 写进模板并打印到 stdout，不创建目录、不写密钥。`config check` 只做静态解析与校验。

内嵌默认模板与 `share/default-config.toml` 同结构。运行中的数值上限以 `ocd --config /abs/config.toml capabilities --json` 的 `limits` 为准。

## 密钥

密钥只走引用，不要写进 unit、镜像、仓库或配置明文。

- `server.admin_auth`：`env` 和/或绝对路径 `file`（TOML 里的 secret 对象）。
- S3：`access_key_id_env` / `access_key_id_file` 与 `secret_access_key_env` / `secret_access_key_file`，每对至少提供一种。
- master key：`storage.master_key_file`（绝对路径）；可选 `storage.master_key_env`。
- 环境变量名必须是非空的大写 ASCII、数字和下划线，且不能以数字开头。
- 租户 binding 名不得以 `OPEN_COMPUTE_` 开头；那是平台保留前缀，不是让你把密钥写进配置正文的借口。

非 loopback 的 admin 监听必须配置 `server.admin_auth`。

## data-dir 与锁

`[storage]`：

| 字段 | 作用 |
| --- | --- |
| `data_dir` | 绝对数据根。SQLite、身份、master key、runtime 解压、缓存都在这 |
| `master_key_file` | 绝对 master key 路径 |
| `sqlite_busy_timeout_ms` | SQLite `busy_timeout` |
| `free_space_soft_bytes` | 低于此值健康降级 |
| `free_space_hard_bytes` | 低于此值拒绝 mutation；必须 ≤ soft |

一个 `ocd` 对应一个 data-dir。排他锁是 `<data_dir>/platform.lock`。第二实例会得到 `DATA_DIR_IN_USE`，不要绕过。data-dir 必须可写且可执行（解压后的 workerd 须在此执行）。

## S3（SigV4）

S3 是平台 authority 的一部分，单文件不内嵌对象存储。协议是 AWS SDK SigV4。

| 字段 | 约束 |
| --- | --- |
| `endpoint` | 服务 URL |
| `region` | 非空；`auto` 可接受 |
| `bucket` | 非空 |
| `force_path_style` | 默认 `true` |
| `verify_tls` | 不能关 |
| `prefix` / `r2_prefix` | 必须不相交；`prefix` 不能以 `tenant/` 开头 |

失败 upload 不是 committed。不要临时换 provider/prefix 来「先启动」。

## 其它段

模板还包含 `[server]`、`[runtime]`、`[cache]`、`[response_cache]`、`[images]`、`[metrics]`、`[hardening]`、`[workers]`、`[kv]`、`[r2]`、`[d1]`、`[queues]`、`[durable_objects]`、`[scheduler]`（含 pool）和 `[workflows]`。这些是本机配额与超时，不是 Cloudflare 套餐。改之前用 `config check`，改完用 `capabilities --json` 看实际 `limits`。

`hardening.emergency_reserve_bytes` 必须低于 storage 的 hard reserve。
