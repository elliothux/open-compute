# P12：Wrangler 项目开发与部署体验

状态：Day 1 产品合同与架构设计完成；待实施与验收。

日期：2026-09-07

本文定义当一台机器上已有常驻 `ocd` daemon 时，Worker 项目如何开发、选择目标、管理凭据、部署和查看日志。
本文建立在 [P6 Cloudflare v4 API 与 Wrangler 子集兼容设计](implemented/p6-cloudflare-v4-wrangler-compatibility.md)、
[P7 Workers Logs 与 realtime tail](implemented/p7-workers-logs-realtime-tail.md) 和
[P11 ocd 安装、实例与本机运维体验](implemented/p11-ocd-operator-experience.md) 之上。

核心结论是：**Wrangler 管项目，`ocd` 管平台、目标选择和凭据注入。** `ocd` 不实现第二套 build/deploy 客户端，
也不要求每个 Worker 项目启动一个 daemon。

## 1. 目标与非目标

P12 的目标：

- 保留标准 `wrangler.jsonc` 和上游 Wrangler 命令，不创建 open-compute 项目配置方言；
- 一个常驻 `ocd` 安全承载多个 Worker 项目、Version、Deployment 和资源；
- 日常编码使用快速的本地 `wrangler dev`，真实集成使用专用 dev/staging Deployment；
- 提供窄的 `ocd wrangler` 包装命令，只负责目标选择、兼容版本检查和子进程环境注入；
- 区分本机 `instance` 与远程 `target`，避免用本机 registry 假装远程控制平面；
- 不把 API token 写入 `wrangler.jsonc`、项目 `.env`、argv、日志或 shell history；
- CI 仍可直接使用 Wrangler 标准环境变量，不依赖开发机的隐式状态。

P12 Day 1 不提供：

- `ocd deploy`、`ocd dev` 或自定义 Worker bundler/uploader；
- Wrangler fork、全局隐式安装或由 `ocd` 运行时下载 Node.js/Wrangler；
- `wrangler dev --remote`、Cloudflare preview URL/route 或远程 binding proxy 的伪兼容；
- Git push/build service、自动 deploy-on-save、托管 CI/CD 或自动生产发布；
- 通过 project directory 自动修改一台远程生产 daemon 的 service lifecycle；
- 多机集群、跨安装流量切分或托管式 OAuth control plane。

## 2. 三层配置边界

| 对象 | Authority | 用途 |
| --- | --- | --- |
| open-compute 实例 | `compute.toml` 或 `/etc/open-compute/config.toml` | daemon、data-dir、listener、object backend、平台 secret reference |
| Worker 项目 | `wrangler.jsonc` | source、compatibility date/flags、vars、bindings、triggers、Wrangler environment |
| 开发机远程目标 | per-user `ocd` target registry | API base URL、account ID、deployer credential reference |

三者不能合并。`compute.toml` 不是 Worker 项目清单，`wrangler.jsonc` 不存储 daemon path 或 open-compute token，
target 也不复制 Worker bindings。普通 Worker repository 只需要 `wrangler.jsonc`；只有项目目录本身同时是本机平台运维目录时，
才需要 `compute.toml`。

一个 `ocd` instance 可承载多个 Worker project；项目以 account + Script name 和各资源的官方 ID 区分。删除或重新部署一个项目
不得改写 instance config、service definition 或其他项目的 authority。

## 3. 概念：instance、target 与 Wrangler environment

| 概念 | 例子 | 选择什么 | 是否可远程 |
| --- | --- | --- | --- |
| instance | `k7m2r` | 本机由某个 canonical config path 派生的 `ocd` 进程 | 否 |
| target | `company-prod` | 一个 API origin + account + credential reference | 是 |
| Wrangler environment | `dev`、`staging`、`production` | 同一 Worker project 内的名称、vars 和 bindings 投影 | 是 |

instance ID 是 P11 的本机运维身份，不发放给远程开发机作为伪集群 ID。target name 是开发者自定义的本机别名，
不进入服务端 identity。Wrangler environment 完全由上游 schema 解析；`ocd` 不重新实现其继承规则。

