---
title: "配置"
---

`--config` 指定唯一一份常规文件。相对值只按进程启动时的 cwd 解析一次，绝对值保持绝对语义；文件 leaf 以 no-follow 方式打开，不搜索 parent 或 `$HOME`。TOML 内的相对文件系统路径以实际打开配置文件的 canonical parent 为基准，`.`/`..` 会规范化，`~`、环境变量文本、glob 与 URI 不展开。解析阶段不读 `.env`、不解析密钥值，未知字段直接拒绝。

## 配置归属

关于 daemon、实例、数据目录、Gateway 与扩展如何协作，请先看[架构与职责边界](/zh/docs/ocd/architecture/)。

配置分为两层：

- `<OCD_DIR>/ocd.toml` 配置 daemon 作用域：共享 listener、admin 凭据、Gateway、全局限制和显式实例 registry。
- 每份已登记的 `compute.toml` 配置一个隔离实例：数据与对象权威、凭据、产品、Dashboard、公网 base domain 和扩展。

```toml title="ocd.toml"
[server]
public_bind = "127.0.0.1:8787"
admin_auth = { file = "./keys/admin.token" }

[artifacts]
max_concurrent_requests = 16

[metrics]
max_series = 1024

[[instances]]
config = "instances/default/compute.toml"
autostart = true
```

受管实例必须在 `<OCD_DIR>/ocd.toml` 中显式登记。其 `[server]` 配置共享的 `public_bind` 和可选 `admin_bind`；`[artifacts].max_concurrent_requests`（默认 16）限制所有实例的 Git 在途请求，`[metrics].max_series`（默认 1024）限制 daemon 对外暴露的 metric series。每个 `[[instances]]` 条目只包含 `config` 与 `autostart`；身份、数据路径、digest 与进程状态不会复制进清单。用户 OCD 目录由运行 UID 的系统账户 home 加 `.open-compute` 确定，不受覆盖的 `HOME` 影响；系统 OCD 目录是 `/var/lib/open-compute`。

`<OCD_DIR>/instances/` 只是 setup 建议的默认位置。运行时不扫描该目录，不据此登记或启动实例，也不从目录名推导身份或数据位置；外置配置和数据目录仍然支持。

当前每个启用的实例会暴露 752 条固定 metric series。共享默认值 1024 只容纳一份完整实例抓取；第二个新的抓取会得到 `503`，但不会停止任何实例。需要同时抓取多个实例时，应提高 `ocd.toml` 的 `[metrics].max_series`。实例停止后释放已登记的 series。

已登记实例不能声明相同或父子重叠的公网 base domain。登记和 daemon 冷启动都会先拒绝冲突，不按加载顺序选择赢家。

本页路径示例使用系统级默认实例。部分内嵌 runbook 使用其他精确文件名；命令行只有 `--config`，没有按文件名切换的第二套格式。

```sh
ocd config init --data-dir /var/lib/open-compute/instances/default/data > /var/lib/open-compute/instances/default/compute.toml
ocd --config /var/lib/open-compute/instances/default/compute.toml config check
```

`config init` 先按启动 cwd 解析 `data-dir`，再把绝对路径写进模板并打印到 stdout；不创建目录、不写密钥。`config check` 只做静态解析与校验。

内嵌默认模板与 `share/default-config.toml` 同结构。运行中的数值上限以 `ocd --config /abs/config.toml capabilities --json` 的 `limits` 为准。

## Operator HTTP proxy

operator-owned AI、target、release 与远程 S3 请求在 `ocd` 启动时选择一个 proxy，第一个非空变量生效：

```text
HTTPS_PROXY → https_proxy → ALL_PROXY → all_proxy → HTTP_PROXY → http_proxy → direct
```

选中值必须是无 credential 的 canonical `http://host:port` URL。`NO_PROXY` 优先于 `no_proxy`，支持 `*`、exact IP、IP CIDR、exact domain 与 domain suffix；loopback destination 始终直连。不支持 macOS System Settings、PAC/WPAD、SOCKS、proxy authentication、interception CA 或 OS proxy discovery。必须把变量写入实际启动 `ocd` 的 shell、launchd unit 或 service environment；显式 proxy 无效或不可达时 fail closed。tenant Worker egress 与 public Git import 不使用该策略。

`GET /client/v4/open-compute/system/status` 只报告 `direct`、`proxy` 或 `invalid`；有效 proxy 还会报告命中的变量名和无 credential 的 origin。

## 密钥

密钥只走引用，不要写进 unit、镜像、仓库或配置明文。

