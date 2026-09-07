# P13：面向用户的文档站重构

状态：Day 1 信息架构、内容合同与实施方案完成；待实施与验收。

日期：2026-09-07

本文定义 `https://open-compute.dev` 文档站的目标用户、信息架构、内容迁移、质量门禁和发布合同。P13 实施时，
[P11 ocd 安装、实例与本机运维体验](p11-ocd-operator-experience.md)与
[P12 Wrangler 项目开发与部署体验](p12-wrangler-project-workflow.md)均视为已经实现并通过验收；公开文档必须直接描述它们的
最终行为，不得继续展示旧的 `oc` wrapper、手工注入本机 token 或“planned P11/P12”过渡路径。

P13 的核心结论是：**文档站首先服务于在自己机器上安装 `ocd`、运行平台并部署应用的开发者；产品参考随后；架构与贡献指南最后。**
首页、导航、搜索结果和每条上手路径都必须体现这个优先级。

## 1. Review 范围与当前证据

本次 review 检查了 `packages/docs` 的全部中英文 Markdown、VitePress 配置、导航、首页、安装/CLI/产品页面、`llms.txt`、
站点发布配置，以及与 P11/P12 的命令合同。2026-09-07 的仓库状态为：

- 英文与中文各 115 个内容页，共 230 页，路径集合完全对称；
- 104 个页面不超过 25 行，大量产品都机械拆成 Overview / Get started / Concepts / Guides / Examples / Limits / Deviations；
- 46 个页面仍把 `oc`、`bun run oc deploy`、`oc build` 或 `oc types` 当作用户入口；
- 14 个页面把 `CLOUDFLARE_API_BASE_URL` 等原始环境变量作为首选上手路径；
- 首页首先展示产品分类和平台概念，安装与运维 `ocd` 被放在侧栏最后；
- 当前 sidebar 是对所有路由生效的一棵巨大产品树，不会随着“安装、开发、运维、参考、贡献”任务切换；
- 当前顶栏只有 Get started、Directory、`ocd`，没有独立的 Develop 和 CLI 入口；
- `ocd/get-started` 仍要求手工安装单个文件、生成配置再前台 `run`，没有以安装脚本、`ocd setup` 和 managed service 为主线；
- `ocd/deploy` 实际讲的是部署 daemon，却与部署 Worker 应用共用“Deploy”一词；
- CLI 页面已包含部分 P11 命令，但没有 P12 的 context、Wrangler launcher 和完整目标选择模型；
- 产品上手页大量从 raw v4 `curl` 或仓库内示例开始，要求用户先理解 account、token 和 API origin；
- 站点没有本地搜索、页面 frontmatter/description、last-updated、GitHub/edit 入口和专门的 contributor 区域；
- `public/llms.txt` 是手工维护的短目录，已经遗漏现有产品和新的首要用户旅程；
- `packages/docs/README.md` 的部署命令使用 `bunx wrangler deploy`，没有锁定为项目内 Wrangler；
- 当前 `/install.sh` 的公开交付与仓库 `scripts/install.sh` 之间没有站点构建期同一性检查。

这些问题不是单页措辞问题，而是用户、任务和 authority 排序错误。P13 不在旧 sidebar 上继续添加零散页面，而是重建站点骨架。

## 2. 用户与任务优先级

### 2.1 第一优先级：self-hosting developer

第一优先级用户不要求先区分“operator”和“application developer”。典型用户是一名负责一台服务器和若干 Worker 项目的开发者，
需要按顺序完成：

1. 判断 open-compute 是否适合自己的单机部署；
2. 安装或升级全局 `ocd`；
3. 用推荐配置完成 `ocd setup`，确认 service ready，并打开 Dashboard；
4. 在普通 Worker repository 中安装项目内 Wrangler；
5. 使用 `wrangler dev` 做快速本地开发；
6. 使用 `ocd wrangler` 部署、管理资源、查看日志和回退；
7. 理解 local instance、remote context 与 Wrangler environment 的区别；
8. 配置生产 listener、secret、Local/S3 object authority、备份和恢复；
9. 在 CI 中以稳定、无交互、无长期 admin token 的方式部署。

首页与主导航必须让以上每个任务在一次选择内可见。文档不要求用户先阅读架构、兼容矩阵或产品目录才能完成第一个部署。

### 2.2 第一优先级的第二条路径：团队应用开发者