`--env` 属于上游 Wrangler，`target` 因而只表示“请求发到哪个远程 open-compute”。P12 不使用 `context`，也不提供
`ocd context`、`--context`、`contexts.toml` 或兼容 alias；`--remote` 同样不作为 selector，避免与 Wrangler 自己的
`wrangler dev --remote` 形成位置相关的双重含义。

为避免在不同终端或 repository 中意外把命令发往 production，P12 不提供全局可变的“当前远程 target”。远程目标必须每次
传 `--target <name>`，或固定在项目的 package/CI script 中。本机唯一运行 instance 仍可按 P11 自动选择。

## 4. 命令面

```text
ocd target add <name> --api-base-url <url> --account-id <id> --token-file <path>
ocd target list [--json]
ocd target show <name> [--json]
ocd target test <name> [--json]
ocd target remove <name>

ocd wrangler [--target <name> | --instance <id> | --config <path>] [--project <dir>] <wrangler command> [arguments...]
```

`target add` 的 name 必须匹配 `[a-z][a-z0-9-]{0,31}`。`--target`、`--instance` 和 `--config` 三者互斥。
所有 `ocd` option 必须位于 Wrangler command 之前；第一个位置参数是 Wrangler command，从它开始的 argv 全部原样透传。
日常命令不需要额外的 `--`。只有 Wrangler argv 本身以 flag 开头（例如查看上游版本）时，才用标准的可选 `--` 消除歧义：
`ocd wrangler -- --version`。`--project <dir>` 同时指定 project-local Wrangler 的解析起点和 child cwd，方便从任意目录调用；
它不改变 target 选择。

例子：

```bash
# 本机唯一运行 instance
ocd wrangler deploy --env dev

# 本机多实例
ocd wrangler --instance k7m2r tail --env staging

# 远程目标；target 与 Wrangler environment 是两个独立概念
ocd wrangler --target company-prod deploy --env production

# 从项目目录外调用
ocd wrangler --target company-prod --project /srv/workers/billing deploy --env production

# Wrangler 自己的 flag 在 command 后直接透传
ocd wrangler --target dev-server deploy --config ./wrangler.jsonc
```

P11 的异步版本检查 pre-command hook 会在 `ocd wrangler` 选择目标前运行；它不读取、修改或记录 Wrangler argv。

## 5. 目标选择

`ocd wrangler` 按以下规则选择且只选择一个执行目标：

1. 显式 `--target <name>` 选择远程 target；
2. 显式 `--instance <id>` 选择本机 registry 中的 instance；
3. 显式 `--config <path>` 按 P11 派生并选择本机 instance；
4. 未传 selector 时，完整复用 P11 online 命令的 0/1/N 选择器；
5. 不会从 target registry 隐式选择远程 target。

多个本机 instance 运行且未显式选择时，返回 P11 的稳定歧义错误和完整短 ID 列表；不使用 Worker name、
`wrangler.jsonc` 或端口猜测 instance。本机没有可选 instance 时明确提示 `--target`，不自动发往 Cloudflare。

本机 instance 的 API base URL 只从已验证 instance descriptor/config 中的 admin listener 派生，不使用 tenant public
listener、Dashboard URL 或端口扫描。远程 target 同样必须指向运维者明确暴露并保护的 admin API origin。

## 6. 远程 target registry

target registry 是 per-user client configuration，不是 instance registry、`control.sqlite` 或 Worker project 数据。Linux 使用
`$XDG_CONFIG_HOME/open-compute/targets.toml`（未设置时为 `~/.config/open-compute/targets.toml`），macOS 使用
`~/Library/Application Support/open-compute/targets.toml`。它使用 bounded schema、atomic write、fsync 和 owner-only 权限。

每条记录只包含：

```text
schema_version
name
api_base_url
account_id
token_file
created_at
```

规则：

- remote URL 必须是 `https://<authority>/client/v4`；只有 loopback target 可使用 `http`;
- URL 拒绝 userinfo、query、fragment、非标准 path 和跨 origin redirect；
- `account_id` 按 P6 的公开 ID 语法校验；
- `token_file` 必须是 absolute、owner-owned、mode `0600`、no-follow 且 bounded 的普通文件；
- registry 只存文件 reference，不存 token value、admin token、Wrangler argv 或项目路径；
- `target show/list --json` 不读 token value，也不输出凭据摘要；
- `target remove` 只删除 target record，不删除外部 token file；
- 未知 schema、symlink、宽松权限、重名 target 和重复 `(api_base_url, account_id)` 都 fail closed。