- 作用域 `ocd.toml` 中的 `server.admin_auth` 是唯一的全局 admin Bearer token 引用。每份 `compute.toml` 只配置 `auth.deployer_auth` 和 `auth.read_only_auth`；三者解析后的 token 必须互不相同。引用使用 `env` 和/或 `file` 路径。
- 仅 S3 后端：`storage.access_key_id_env` / `storage.access_key_id_file` 与 `storage.secret_access_key_env` / `storage.secret_access_key_file`，每对至少提供一种；Local 不读取这些环境变量。
- master key：`data.master_key_file`；可选 `data.master_key_env`。
- 环境变量名必须是非空的大写 ASCII、数字和下划线，且不能以数字开头。
- 租户 binding 名不得以 `OPEN_COMPUTE_` 开头；那是平台保留前缀，不是让你把密钥写进配置正文的借口。

所有 admin listener（包括 loopback）都必须配置三类角色 token；解析后 token 值相同会拒绝启动，不按匹配顺序降权。

## 实例身份与可选界面

`[instance].name` 是 CLI 与 Dashboard 使用的可变显示名。持久 InstanceId 在实例数据权威中初始化，不能通过配置指定。

在实例配置中启用 operator UI：

```toml
[dashboard]
enabled = true
```

实例也可声明一个公网 Gateway domain：

```toml
[public_gateway]
base_domain = "compute.example.com"
```

详见 [Dashboard](/zh/docs/ocd/dashboard/) 和 [Gateway](/zh/docs/gateway/)。共享 Gateway listener 与 Caddy 输入仍属于 `ocd.toml`。

## `[ai]`：provider backend 与 embedding profile

一个 AI backend 表示一个 operation-specific 最终请求 URL。`ocd` 不会追加 `/embeddings` 或 `/chat/completions`，因此 `endpoint` 必须同时包含 provider 的路径前缀和操作路由：

```toml
[ai]
default_embedding_model = "company/qwen-embedding"

[ai.backends.bailian-embeddings]
protocol = "openai_embeddings_v1"
endpoint = "https://dashscope.aliyuncs.com/compatible-mode/v1/embeddings"
auth = { kind = "bearer", secret = { env = "DASHSCOPE_API_KEY" } }
headers = { "X-Title" = "open-compute" }

[ai.embedding_profiles."qwen/qwen3-1024"]
dimensions = 1024
max_input_tokens = 8192
send_dimensions = true
tokenizer = { kind = "qwen3", revision = "pinned-tokenizer-revision", artifact = { path = "/opt/open-compute/tokenizers/qwen3/tokenizer.json", sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" } }

[ai.embedding_models."company/qwen-embedding"]
backend = "bailian-embeddings"
remote_model = "text-embedding-v4"
profile = "qwen/qwen3-1024"
```

创建 AI Search 实例前必须配置 `default_embedding_model`。只有需要 AI Search chat、query rewrite 或 reranking 时，才需要再配置 `default_generation_model` 和对应的 `generation_models` 条目。

认证是闭集：`bearer`、单个自定义 secret `header` 或 `none`。需要自定义 key header 时写成 `auth = { kind = "header", name = "X-API-Key", secret = { file = "/run/secrets/provider-key" } }`。可选 `headers` map 只承载非敏感静态 metadata，不能覆盖 `Authorization`、自定义 auth header、host/content header、cookie、proxy 或 hop-by-hop header。`none` 只允许 loopback HTTP；非 loopback endpoint 必须使用 HTTPS。

Profile 让模型事实可复用而不变成隐式猜测。维度、最大输入 token 数、是否发送 `dimensions`，以及 digest-pinned offline tokenizer 都属于 profile；AI Search 的 metric 固定为 cosine。`config check` 只验证 artifact 声明而不读取文件；`ocd` 组合 AI Search 服务时校验本地 bytes，绝不下载 tokenizer。

## `[data]`：平台状态与锁

`[data]` 必填，其中的 `path` 也必填。运行时不会根据配置文件名、配置父目录或 `instances/` 目录推断数据目录：

| 字段                     | 作用                                                           |
| ------------------------ | -------------------------------------------------------------- |
| `path`                   | 数据根。保存 SQLite、身份、本地对象、实例级 runtime 状态与缓存 |
| `master_key_file`        | master key 路径；相对配置文件解析，可位于数据根之外            |
| `sqlite_busy_timeout_ms` | SQLite `busy_timeout`                                          |
| `free_space_soft_bytes`  | 低于此值健康降级                                               |
| `free_space_hard_bytes`  | 低于此值拒绝 mutation；必须 ≤ soft                             |

相对 `data.path` 以 `compute.toml` 所在目录为基准解析。共享且已校验的 runtime package 位于 `<OCD_DIR>/cache/packages/`；实例级 runtime config、lease 与 staging 状态才位于此数据根内。若解析后的数据根位于 OCD_DIR 内，它必须是 `<OCD_DIR>/instances/` 的严格子目录；OCD_DIR 本身、`instances/` 容器、`instances-old/` 及 OCD_DIR 的其他子树都拒绝。外置数据根允许使用，但不得反过来包含 OCD_DIR；已登记的数据根不得互相重叠。