这类用户通常不管理 daemon，只拿到一个 context、account 和 deployer credential。他们需要创建项目、本地开发、连接 dev/staging/prod、
声明 bindings、部署、tail 和排障。Develop 章节必须能独立服务这类用户，不把 systemd、SQLite 或 master key 混入日常应用开发路径。

### 2.3 第二优先级：评估、架构与贡献

评估者关心支持哪些 Cloudflare API、单机拓扑差异、limits、数据位置、安全边界和不支持项；贡献者需要架构、repository layout、
源码构建、测试、workerd pin、代码规范、发布与安全政策。Products、Reference 与 Project 服务这类任务，可以从顶栏和页脚访问，
但不抢占首页主流程，也不能混进安装 `ocd` 或部署应用的步骤。

## 3. 内容基线：P11/P12 已实现

P13 重写后的站点必须把以下行为写成当前产品事实：

- `ocd` 通过正式安装脚本安装到全局 bin，并由 `ocd upgrade` / `ocd uninstall` 管理；
- `ocd setup` 提供交互式流程，`ocd setup --yes` 使用推荐配置并直接注册、enable、启动 managed service；
- 当前目录 `compute.toml`、系统 `/etc/open-compute/config.toml` 和显式 `--config` 按 P11 的固定优先级工作；
- 本机实例具有 4–5 位、无前缀、由 canonical config path 派生的稳定 ID；
- `ocd instances/status/start/stop/restart/logs/dashboard` 在任意目录遵循同一实例选择规则；
- `--instance` 与 `--config` 互斥，显式 ID/路径优先，0/1/N 运行实例按 P11 返回确定结果；
- `ocd dashboard` 使用一次性 login code 打开正确实例，不让用户复制长期 admin token；
- 每条有效 CLI 命令执行前使用冷却缓存完成非阻塞升级提醒，daemon startup 保持离线；
- Worker 项目只使用标准 `wrangler.jsonc` 和项目内精确版本 Wrangler；
- `wrangler dev` 是快速本地循环，`ocd wrangler` 是面向真实 open-compute target 的安全 launcher；
- `ocd wrangler deploy --env dev` 不要求多余的 `--`；从 Wrangler command 开始的 argv 原样透传；
- `ocd wrangler --project <dir> ...` 可从任意目录选择项目；
- local instance、remote context 和 Wrangler environment 是三个独立概念；
- context 显式选择远程 API/account/deployer credential，站点示例使用 `dev-server`、`company-prod` 等不与 environment 混淆的名称；
- CI 可以直接使用标准 Cloudflare 环境变量，也可以在合适的 runner 上使用 `ocd wrangler`，但不依赖开发机隐式状态。

P14 Artifacts、P15 Browser Run 以及其他未完成能力仍按各自状态呈现。P13 不因重写文档而把后续设计描述成已发布能力。

## 4. 导航与路由总图

顶栏固定为：

```text
Get started | Develop | Operate | CLI | Products | Reference | Project ▾ | Language | Search
```

中文对应：

```text
开始使用 | 开发应用 | 运行与运维 | CLI | 产品 | 参考 | 项目 ▾ | 语言 | 搜索
```

VitePress 改为 route-scoped sidebars。每个一级区域只展示自己的任务树和一小组相关跨区链接，不再让每个页面都承载完整产品树。

| 一级入口 | 主要问题 | 目标路径 |
| --- | --- | --- |
| Get started | 如何从空机器到第一个可访问的 Worker？ | `/get-started/` |
| Develop | 如何创建、开发、部署、调试应用？ | `/develop/` |
| Operate | 如何安装、配置、运行、升级和恢复 `ocd`？ | `/operate/` |
| CLI | 某个命令、selector、输出和退出码如何使用？ | `/cli/` |
| Products | Worker、KV、D1、R2 等具体能力怎么用？ | `/products/`，保留现有产品 canonical URL |
| Reference | 配置 schema、兼容性、limits、API 与路径合同是什么？ | `/reference/` |
| Project | 架构、贡献、源码构建、测试和发布如何工作？ | `/project/` |

`/zh/**` 保持完全相同的路径集合。语言切换必须落到当前页面的另一语言版本，而不是回到首页。

## 5. 首页与 Get started

### 5.1 首页

首页不是产品百科或架构 README。首屏只回答：它是什么、适合谁、下一步是什么。

推荐信息顺序：