`target test` 是显式网络命令：它读取 deployer token，调用 account discovery 和
`/client/v4/open-compute/capabilities`，验证 TLS、origin、account、token 认证与 target 公布的 Wrangler exact pin。实际
mutation 仍由服务端逐请求执行 deployer scope 授权。失败响应不回显 token、
Authorization header 或未清理的 URL。P12 不提供 `--insecure`。

## 7. `ocd wrangler` wrapper 合同

wrapper 是一个薄的 process launcher，不是 Wrangler adapter。执行顺序：

1. 选择唯一的本机 instance 或远程 target；
2. 解析并安全读取所选执行目标的 deployer token reference；
3. 从本机 instance descriptor 或远程 target 得到 API base URL 和 account ID；
4. 解析 project-local Wrangler executable 并读取其精确版本；
5. 用所选 instance/target 的 capabilities 验证该 Wrangler 版本是当前 open-compute release 认证的精确 pin；
6. 打印一行不含 secret 的执行目标 kind/name、origin、account 和 Wrangler version 摘要；
7. 构造子进程环境并原样传入从 Wrangler command 开始的 argv；
8. Unix 上使用 process replacement 保留 TTY、signal 和 exit status。

子进程必须收到：

```text
CLOUDFLARE_API_BASE_URL=<normalized API base URL ending in /client/v4>
CLOUDFLARE_API_TOKEN=<deployer token>
CLOUDFLARE_ACCOUNT_ID=<account id>
WRANGLER_LOG_SANITIZE=true
WRANGLER_SEND_METRICS=false
WRANGLER_SEND_ERROR_REPORTS=false
```

launcher 移除已继承的 `CLOUDFLARE_API_KEY`、`CLOUDFLARE_EMAIL` 以及 deprecated `CF_API_*` / `CF_ACCOUNT_ID` /
`CF_EMAIL`，避免另一套凭据或 base URL 参与 Wrangler precedence。它用所选执行目标覆盖同名的三个现代变量，
但不改写父 shell、项目 `.env` 或用户配置。

wrapper 不得：

- parse/rewrite `wrangler.jsonc` 或 Wrangler stdout/stderr；
- 翻译 Wrangler error、重试 mutation 或根据 command name 调用私有 API；
- 把 token 放入 argv、debug log、support bundle、child command preview 或 process title；
- 自动运行 `wrangler login`、读取 Cloudflare OAuth profile 或回退到 `api.cloudflare.com`；
- 在 Wrangler 缺失或版本不匹配时下载/更换 package。

直接使用环境变量的 Wrangler 仍是完整支持的底层接口；wrapper 是为人类开发者提供的安全便利层，不是新 transport。

## 8. Wrangler 解析与版本

Cloudflare 建议在项目内安装 Wrangler。P12 的解析顺序固定为：

1. 显式 `--project <dir>` 时 canonicalize 该目录，否则使用启动 cwd；
2. 从该目录向上查找最近的 `node_modules/.bin/wrangler`，不跨越 filesystem boundary；这同时支持普通项目和 hoisted
   monorepo workspace；
3. 找不到 project-local binary 时返回安装精确认证版本的提示，不隐式使用全局 `PATH`，也不自动运行 package manager。

项目 `package.json` 应使用精确版本并提交自身 package manager lockfile：

```json
{
  "devDependencies": {
    "wrangler": "4.127.1"
  }
}
```

`4.127.1` 是本文日期下 P6 认证的 pin；真实 authority 是所选 instance/target 的 capabilities 与当前 P6 contract，不是本文中
永久不变的数字。不接受 caret/tilde 或“同 major 应该可以”；Wrangler 的请求序列、multipart 和 schema 只按 P6 Gate 认证。