同一作用域的一个 `ocd` daemon 管理已登记的实例。每个实例独占 `<data.path>/platform.lock`，不要绕过；数据目录须可写且可执行。

## `[storage]`：对象正文

`storage.backend` 必填且只能是 `local` 或 `s3`。两种 variant 互斥，不回退、不双写、不自动迁移；两者都使用 canonical 且互不重叠的 `prefix` / `r2_prefix`。

Local 字段：

| 字段                    | 约束                                    |
| ----------------------- | --------------------------------------- |
| `free_space_soft_bytes` | 低于此值对象存储健康降级                |
| `free_space_hard_bytes` | 低于此值拒绝对象写入；必须 ≤ soft       |
| `partial_grace_ms`      | 回收可证明归属的 crash 残留前的最短等待 |

Local root 固定为 `<data.path>/objects`；不接受 `storage.path`。它必须是受支持本地文件系统上的 mode-0700 目录；symlink、特殊文件、未知 entry、不安全权限及 network/FUSE filesystem 均 fail closed。Local 直接访问文件系统，不启动 S3 server 或 rclone。

S3 使用 AWS SDK SigV4：

| 字段                   | 约束                      |
| ---------------------- | ------------------------- |
| `endpoint`             | 服务 URL                  |
| `region`               | 非空；`auto` 可接受       |
| `bucket`               | 非空                      |
| `force_path_style`     | 默认 `true`               |
| `verify_tls`           | 不能关                    |
| `prefix` / `r2_prefix` | 必须 canonical 且互不重叠 |

多个实例共用同一 S3 endpoint 和 bucket 时，必须分别配置互不重叠的 `prefix` 与 `r2_prefix`；不能让两实例都使用默认值。登记时会拒绝任意前缀交叠，启动时两个远端前缀的 marker 都绑定 InstanceId；marker 缺失或不匹配会拒绝启动。

失败 upload 不是 committed。平台初始化后会绑定 backend kind 与 authority fingerprint；不要临时切换 backend、root、provider、bucket 或 prefix 来「先启动」。

## `[extensions.<name>]`：可信本地原生扩展

macOS 与 Linux operator 可把本地扩展静态暴露为 Service Binding 目标：

```toml
[extensions.local-files]
path = "./extensions/local-files"
```

路径相对实际加载的实例 config file 解析。目录必须包含严格的 `extension.toml`，指向一个已打包 facade module 与一个可执行 Provider。扩展是 operator 信任的代码，只在该实例启动时加载；`ocd` 不向它注入 tenant secret 或平台凭据，也不负责安装、下载、版本管理、热更新或 OS sandbox。扩展名与该实例的 Worker service name 共用 namespace，不得与 live Worker 冲突。完整说明见[扩展](/zh/docs/extension/)。

## `[private_services.<name>]`：固定私网 HTTP Service target

operator 可以通过标准 Service Binding `fetch()` 暴露一个固定的私网或回环 HTTP endpoint，而不向租户开放通用私网出站：

```toml
[private_services.inventory]
scheme = "http"
host = "10.20.0.15"
port = 8080
path_prefixes = ["/v1/"]
methods = ["GET", "POST"]
credential_header = "x-api-key"
credential = { file = "/run/secrets/inventory-key" }
allow = [{ account_id = "<instance-id>", worker_id = "<worker-id>", entrypoint = "api" }]
```

`ocd` 启动时把 endpoint 解析并固定到私网地址；重定向只返回给调用方，绝不跟随。调用 Worker 不能选择 URL 或 credential；内部 header 与租户认证 header 会被移除，配置 credential 只在 host 侧请求中注入。上传准入和每次调用都会重新检查精确 instance、Worker、可选 Version、entrypoint 与 policy revision。因此新建 Worker 必须先取得稳定 Worker ID，才能添加该 binding。私网 target 只支持 Service Binding HTTP `fetch()`；RPC 与 `connect()` fail closed。普通租户 `fetch()` 仍只允许公网地址。

## 其它段

实例模板还包含 `[instance]`、`[auth]`、`[runtime]`、`[cache]`、`[response_cache]`、`[images]`、`[ai]`、`[document_parser]`、`[observability]`、`[metrics]`、`[hardening]`、`[workers]`、`[kv]`、`[r2]`、`[d1]`、`[queues]`、`[durable_objects]`、`[scheduler]`（含 pool）、`[workflows]`，以及可选的 `[dashboard]`、`[public_gateway]`、`[extensions.<name>]` 和 `[private_services.<name>]`。公共监听设置只属于 `ocd.toml`，不属于 `compute.toml`。这些是本机配额与超时，不是 Cloudflare 套餐。改之前用 `config check`，改完用 `capabilities --json` 看实际 `limits`。

`hardening.emergency_reserve_bytes` 必须低于 `[data]` 的 hard reserve。