1. 一句话价值：在自己的单机上运行 Cloudflare Workers-compatible 应用；
2. 主 CTA：Install open-compute；次 CTA：Deploy an app；第三链接：See compatibility；
3. 四步成功路径：安装 `ocd` → `ocd setup --yes` → 项目安装 Wrangler → `ocd wrangler deploy`；
4. “开发”和“运维”两条任务入口；
5. 当前支持的核心产品，用简单的 supported/partial/not available 标签，不用主观百分比进度条；
6. 单机模型和关键边界的短说明；
7. 架构、GitHub、贡献入口放在页面后半部。

首页命令必须能从发行产物和一个普通空目录执行，不得要求 clone open-compute repository、Rust、Bun workspace 或内部 `examples/` 路径。

### 5.2 `/get-started/` 的唯一黄金路径

Get started 是一个可在十分钟内完成的端到端教程，而不是链接目录：

1. 支持平台与前置条件；
2. 一行安装与“下载后审阅再执行”的等价安全流程；
3. `sudo ocd setup --yes`（或明确的 macOS/user-scope variant）；
4. `ocd status` 与 readiness 成功的预期输出；
5. `ocd dashboard`；
6. 创建最小 TypeScript Worker project；
7. 安装 target 要求的项目内精确 Wrangler；
8. `wrangler dev` 本地验证；
9. `ocd wrangler deploy` 部署；
10. curl 访问 Worker、`ocd wrangler tail` 查看日志；
11. 下一步分别链接到 Develop、Operate 和 Products。

每一步写出成功信号和失败时的唯一下一跳。默认路径使用一个本机唯一实例，不在黄金路径提前引入 `--instance`、`--context`、S3、
手工 token 或 raw v4 API；这些概念在用户已经成功部署后再展开。

## 6. Develop：应用开发专章

新增顶级 `/develop/`，并按开发工作流排序：

| 页面 | 内容 |
| --- | --- |
| `/develop/` | 本地循环、真实 target、部署与 CI 的总览 |
| `/develop/create-a-project` | 标准 `package.json`、精确 Wrangler、`wrangler.jsonc`、TypeScript Worker 最小项目 |
| `/develop/local-development` | `wrangler dev`、`.dev.vars`、本地 bindings、何时必须上真实 target |
| `/develop/targets` | local instance、remote context、Wrangler environment 的区别和选择表 |
| `/develop/deploy` | `ocd wrangler deploy`、`--project`、多实例、远程 context、预期输出 |
| `/develop/resources-and-bindings` | 用 Wrangler 创建资源、写 binding、生成类型、验证 account scope |
| `/develop/variables-and-secrets` | local vars、deployer credential、Worker secret 的不同生命周期 |
| `/develop/logs-and-debugging` | `tail`、Dashboard、request ID、部署失败与 runtime failure 分流 |
| `/develop/versions-and-rollbacks` | immutable Version、100% Deployment、验证与显式回退 |
| `/develop/environments` | dev/staging/production 的项目组织与隔离边界 |
| `/develop/frameworks` | standard generated Wrangler config、framework adapter、monorepo/hoist |
| `/develop/ci-cd` | exact pin、lockfile、标准环境变量、最小权限 deployer token、非交互部署 |

Develop 页面以 `ocd wrangler` 和项目内 Wrangler 为主，不以 raw `curl` 为首选。产品资源页中的创建命令也优先展示：

```bash
ocd wrangler deploy --env staging
ocd wrangler tail --env staging
```

资源创建页面的命令名必须以 P6 认证的固定 Wrangler 实际语法为准，实施时不得凭印象补造命令。

从项目目录外调用统一使用：

```bash
ocd wrangler --project /srv/workers/billing deploy --env production
```

常规文档不显示 wrapper 分隔符。只有解释 flag-first argv 时才展示 `ocd wrangler -- --version`。

## 7. CLI：独立命令参考

新增顶级 `/cli/`。它不是一篇不断增长的长页面，而是稳定的命令索引与语义页面：