launcher 直接执行解析到的 project-local executable，不经过 `npx`、`bunx`、`pnpm dlx` 或 shell；因此不会触发隐式下载，
也不会因 package manager 不同而改变行为。解析 project-local executable 是开发者主动调用的开发工具行为，不进入 daemon startup、
systemd/launchd service 或 production runtime `PATH` 搜索。

## 9. 日常本地开发

最短、最快的编辑循环继续是：

```bash
wrangler dev
```

Worker 代码和默认 bindings 在 Wrangler/Miniflare 管理的本地 workerd 中运行；它不连接、启动或修改常驻 `ocd`。本地 secret
使用 gitignored `.dev.vars` 或上游支持的本地机制，不复制生产 deployer token。

多 Worker service binding 项目可使用上游多 `--config` 本地模式：

```bash
wrangler dev -c ./api/wrangler.jsonc -c ./auth/wrangler.jsonc
```

P12 不宣称本地模拟等于真实 open-compute 集成。对 SQLite recovery、Queues/Workflows scheduler、S3 backend、平台限制、
多 Worker 路由和真实 workerd loader 敏感的行为，必须进入第 10 节的真实 target 验证。

## 10. 真实 runtime 开发环境

open-compute Day 1 不实现 `wrangler dev --remote`。该模式依赖 Cloudflare preview infrastructure、临时路由和远程会话语义，
不是只把 `CLOUDFLARE_API_BASE_URL` 指向另一个 origin 就能得到的通用协议。上游已把完全 remote dev 标记为 legacy，
并建议优先本地执行。

真实 open-compute 开发使用专用 Wrangler environment 和 Worker name/resource：

```bash
ocd wrangler --target dev-server deploy --env dev
ocd wrangler --target dev-server tail --env dev
```

部署创建 immutable Version 和新的单 Version、100% Deployment，再原子改变 active routing；不重启 `ocd` daemon、不重建
instance registry，也不改写其他 Script。开发者通过 P6 endpoint 访问真实 Worker，通过 P7 `wrangler tail` 看 realtime log。

每次保存自动上传不是 Day 1 默认；它会快速累积 Version、触发资源 mutation 并使真实数据风险模糊。项目可在自身
package script 中增加显式的 watch/deploy 工具，但 `ocd` 不内建第二个 file watcher。

## 11. 部署、发布与回退

小型项目可直接：

```bash
ocd wrangler --target company-prod deploy --env production
```

需要显式 Version 控制时：

```bash
ocd wrangler --target company-prod versions upload --env production
ocd wrangler --target company-prod versions deploy <version-id>@100% --env production
```

P6 Day 1 只支持单 Version `100%` Deployment，不支持多 Version percentage rollout。回退也是创建一个指向旧 immutable Version 的
新 `100%` Deployment，不修改旧 Version 或数据库历史。

推荐项目流程：

1. 锁定并安装执行目标认证的 Wrangler pin；
2. 本地 typecheck/test 与 `wrangler deploy --dry-run`；
3. 对专用 dev/staging environment 执行真实部署、endpoint smoke 和 tail 检查；
4. 由 CI 使用 deployer token 向 production 上传已确定的 source/build input；
5. 保留新 Version/Deployment ID 和审计记录；
6. 失败时显式回退到已验证的旧 Version。

`wrangler.jsonc` 中的 environment 是项目级隔离，不是数据与故障的物理隔离。若 staging 与 production 不得共享 data-dir、
object authority、listener 或故障域，必须使用两个 P11 instance/两个远程 target，不能只依赖 `--env`。

## 12. Framework 和生成配置

framework adapter 仍使用 Wrangler 官方 `.wrangler/deploy/config.json` 跳转到生成的部署配置。`ocd wrangler` 默认从调用者 cwd
启动上游 binary；显式 `--project` 时从该目录启动。wrapper 不预解析 redirect，因此 Wrangler 自己的 upward discovery 和
generated-config 语义保持不变。

项目不生成 `open-compute.json`、不将 `compute.toml` 指向 build output，也不让 framework plugin 直接调用私有 upload endpoint。
自定义 build 只改变 Wrangler 的本地 input，最终 transport 仍是 P6 认证的上游 Wrangler。

## 13. 项目 scripts 与 CI

开发者可以把远程 target 名显式固定在 package script：