| 页面 | 内容 |
| --- | --- |
| `/cli/` | 命令分组、语法规则、如何使用 `--help` |
| `/cli/target-selection` | config discovery、0/1/N instance、selector 优先级、互斥规则 |
| `/cli/setup` | interactive/`--yes`、system/user scope、输出与失败边界 |
| `/cli/service` | `start`、`stop`、`restart`、`status`、`logs` |
| `/cli/instances` | `instances`、稳定短 ID、registry、`instance remove` |
| `/cli/dashboard` | target 选择、一次性登录、`--no-open` |
| `/cli/contexts` | add/list/show/test/remove、credential file、安全限制 |
| `/cli/wrangler` | project-local resolution、`--project`、argv 透传、target 环境注入 |
| `/cli/config` | `config init/check` 与 `compute.toml` / system config |
| `/cli/diagnostics` | `doctor`、`capabilities`、`docs`、`licenses`、support bundle |
| `/cli/backup` | create/list/inspect/delete/retention/restore 命令索引，链接到操作指南 |
| `/cli/releases` | update reminder、`upgrade`、`uninstall` |
| `/cli/output-and-exit-codes` | stdout/stderr、`--json`、稳定错误 code、TTY 和 child exit status |

每个命令条目至少包含 synopsis、是否需要 config/online instance、selector、是否联网、是否修改状态、关键输出、退出行为和一个最小示例。
CLI help 是参数 authority；站点补充任务语义和安全边界，不复制一份可能漂移的 parser 规则。

命令页面必须明确区分：

- global：help/version/docs/licenses/instances/setup/upgrade/uninstall；
- config-bound：config check、capabilities 和其他读取配置的命令；
- online instance：status/stop/restart/logs/dashboard；
- offline exclusive maintenance：backup/restore/recovery；
- developer launcher：context 与 `ocd wrangler`。

## 8. Operate：安装与运行平台

将当前 `/ocd/` 重构为 `/operate/`，避免让首次用户先理解二进制名，也避免把 daemon deployment 和 app deployment 都叫 Deploy。

| 页面 | 内容 |
| --- | --- |
| `/operate/` | 单机 ownership 模型与日常运维地图 |
| `/operate/install` | install.sh、校验、支持平台、安装位置 |
| `/operate/setup` | 推荐 setup、交互选项、system/user scope |
| `/operate/configuration` | 配置任务指南；完整字段转到 Reference |
| `/operate/instances-and-services` | systemd/launchd、多个实例、start/stop/restart/status/logs |
| `/operate/network-and-auth` | public/admin listener、三类 token、TLS 与最小暴露 |
| `/operate/storage` | Local 默认、S3 选择、authority 不可静默切换 |
| `/operate/dashboard` | 打开、登录、安全模型和常见问题 |
| `/operate/health-and-monitoring` | live/ready、doctor、metrics、何时重启 |
| `/operate/upgrade-and-uninstall` | dry-run、重启影响、receipt、保留数据 |
| `/operate/backup-and-restore` | 备份、验证、保留、恢复演练 |
| `/operate/incidents/` | 现有 runbook，按症状与风险组织 |

旧 `/ocd/deploy` 不保留同名正文。daemon 使用“Install / Run as a service”，应用使用“Deploy an app”。页面标题、搜索关键词和链接
都不得再让两种 deployment 混淆。

## 9. Products 与 Reference 的收敛

### 9.1 产品页

保留 `/workers/`、`/kv/`、`/d1/`、`/r2/` 等 canonical URL，避免把开发者熟悉的产品名藏进平台内部模块。但取消每个产品固定复制
七个页面的要求。每个产品只在内容足够时拆页，默认结构是：

1. 产品概览：适用场景、最小 Worker 示例、如何创建资源和绑定；
2. 使用指南：围绕真实任务组织，而不是空泛 Concepts/Guides；
3. API/limits/deviations：只有内容足以独立检索时拆分，否则在概览内成节；
4. 下一步指向 Develop 的通用部署、secret、logs、environment 和 CI 页面，避免每个产品重复相同命令。

现有 25 行以下页面逐个判定：保留并扩写、合并到产品概览、或删除并加精确 redirect。不得为了 sidebar 对称保留空壳。

### 9.2 Reference

新增 `/reference/` 并集中稳定合同：

- `compute.toml` 完整字段、默认值、范围、secret 类型、restart requirement；
- Wrangler project config 的支持范围与固定版本；
- Cloudflare v4 API reference；
- compatibility、deviations、limits、unsupported；
- filesystem paths、registry/context/cache 位置和权限；
- authentication roles；
- health/metrics/error/exit-code schema；
- release/runtime identity。

当前 Platform 页面中的主观 `90%`、`95%`、`80%` 进度条移除。公开状态只使用有可验证含义的 `Supported`、`Partial`、`Not available`、
`Preview`，并链接到精确限制或 capabilities。未实现的 roadmap 不占用产品导航；必要时在 Unsupported/Roadmap 中明确标记。

## 10. Project：架构与贡献者内容

新增 `/project/`，作为第二优先级之外的独立入口：

| 页面 | Authority |
| --- | --- |
| `/project/architecture` | 单进程、crate ownership、workerd child、SQLite/ObjectBackend、请求路径 |
| `/project/repository` | `crates/`、`packages/`、`test/`、`docs/`、`third_party/workerd` |
| `/project/build-from-source` | Rust/Bun/Git LFS、显式 runtime asset build，不混入用户安装 |
| `/project/testing` | 链接并摘要 `docs/references/testing.md` 的单轮 Gate 政策 |
| `/project/contributing` | 贡献流程、范围选择、代码/文档要求、PR 前检查 |
| `/project/workerd` | pin、fork、submodule 与兼容边界 |
| `/project/security` | security boundary、漏洞报告入口、secret 禁止项 |
| `/project/releases` | 发布模型与 contributor release 流程 |

README 可以继续承担项目介绍，但公开 docs 不直接把 `AGENTS.md` 当作贡献者入门页。Project 页面摘要稳定规则并链接仓库 authority；
用户安装页永远不要求阅读 Project 区域。

## 11. 命令、术语与写作合同

### 11.1 唯一产品命令模型

面向用户的所有页面遵循：

```text
安装/服务/运维       ocd ...
快速本地应用开发     wrangler dev
真实 target 操作     ocd wrangler ...
CI                    项目内 wrangler + 标准环境变量，或显式 ocd wrangler
```

公开站点删除 `oc deploy`、`oc run`、`oc build`、`oc types` 和 `bun run oc ...` 的用户路径。若 repository 内部 toolchain 尚有用途，
只在 Project contributor 文档描述，并明确不是发行产品 CLI；不得保留 legacy alias 教程。

### 11.2 固定术语

| 术语 | 只表示 |
| --- | --- |
| instance | 本机、由 config path 派生 ID 的 `ocd` 实例 |
| context | 开发机保存的远程 API/account/credential 别名 |
| environment | `wrangler.jsonc` 中由 `--env` 选择的项目配置 |
| platform config | `compute.toml` 或系统 `config.toml`，配置 `ocd` |
| project config | `wrangler.jsonc`，配置 Worker application |
| install/run `ocd` | 部署平台二进制和 service |
| deploy app | 上传 Version 并创建 Deployment |

英文正文首次出现时解释名词；中文保留必要的 CLI/Cloudflare 专有词，但不用中英混杂代替解释。

### 11.3 页面模板

任务型页面按需包含：目标、适用对象、前置条件、步骤、预期结果、验证、回滚/清理、下一步。Reference 页面按需包含：syntax/schema、
default、required、mutability、security、failure、related commands。不是每页机械填满所有栏目，但任何 mutation 教程都必须给出验证和恢复路径。

代码块默认可复制：不使用仓库私有 cwd、虚构 token、过期日期、弯引号或省略关键参数；placeholder 使用一致的 `<...>`。

## 12. 双语、搜索与页面体验

- 英文仍是 `/` 默认语言，中文为 `/zh/`；一级路由和页面集合必须严格对称；
- 同一 PR 更新所有受影响语言；缺少翻译时不发布英文-only 空壳，也不静默跳回另一语言；
- VitePress 启用本地全文搜索，不把查询或访问记录发送给第三方；
- 搜索同义词覆盖 install/setup/service/daemon、deploy app、instance/context/env、配置/部署/实例/环境；
- landing page 增加明确 `title`、`description`、canonical 和语言 alternate metadata；
- 启用 last-updated，配置 repository/edit link 和报告文档问题入口；
- 增加面向任务的 404：提供 Install、Develop、CLI、Products 和搜索入口；
- 代码块语言、heading 层级、表格和链接文本满足键盘与屏幕阅读器基本可访问性；
- 不为了视觉效果引入独立前端应用。优先使用 VitePress 默认主题、少量可维护 CSS 和原生 local search。

## 13. 内容 authority 与防漂移

P13 不建立第二套产品事实数据库。authority 固定为：