```json
{
  "scripts": {
    "dev": "wrangler dev",
    "deploy:dev": "ocd wrangler --target dev-server deploy --env dev",
    "deploy:prod": "ocd wrangler --target company-prod deploy --env production",
    "logs:prod": "ocd wrangler --target company-prod tail --env production"
  }
}
```

target name 不是 secret，可以进入版本控制；target registry 和 token file 不进入 repository。

CI 的基础合同仍是上游环境变量：

```bash
CLOUDFLARE_API_BASE_URL="$OPEN_COMPUTE_API_BASE_URL" \
CLOUDFLARE_API_TOKEN="$OPEN_COMPUTE_DEPLOYER_TOKEN" \
CLOUDFLARE_ACCOUNT_ID="$OPEN_COMPUTE_ACCOUNT_ID" \
wrangler deploy --env production
```

CI secret store 应只发放 deployer token，不使用 admin token。CI 必须锁定 Wrangler package/lockfile，打印非 secret 的执行目标 origin、account、
Wrangler version 和部署结果 ID。`ocd wrangler` 在 CI 中可选；自动化不需为了部署强制创建开发机 target。

## 14. 失败语义

- 执行目标不唯一、指定的 target 不存在、token file 不安全或 capabilities 不可达时，不启动 Wrangler；
- Wrangler 缺失或版本不是所选 instance/target 认证的精确 pin 时，输出检测到的路径/版本与期望版本，不自动修复；
- 所选 instance/target 返回 unsupported capability 时保留 Cloudflare-style error，wrapper 不删除 binding 或改写 config 后重试；
- Wrangler 退出后 `ocd` 返回同一 exit status；不把部分成功改写为成功；
- 请求中断不能推断服务端 mutation 已取消；用 Wrangler 列表/查询命令核对 Version、Deployment 和资源状态；
- target 失效不影响远程 daemon 运行，删除项目也不自动 remove target。

## 15. 安全边界

- 只把 deployer token 注入 Wrangler child；Dashboard/operator 的 admin token 不参与项目部署；
- child environment 是为兼容上游 Wrangler 所必需的短命传递边界，不因此允许 argv、文件或日志泄露；
- wrapper 不输出可复制的完整 child environment，`--json` 也不例外；
- remote target 要求受信 TLS；loopback HTTP 是唯一例外，不增加 `--insecure` fallback；
- wrapper 禁用 Wrangler telemetry/error reporting，避免把 self-hosted target metadata 发送给 Cloudflare；
- support bundle 可列出 target name 和经现有策略清理的 origin，但不读 token file 或采集 Wrangler `.env`/`.dev.vars`；
- `target test` 的 audit 只记录 target name/origin、Wrangler pin 和 request ID；`ocd wrangler` 在 process replacement 前只记录
  target 和 launcher 结果，不记录 opaque argv 全文、环境值或无法观测的 child exit class。

## 16. 代码与文档所有权

- `crates/core`：bounded target name、normalized API base URL 和 target descriptor 值类型；
- `crates/service`：target registry、P11 selector 复用、Wrangler resolution/version check、child environment 与 process replacement；
- P6 capabilities：继续是 Wrangler pin 和 endpoint support 的唯一服务端 authority；
- P7：继续拥有 `wrangler tail` wire/runtime 合同，P12 只启动该命令；
- `packages/docs`：开发、target、CI、staging/production 和回退文档；
- Worker project：拥有 `wrangler.jsonc`、package scripts、lockfile、local `.dev.vars` 和自身测试。

不新建 Rust Worker build crate、Node wrapper package、自定义 project manifest 或第二套 Cloudflare client。实现应直接复用 P11 的安全文件、
selector 和错误语义，并保持 target 与 instance registry 两个简单、不可互换的结构。

## 17. 实施顺序

### P12.1：远程 target 与执行目标选择

- 实现 target value types、registry 和 add/list/show/test/remove；
- 实现 URL、account、token-file 安全校验；
- 复用 P11 instance selector 并锁定三种 selector 的互斥规则。

### P12.2：Wrangler launcher

- 实现 command-first trailing argv，并保留仅用于 flag-first argv 的可选 `--`；
- 实现 `--project`、project-local resolution 与精确 pin 校验；
- 实现清理后的 child environment 和 Unix process replacement；
- 保留 TTY、signal、stdout/stderr 和 exit status。