| 事实 | Authority | 站点策略 |
| --- | --- | --- |
| CLI 参数与 help | Rust Clap command tree | CLI reference 补充语义；CI 核对 command/option inventory |
| 默认配置与字段 | `share/default-config.toml` + Rust config types | Reference 表与示例必须通过 config check fixture |
| Wrangler 精确 pin | root dependency catalog + P6 capabilities | 扫描站点出现的版本并要求全部匹配 |
| 产品/limits | capabilities schema + compatibility reference | 生成/校验摘要，不手写百分比 |
| install script | `scripts/install.sh` | 站点 `/install.sh` 必须逐字节来自同一文件或构建时验证一致 |
| incident procedure | `docs/references/runbooks/` | 站点写任务入口并同步命令；不修改历史结果报告 |
| API | P6 route inventory/OpenAPI | Reference 从同一 inventory 生成或校验 |

增加一个小而直接的 docs validation 入口，至少执行：

1. VitePress production build 和 dead-link failure；
2. 英文/中文相对路径集合一致；
3. 禁止公开页面出现 `bun run oc`、旧 config 文件名和已退役命令；
4. Wrangler SemVer 与 root catalog 一致；
5. 导航中的每个路径存在，所有内容页可从 sidebar、页面链接或 sitemap 到达；
6. 关键 quickstart command fixture 与真实 CLI help/config parser 一致；
7. `/install.sh` 与仓库 authority 一致且能被部署配置公开访问；
8. `llms.txt` 链接存在并覆盖 Get started、Develop、Operate、CLI、Products、Reference；
9. Markdown 中没有把 planned/unsupported capability 写成 supported 的已知状态冲突。

只生成适合机械维护的 inventory/索引，不自动生成用户教程正文。生成输出必须有明确 source、可复现并在 CI 中 diff-check；不在 VitePress
运行时调用网络或要求启动 `ocd`。

## 14. URL 迁移与 redirect

保留现有产品 canonical URL。以下用户任务 URL 使用精确 301：

| 旧路径 | 新路径 |
| --- | --- |
| `/get-started` | `/get-started/` |
| `/directory` | `/products/` |
| `/ocd/` | `/operate/` |
| `/ocd/get-started` | `/operate/install` |
| `/ocd/configuration` | `/operate/configuration` |
| `/ocd/deploy` | `/operate/instances-and-services` |
| `/ocd/health` | `/operate/health-and-monitoring` |
| `/ocd/backup` | `/operate/backup-and-restore` |
| `/ocd/cli` | `/cli/` |
| `/ocd/incidents/**` | `/operate/incidents/**` |

中文使用完全对称的 `/zh/...` redirect。只为已经公开的有限路径保留 redirect，不建立通用猜测 rewrite，也不长期维护旧内容副本。
每个 redirect 的目标在构建测试中验证，不能形成链或循环。

## 15. `llms.txt`、SEO 与站点发布

- 从 route inventory 生成或校验 `llms.txt`，首序为 Get started、Develop、Operate、CLI，而不是产品字母表；
- 可增加 bounded 的 `llms-full.txt`，仅汇总当前公开用户文档，不混入内部 plan、历史结果或 secret 示例；
- sitemap、canonical、locale alternate 使用 `https://open-compute.dev`；
- 未实现 capability 页面必须有明确状态，不能以 SEO landing page 形式暗示可用；
- `packages/docs` 自己声明项目内精确 Wrangler，并提供 `deploy` script；部署文档使用 `bun run deploy`，不使用 `bunx`/`npx` 动态解析；
- 站点部署只上传已构建静态资产；build 不访问运行中的 open-compute，不读取 operator token；
- VitePress `dist`/cache 保持未跟踪，正式发布检查 `_redirects`、`install.sh`、404、sitemap 和关键页面响应。

## 16. 实施顺序

### P13.1：事实清理与黄金路径

- 冻结 P11/P12 已实现后的命令 inventory；
- 重写中英文首页和 `/get-started/`；
- 删除公开页面中的 `oc` 和手工本机 token 首选路径；
- 让安装、setup、status、dashboard、项目内 Wrangler、local dev、deploy、tail 形成一条可执行链；
- 发布并验证与 `scripts/install.sh` 相同的 `/install.sh`。

### P13.2：信息架构与核心章节

- 实现 route-scoped sidebar、顶栏、本地搜索和新 404；
- 建立 `/develop/`、`/operate/`、`/cli/`；
- 将当前 `/ocd/**` 内容按任务迁移并增加 redirects；
- 完成 context/instance/environment、`--project`、service lifecycle、升级/卸载和 Dashboard 文档。