### P12.3：项目与 CI DX

- 增加本地 dev、真实 dev/staging、production、tail 和回退 runbook；
- 增加 package scripts 和主流 CI 的无 secret 模板；
- 对固定 Wrangler 和真实 `ocd` 执行端到端 Gate。

## 18. 验收矩阵

最低回归覆盖：

- target name/URL/account/token-file 的正常与全部 fail-closed 路径；
- target registry 的 symlink、owner、mode、unknown schema、atomic write 和 crash recovery；
- `--target` / `--instance` / `--config` 互斥与 P11 0/1/N 本机选择；
- command 后未知 flag、Unicode、空值、Wrangler `--config` 和 `--cwd` 逐字节传递，并覆盖 flag-first argv 的可选 `--`；
- cwd/`--project`、nearest project-local Wrangler、hoisted workspace、missing binary、错误 version 和 capabilities pin mismatch；
- child 环境精确注入所选执行目标的三个 Cloudflare 值并移除冲突 legacy auth/base variables；
- token 不出现在 argv、stdout/stderr、debug log、JSON、audit、support bundle 或测试失败证据；
- TTY、SIGINT/SIGTERM、Wrangler exit code 和快速 child exit 的透传；
- 本地 `wrangler dev` 不访问 `ocd`，`wrangler dev --remote` 不被宣称支持；
- 真实 dev 部署在不重启 daemon 的前提下创建 Version/100% Deployment 并可访问；
- P7 `wrangler tail` 通过同一 target 收到该 Worker 的实时事件；
- 两个项目和两个 Wrangler environment 不串 Script、binding、secret、Version 或 Deployment；
- direct CI environment 和 wrapper 路径产生相同的官方 Wrangler wire behavior。

固定 Wrangler 的真实 Gate 必须使用已准备的正式 binary/runtime inputs，不在测试中即时下载。安全的 fake Wrangler 只用于校验
argv/environment/process semantics，不替代 P6/P7 真实 wire 和 runtime Gate。

## 19. Definition of Done

P12 只有同时满足以下条件才可移入 `docs/implemented/`：

1. 一个常驻 `ocd` 可被至少两个独立 Worker project 通过固定上游 Wrangler 安全部署；
2. local instance 与 remote target 的选择、互斥、失败和输出合同与 P11/P12 一致；
3. `ocd wrangler` 不解析项目配置或实现 transport，只完成目标、版本和 process environment 边界；
4. project-local Wrangler exact pin、generated config、environment、secret/resource 命令和 P7 tail 通过真实验收；
5. 日常 `wrangler dev` 保持上游本地流程，真实 integration 使用显式 dev/staging Deployment；
6. token 只从安全 reference 读取并且仅在 Wrangler child environment 中短命存在，所有泄露扫描通过；
7. CI 可不创建 target，直接使用标准环境变量和同一 Wrangler pin 部署；
8. 部署不重启 daemon，回退不修改 immutable Version，多项目/多环境不串 authority；
9. 中英文项目、target、CI、发布和故障处理文档与 CLI help 同步；
10. 静态检查、覆盖率和最终单轮 workspace Gate 按仓库政策通过，且无遗留 Wrangler/ocd 进程、listener 或 secret。

完成前，文档只能把 `ocd wrangler`、remote target 和上述项目 DX 标为 planned。当前已实现且可依赖的底层路径仍是 P6
记录的三个 Wrangler 环境变量和精确认证的上游版本。

## 20. 官方上游依据

- [Wrangler](https://developers.cloudflare.com/workers/wrangler/)
- [Wrangler commands](https://developers.cloudflare.com/workers/wrangler/commands/)
- [Wrangler configuration 与 generated config](https://developers.cloudflare.com/workers/wrangler/configuration/)
- [Wrangler system environment variables](https://developers.cloudflare.com/workers/wrangler/system-environment-variables/)
- [Wrangler environments](https://developers.cloudflare.com/workers/wrangler/environments/)
- [Workers local development 与 remote development](https://developers.cloudflare.com/workers/local-development/)