### P13.3：产品与 Reference 收敛

- 审查每个短页面，合并空壳并保留有用 canonical product URL；
- 把通用部署/secret/environment/log/CI 内容去重到 Develop；
- 建立完整 config、compatibility、limits、auth、path、exit-code Reference；
- raw v4 curl 下沉到 API/reference，不再作为产品首选上手方式。

### P13.4：Project 与持续质量

- 建立架构、贡献、源码构建、测试、安全、workerd、发布页面；
- 加入双语 parity、CLI/config/pin、stale command、route reachability、install script 和 `llms.txt` 检查；
- 更新 README 与所有仓库入口，使用户首先到公开 Get started，贡献者到 Project；
- 完成 production build、链接检查和真实 quickstart 文档冒烟。

## 17. 验收矩阵

最低验收覆盖：

- 新用户从首页最多一次选择进入安装、开发应用、CLI 或产品目录；
- 在支持的干净 Linux/macOS host 上，只按 Get started 可完成 install → setup → ready → dashboard → first Worker deploy；
- quickstart 不依赖 repository checkout、Rust toolchain、root Bun workspace、内部 example path 或手工复制 admin token；
- 唯一本机实例的常规开发命令为 `ocd wrangler deploy`，无强制 `--`；
- 多实例错误展示短 ID，文档能引导到 `--instance`；远程部署明确使用 `--context`；
- `--context company-prod` 与 `--env production` 在内容、示例和 glossary 中始终保持不同概念；
- 从其他 cwd 使用 `--project` 能找到 project-local/hoisted Wrangler，站点不建议隐式全局 Wrangler；
- Develop 覆盖 create/local dev/real target/resources/secrets/logs/rollback/environments/frameworks/CI；
- CLI 覆盖 P11/P12 全命令、selector、联网/变更属性、JSON/exit status；
- Operate 覆盖 install/setup/config/service/instances/dashboard/network/storage/health/upgrade/backup/incidents；
- public docs 中 `oc` 旧命令、旧配置名和相互矛盾的安装流程为零；
- 所有 Wrangler version 文本与 authority pin 一致；
- 所有 advertised capability 能映射到 capabilities/compatibility authority，后续 P14/P15 不被误写为 available；
- 英文/中文路径、导航层级和关键命令语义完全对称；
- 现有 public product URL 可访问，迁移 URL 只经过一次 301 到有效目标；
- local search 能以 install、setup、deploy、instance、context、config、backup 及中文同义词找到首选任务页；
- 每个页面从一个 sidebar、landing page、搜索 inventory 或 redirect 可达，不存在 orphan page；
- `llms.txt` 无死链并按用户任务优先；
- docs build 不访问网络、不读取 secret、不启动 daemon，不提交 `.vitepress/dist`；
- `git diff --check`、docs validation 和 production VitePress build 通过。

## 18. Definition of Done

P13 只有同时满足以下条件才可移入 `docs/implemented/`：

1. 首页和导航明确把安装/配置 `ocd` 与开发/部署应用放在架构和贡献之前；
2. `/develop/` 与 `/cli/` 是完整、可搜索、双语的独立章节，不是单页占位；
3. Get started 已在干净支持主机按正式发行物端到端执行，记录命令、版本、目标与成功结果；
4. 公开用户路径全部使用 P11/P12 最终 CLI，旧 `oc` 和临时环境变量流程已经删除；
5. Operate、Products、Reference、Project 的 ownership 清晰，daemon deployment 与 app deployment 不再混名；
6. 104 个现有短页面均经过保留/扩写/合并/redirect 的显式判定，不再以目录对称制造空壳；
7. CLI/config/Wrangler pin/capabilities/install script/双语/链接/route 的自动检查进入常规 docs Gate；
8. 中英文 production build、local search、redirect、404、sitemap、`llms.txt` 与移动端导航完成验证；
9. README、站点发布说明和仓库内所有用户入口指向同一黄金路径；
10. 实施完成后的设计文档移入 `docs/implemented/`，持续维护规则进入 `docs/references/` 或站点 contributor 文档。

完成前，本文只定义 P13 目标。当前 `packages/docs` 仍包含 review 中列出的旧命令和信息架构，不能仅通过修改导航或状态标签宣称完成。
