# Cloudflare Worker Runtime 全量兼容目标

状态：Active，2026-08-30。本文定义 open-compute 当前兼容目标；它覆盖目标能力，不表示这些能力
已经实现。当前实现状态仍以
[`references/cloudflare-compatibility.md`](references/cloudflare-compatibility.md)、机器可读 capability、
contract catalog 和实际 Gate 结果为准。

## 1. 结论

open-compute 的目标是在一个 `platformd`、一个受监督的 stock `workerd`、本地 SQLite 和一个
S3-compatible object store 上，提供以下 Cloudflare tenant Worker 编程面：

- Workers runtime；
- Durable Objects；
- Queues；
- Workflows；
- R2；
- D1；
- KV。

“全量兼容”在本文中有严格含义：对固定上游版本中属于上述范围的**全部 stable TypeScript API**，
open-compute 必须提供相同的类型签名、运行时成员、成功/失败行为和单机可观察副作用。不能只实现常用
方法，不能用产品名、一个 smoke test 或 TypeScript 中存在同名 interface 推导兼容。

本目标只覆盖 tenant Worker 内使用的 runtime 和 binding API。Cloudflare account/resource management
API 不在目标内；详见第 5 节。

## 2. 类型是上游产物，不是本项目重新设计的接口

### 2.1 权威来源

类型真值按以下顺序确定：

1. 固定版本的
   [`@cloudflare/workers-types`](https://www.npmjs.com/package/@cloudflare/workers-types) stable 入口；
2. 与 production `workerd` pin 对应的 upstream generated types、`defines/*.d.ts` 和生成脚本；
3. Cloudflare 官方 runtime/binding 文档，用于解释行为、限制和类型无法表达的语义；
4. 同一版本 workers-sdk 的 `wrangler types` 生成行为，用于验证 `Env` 和 RPC 类型组合；
5. 真实 stock workerd 和受控 Cloudflare differential 的可观察行为。

Cloudflare 官方说明 Workers API 类型直接从 workerd 生成，并推荐应用使用 `wrangler types`；
`@cloudflare/workers-types` v5 的 stable 入口表示最新 runtime 类型：

- <https://developers.cloudflare.com/workers/languages/typescript/>
- <https://developers.cloudflare.com/workers/runtime-apis/rpc/typescript/>

若 npm 类型、匹配 revision 的 workerd generated types 和官方文档发生冲突，先阻塞版本更新并确认
上游事实；不得在 open-compute 中手写一个“折中”接口。

### 2.2 消费方式

默认方案是让 tenant 工具链直接消费固定版本的 `@cloudflare/workers-types`。允许的等价方案只有：

- 从正式 pin 对应的 workerd 源码可复现生成、且经过 digest/AST 对比证明等价的 upstream 类型产物；
- 由 `wrangler types` 或等价的本地 generator 生成 `Env`、Service RPC 和 Durable Object stub 组合类型。

`@open-compute/workers-types` 不再拥有 Cloudflare API 定义。若保留该 package，它只能：

- 引用或重新导出固定的 upstream 类型产物；
- 生成当前 Worker 的 `Env` binding 成员；
- 在明确的 `open-compute:*` module 下声明本平台专有类型。

它不得重新声明或裁剪 `Request`、`Response`、`ExecutionContext`、`KVNamespace`、`R2Bucket`、
`D1Database`、`DurableObjectState`、`Queue`、`Workflow` 等 Cloudflare 类型，也不得复制部分 overload、
修改返回值、用 `unknown`/宽泛 index signature 掩盖未实现方法，或通过自有 alias 制造第二套 API。

完整 upstream 类型中出现某个其他 Cloudflare 产品的 type name，不等于 open-compute 注入了该 binding。
能力广告由实际 `Env` binding、runtime availability、capability catalog 和 Gate 共同决定，不能再以
“类型名称不存在”作为 unsupported 产品的主要保证。`@cloudflare/workers-types/experimental` 默认不在
兼容目标内；只有用户另行扩大范围时才允许进入 baseline。

### 2.3 类型验收

类型 Gate 至少验证：

1. npm/workerd 类型版本、源码 revision 和 digest 与 formal runtime pin 一致；
2. upstream stable declarations 未被手工改写；若使用生成产物，生成两次字节一致；
3. 七项产品和 Workers runtime 的 interface、class、type、module、方法、overload、generic、参数、
   返回值和 readonly/optional 属性均与 upstream AST 一致；
4. `Env` 只包含本 deployment 实际声明且经 authority 验证的 binding；
5. upstream compile fixtures 和本项目每项 binding compile fixture 都通过；
6. runtime 尚未实现的 upstream stable API 记为 `blocked`，不能从类型中删除后称为兼容。

只用正则确认 interface 名称存在，不构成类型兼容证据。

## 3. 单一 latest runtime 语义

### 3.1 不提供 compatibility date 模式选择

open-compute 不支持 tenant 选择历史 `compatibility_date`，也不维护 date range、旧行为模拟、按日期
分支或旧 flag 组合。所有 tenant Worker 使用同一个平台 runtime contract：**正式 pin 在本次上游
同步时能提供的最新 stable Cloudflare 行为**。

workerd 配置仍需要一个 date 才能编译 Worker。该值改为 formal runtime lock 中的内部构建输入，
本文称为 `effectiveCompatibilityDate`：

- 不出现在 tenant project schema、deployment request 或公开 capability 的 min/max 区间中；
- 必须是 pinned workerd 接受的最新非未来 stable date；
- production startup 只读取随二进制构建的固定值，不联网计算“今天”或自动升级；
- 所有 tenant deployment 和动态 Worker 都使用同一个值；
- system Worker 可因宿主实现使用独立的内部 flags，但不能改变 tenant 可见合同。

tenant 也不获得 `compatibility_flags` 开关。属于 latest stable contract 的能力由平台内部按匹配
workerd release 的要求启用；历史 opt-out、experimental opt-in 和用户自定义 flag 组合不在目标内。
如果某个 stable API 仍需内部 flag 才能由该 release 启用，该 flag 进入 formal runtime lock 和 Gate，
而不是进入 tenant metadata。

### 3.2 “latest”必须可复现

“对齐当前最新”不是运行时浮动依赖。每次 coordinated dependency update 才重新解析 latest：

1. 查询 Cloudflare 官方 runtime、产品文档、compatibility flags 和 changelog；
2. 选择当时最新 stable `@cloudflare/workers-types`；
3. 选择能够提供相同 stable surface 的正式 workerd release；
4. 固定 `effectiveCompatibilityDate`、必要内部 flags、workers-types version、workerd revision、
   workers-sdk revision 和全部 digest；
5. 对 upstream types、docs、workerd tests 和当前 catalog 做显式 diff；
6. 补齐新增 API/runtime 行为后再完成类型、产品、恢复和 differential Gate；
7. 一次更新整体提交，不保留旧/new date 双路径。

生产启动不得下载类型、workerd 或文档。旧 deployment 在同一 Day1 authority 中随平台升级使用新的
唯一 runtime contract；本项目不为旧 open-compute deployment 保留历史 Cloudflare date 语义。

### 3.3 当前审计快照

截至 2026-08-30：

- npm registry 显示 stable 最新版本为 `@cloudflare/workers-types@5.20260830.1`，对应
  `gitHead=e9dda5963aba7ee4323960db795690ec78fec118`；`workerd` npm 最新版本为
  `1.20260830.1`；
- 本仓库 formal pin 仍是 `workerd v1.20260826.1`，本地 workers-types baseline 是
  `5.20260826.1`，tenant date 还存在 `2022-01-01..2026-08-26` 范围与 flag allowlist。

因此当前仓库尚未达到本文的 single-latest 模型。以上版本号是本次审计证据，不是永久常量；下一次
更新以新的固定 baseline 为准。

## 4. 产品兼容边界

下表不复制上游 TypeScript 签名；具体成员和 overload 永远由第 2 节的 upstream 类型产物所有。

| 领域 | 必须兼容的 Worker 编程面 | 可接受的单机差异 |
| --- | --- | --- |
| Workers runtime | stable globals、Web APIs、Fetch、Streams、WebSocket、Crypto、HTMLRewriter、Cache API、scheduled、TCP sockets、RPC、ExecutionContext、module/handler、Service/Static Assets/Version Metadata tenant surface、latest default Node.js surface，以及七项产品需要的 module imports | 无 Anycast/colo/edge placement；无法真实提供的 edge metadata 必须有文档化、稳定且不泄密的本地语义 |
| KV | 完整 `KVNamespace` stable surface，包括单键/批量 overload、metadata、stream、list 和 cache-status shape | 单节点强一致替代全球 eventual consistency/edge cache；方法 shape、限制与错误仍须兼容 |
| R2 | 完整 Worker `R2Bucket`/object/body/multipart/options/checksum/SSE-C/storage-class surface | object bytes 由配置的 S3-compatible provider 持有；无全球 placement/replication |
| D1 | 完整 Worker database/session/prepared-statement/result/meta surface，包括 bookmark 与 session 顺序一致性 | 单个本地主 SQLite authority，无 read replica/region routing；bookmark 仍须提供可观察的单机等价语义 |
| Durable Objects | 完整 namespace/ID/stub/RPC/state/storage/SQL/sync KV/transaction/alarm/WebSocket hibernation surface | object 固定在本地 workerd；location hint/global migration 无实际调度效果，但不能借此删掉非放置 API |
| Queues | 完整 Worker producer、push consumer、message/batch、ack/retry、metrics、delay 和 stable content types，包括 structured-clone `v8` | 本地 scheduler、at-least-once delivery；不承诺全球扩缩容、严格 FIFO 或 exactly-once |
| Workflows | 完整 Worker binding、instance lifecycle、batch/delete、step config/context、structured-clone payload、并行 step、event、restart-from-step 和 rollback surface | 本地 durable engine；不承诺跨地域执行、全球 placement 或 Cloudflare dashboard/observability 实现 |

一个差异只有同时满足以下条件才可登记为 deviation：它由单机拓扑直接导致、不删除 API 成员或合法
输入、不改变安全/事务/持久化保证、有官方事实来源、有稳定 ID，并有正负向及 restart/crash 回归。
R2 multipart、D1 bookmark、DO hibernation、Queue `v8`、Workflow batch/rollback 等不属于拓扑差异，
必须实现。

## 5. 明确非目标

以下内容不属于本文的 Cloudflare compatibility verdict：

- Cloudflare `/client/v4` account/resource REST API 及其 response envelope；
- Dashboard、账号/组织、API token/permission、billing、plan/quota 管理；
- Wrangler 的账号登录、远程资源创建、发布、版本、secret、tail 等管理命令 parity；
- zone、DNS、route、custom domain、CDN Rules、WAF 和 Cloudflare 其他产品控制面；
- 本文不会仅因产品名而自动纳入 R2 S3-compatible endpoint、Queue pull HTTP endpoint 等不属于
  tenant Worker types 的外部 data-plane 协议；若要兼容这些协议，应另立有独立来源和 Gate 的目标，
  它们也不能被误称为管理 API；
- Cloudflare Edge/Anycast、跨机/跨地域复制、全球 placement、流量调度和多副本 HA；
- `@cloudflare/workers-types/experimental` 与不属于七项产品或 Workers core runtime 的产品 binding；
- Python Workers 等不由 stable TypeScript/workerd JavaScript type surface 定义的语言目标。

open-compute 必须继续提供自己的本地资源 lifecycle、部署、备份、恢复、权限和运维 API，但这些接口
只需满足本项目的安全、完整性和可运维合同，不需要复制 Cloudflare 路径、字段、错误码或 Wrangler
管理行为。可选的 Wrangler config importer 只是开发工具，不属于 compatibility Go 条件。

## 6. 兼容性状态与完成定义

目标范围内的条目只允许以下状态：

| 状态 | 含义 |
| --- | --- |
| `supported` | upstream stable type 原样可用，运行时成功/失败和本地副作用通过全部 Gate |
| `supported_with_deviation` | API 完整，仅存在第 4 节允许的单机语义差异 |
| `blocked` | 属于目标，但实现或证据未完成；发布时不能声称兼容 |

`unsupported` 只用于第 5 节明确非目标，不能用于目标七项产品内的 stable API。当前矩阵中的
`unsupported`/子集条目在迁移完成前应改记为 `blocked`，避免把待实现能力永久化为产品边界。

Platform Go 必须同时满足：

1. 目标版本的 upstream stable types 已直接消费或可复现等价生成，无自有 Cloudflare API 声明；
2. types、workerd release、`effectiveCompatibilityDate`、内部 flags、docs 与 workers-sdk baseline 协调一致；
3. upstream AST 中属于目标范围的每个成员都映射到 capability/catalog 和至少一个 runtime case；
4. 所有 `supported`/`supported_with_deviation` case 走真实 `platformd`、stock workerd、SQLite/S3
   及真实进程路径，成功、拒绝、stream/RPC、restart/crash 行为均通过；
5. `blocked` 为零；没有通过删除类型、忽略 option、返回占位值或 fallback 绕过的方法；
6. 高风险 portable fixture 与真实 Cloudflare 没有未解释的 “Cloudflare pass / open-compute fail”；
7. capability、generated `Env`、文档、实现和测试的双向完整性检查通过；
8. 管理 API 和第 5 节非目标不进入 pass denominator，也不被错误宣传为兼容。

## 7. 当前实现差距与兼容改造清单

### 7.1 审计口径与当前结论

本清单是对 2026-08-30 工作区实现的静态审计，不把“类型里有名字”“stock workerd 大概率支持”或
“某个产品 Gate 已通过一个子集”当作全量兼容证据。主要证据是：

- [`packages/types/index.d.ts`](../packages/types/index.d.ts) 及根 `package.json`；
- [`packages/runtime/workerd.lock.json`](../packages/runtime/workerd.lock.json)、runtime loader 和各 binding
  facade；
- `crates/workers` 的 descriptor/runtime snapshot、`crates/storage` 的 Day1 schema 和各产品 authority；
- [`share/cloudflare-capabilities.json`](../share/cloudflare-capabilities.json)、
  [`test/conformance/catalog.json`](../test/conformance/catalog.json) 与当前 product Gates；
- pinned workerd generated types、workerd tests、workers-sdk/Miniflare 和 WDL 参考实现；
- 本次审计时的 Cloudflare 官方文档和 npm metadata。

| 领域 | 当前实现证据 | 当前目标状态 |
| --- | --- | --- |
| Types | 自有 declaration 只有 330 行，而 pinned workerd generated snapshot 为 17,253 行；前者重写并裁剪了 Web API 和七项产品类型 | `blocked` |
| Runtime baseline | tenant 可提交 date/flags；descriptor、SQLite、runtime snapshot、loader、toolchain、fixture 和 capability 都保留多 date/flag 模型 | `blocked` |
| Workers core | catalog 只枚举 fetch/RPC/streams/basic WebSocket/HTTP fetch；大量 stock-workerd stable surface 没有逐成员 inventory 和 Gate | `blocked` |
| KV | runtime 已有单键和最多 100 键的 `get`/`getWithMetadata`，但自有 types/catalog 没有批量 overload，结果缺 `cacheStatus` | `blocked` |
| R2 | `head/get/put/delete/list` 是子集；multipart 显式抛 `R2_UNSUPPORTED_FEATURE`，SSE-C、非 MD5 checksum、`startAfter` 和完整 storage class 未实现 | `blocked` |
| D1 | query 子集可用；`getBookmark()` 固定返回 `null`，`withSession()` 拒绝显式 bookmark，meta shape 是本地自定义固定字段 | `blocked` |
| Durable Objects | native facet 提供了部分 SQLite storage 基础，但 namespace option 被全部拒绝、RPC 只传 plain DTO、hibernation 未支持，DO 内 Queue/Workflow mutation 被阻塞 | `blocked` |
| Queues | producer/consumer、delay、ack/retry 和 metrics 主路径已存在；`v8` 显式拒绝，consumer batch metadata 未传入，自有返回类型与 runtime 不一致 | `blocked` |
| Workflows | durable replay、基础 step/wait/event/lifecycle 已有；batch/delete、structured clone、完整 step config、通用并行图、rollback 和 restart-from-step 未实现 | `blocked` |
| Conformance | capability/catalog 只到产品方法名，types Gate 只用正则看 interface 名；portable Cloudflare differential 目前只有 Cache API fixture | `blocked` |

以下条目全部是未完成工作。一个条目只有在实现、类型、capability、正负向行为和适用的恢复证据一起
完成后才能勾选；只修改文档或 types 不算完成。

### 7.2 横切基线与类型改造（必须最先完成）

- [ ] **CF-BASE-01：建立一个协调一致的 upstream pin。** 在 formal runtime lock 中固定
  `workerd` release/revision、archive/binary digest、stable workers-types version/gitHead/digest、
  workers-sdk revision、`effectiveCompatibilityDate` 和 tenant 所需的内部 flags。当前仓库的
  `v1.20260826.1`/`5.20260826.1` 已落后于审计时的 `1.20260830.1`/`5.20260830.1`，不得只升级其中
  一个。验收时还要证明 package stable AST 与该 workerd release 的 generated stable AST 没有未解释差异。

- [ ] **CF-TYPE-01：删除自有 Cloudflare API declarations。** 将
  [`packages/types/index.d.ts`](../packages/types/index.d.ts) 中对 Headers、Request、Response、Streams、
  WebSocket、Workers modules 和七项产品接口的声明全部移除。tenant 默认直接消费 pinned
  `@cloudflare/workers-types` stable 入口；若保留 `@open-compute/workers-types`，它只能引用该入口、
  生成 deployment-specific `Env`，以及在 `open-compute:*` module 中声明真正的平台扩展。

- [ ] **CF-TYPE-02：生成而不是手写 `Env` 和 RPC 组合类型。** 从经 authority 验证的 vars、secrets、
  KV/R2/D1/DO/Queue/Workflow/Service/Assets 等 binding descriptor 生成精确 `Env`；未绑定产品不能凭
  upstream 全局 type name 出现在 `Env`。Service RPC、DO stub 和 named entrypoint 使用 upstream
  `wrangler types` 同类组合规则，不使用 `[method: string]: unknown`。

- [ ] **CF-TYPE-03：把 types Gate 改为结构完整性 Gate。** 删除
  [`test/conformance/check.ts`](../test/conformance/check.ts) 中“必须引用自有 types、不得引用 upstream
  types”的检查和 interface-name 正则检查，改为版本/digest、AST declaration/member/overload/generic/
  optional/readonly 比对、两次生成字节一致和 compile fixtures。stable AST 中属于目标的成员如果没有
  runtime case，catalog 必须是 `blocked`，不能从声明中删掉。

- [ ] **CF-LATEST-01：从所有 tenant 输入删除 compatibility date/flags。** 一次性更新
  `packages/toolchain` project schema/build/deploy/framework importer、Worker upload DTO、descriptor、bundle
  canonical hash、deployment/runtime-source DTO、SQLite Day1 schema、loader snapshot、examples 和 tests。
  Wrangler/framework importer 可以读取上游配置以导入项目，但只能归一到当前唯一 baseline；请求旧行为、
  opt-out 或 experimental flag 时必须明确拒绝，不能持久化成 tenant runtime selector。

- [ ] **CF-LATEST-02：让所有 tenant isolate 使用 formal lock 的同一语义。** dynamic Worker、DO class、
  Queue/Cron event target 和 Workflow class 都从同一个 built runtime identity 获得 date/内部 flags；移除
  `COMPATIBILITY_DATE_MIN/MAX`、tenant flag allowlist、runtime snapshot 中的 date/flags 以及 Assets 等代码
  的历史 date/flag 分支。system Worker 的 `experimental`/host flags 继续是内部实现细节，不能进入 tenant
  capability。

- [ ] **CF-CAP-01：把 capability/catalog 从产品名清单改为 upstream-member inventory。** 从 pinned
  stable AST 生成带 stable symbol、member/overload、所属产品和 source identity 的 inventory；将当前
  `methods: ["get", ...]` 之类的粗粒度记录展开到合法参数、返回 shape 和关键语义。公开 capability 删除
  date min/max 和 allowed flags，改为不可变 runtime contract identity 与逐项 `supported`/`blocked` 状态。

- [ ] **CF-DEV-01：拆分当前 deviation 中混入的功能缺口。** `OC-WS-001`、`OC-D1-001` 中的 bookmark
  缺口、`OC-QUEUE-001` 中的 `v8`/metadata 缺口，以及 `OC-WORKFLOW-001/002` 中的 batch、rollback、
  structured clone、parallel 和 output-gate 缺口都应转成 `blocked` 工作项。deviation 只保留第 4、5 节
  允许的单机拓扑差异。

- [ ] **CF-CODEC-01：建立一个受测的 structured-clone/RPC serialization 基础。** DO RPC、Queue `v8`
  和 Workflow payload/result/step output 需要共享清晰的 value contract，但不能用一个更窄的 JSON DTO
  冒充 upstream `Rpc.Serializable` 或 structured clone。codec 必须逐 API 固定循环、transferable、typed
  array、Date/Map/Set/Error 等值是支持还是拒绝，以及大小限制、拒绝形态和跨 SQLite/restart 的稳定表示；
  不属于某项 API 的 capability/stream 类型不得被错误序列化。

### 7.3 Workers runtime 改造

- [ ] **CF-WKR-01：逐项验证 stock workerd stable runtime。** 以 upstream AST 和 runtime API index 为
  inventory，覆盖 globals、Request/Response/Headers/URL、encoding、timers、console、Abort、Web Crypto、
  Streams/BYOB、WebSocket、HTMLRewriter、EventSource、MessageChannel、performance/scheduler、Cache、
  handler/event、module rules、WebAssembly、RPC 和 `cloudflare:*` stable modules。stock workerd 已提供的
  能力以直接暴露和回归为主；loader/wrapper 改写、隐藏或改变错误的能力必须修正。

- [ ] **CF-WKR-02：补齐 handler、context 和 module class 语义。** 验证 module Worker 的 fetch、scheduled、
  queue、alarm/DO、Workflow 入口，`ExecutionContext.waitUntil/passThroughOnException/props/exports`，以及
  `WorkerEntrypoint`、`DurableObject`、`WorkflowEntrypoint`、`RpcTarget` 的 constructor、inheritance、RPC
  和生命周期。当前只用三个 interface 名和一个 common runtime case 的证据不够。

- [ ] **CF-WKR-03：按 latest 默认启用 Node.js surface。** Cloudflare 在 2026-08-04 之后默认启用
  `nodejs_compat` 和 v2；当前 toolchain 仍要求 tenant 显式 flag 才允许 `node:*` import。删除该输入开关，
  使用 pinned runtime 所对应的默认 Node modules/polyfills 和匹配的 `@types/node`，并对 builtin module、
  process/env 隔离、unsupported stub 和 bundler resolution 做 compile/real-runtime Gate。

- [ ] **CF-WKR-04：解决 raw TCP 与当前安全不变量的冲突。** `cloudflare:sockets.connect()` 和由其支撑的
  `node:net` 是 stable Workers runtime API，但当前 `OutboundGateway` 只导出 HTTP(S) `fetch()`，仓库安全
  规则也要求 tenant outbound 为 HTTP(S)-only。因此 raw TCP 不是“单机不相关能力”，而是当前明确的
  `blocked` 项；在另行授权并完成 public-address-only、DNS rebinding/private/loopback/metadata/Unix/port
  policy、budget、half-open/TLS、DO lifetime 和 real-network Gate 的安全设计前，Platform 不得声称完整
  Workers runtime 兼容，也不得用空 stub 或仅类型声明绕过。

- [ ] **CF-WKR-05：对齐已提供的 Workers 配套 surface。** Service bindings/RPC、Static Assets binding、
  Version Metadata、Cache API、scheduled 和当前已广告的 Images binding 必须使用 upstream 名称、签名、
  error 和 lifecycle；`OpenComputeCache`、`OpenComputeExecutionContext` 等本地扩展只能移到
  `open-compute:*` namespace，不能替换同名 Cloudflare 合同。真实 colo、Anycast 和 edge cache 命中来源
  不要求复制，但对应 tenant 属性若属于 stable API，必须返回文档化且稳定的本地 shape。

### 7.4 KV、R2 与 D1 改造

#### KV

- [ ] **CF-KV-01：对齐全部 overload 和返回 shape。** 将当前已经存在的批量 `get`/
  `getWithMetadata` runtime 路径纳入 upstream types/catalog；补齐 `KVNamespace<Key>` generic、type/options
  overload、批量 `Map` 返回、`KVNamespaceListResult` discriminated union，以及
  `getWithMetadata`/`list` 的 `cacheStatus`。单节点可以返回稳定的本地 cache 状态，但不能省略字段。

- [ ] **CF-KV-02：对齐 options、限制和错误。** 覆盖 `null` prefix/cursor、`cacheTtl`、metadata、expiration
  互斥、key/value/metadata/batch 大小、stream 取消和 JSON parse failure；用 stock workerd/Cloudflare
  fixture 固定异常类型、何时同步抛错、何时 Promise reject 和部分 body 消费行为。

- [ ] **CF-KV-03：保留且收窄单机 deviation。** SQLite authority 可提供单节点强一致，不模拟全球
  eventual consistency 或 edge cache 传播；但同进程并发、permission、cursor、TTL/expiration、restart/
  crash、metadata 和 stream 可观察行为仍必须通过 Gate。

#### R2

- [ ] **CF-R2-01：修正 object/body/options/list 基础 shape。** 当前 `version` optional、checksum 为 MD5
  string、metadata 被强制为空对象，且 `list` 没有 `startAfter`/精确 cursor union。改为 upstream
  `R2Object`、`R2ObjectBody`、`R2Checksums.toJSON()`、`R2Objects`、range/conditional overload，并严格验证
  `include`、`onlyIf`、Headers 和错误类型；删除当前额外接受的非上游形态。

- [ ] **CF-R2-02：实现完整 checksum、SSE-C 和 storage class。** `put` 支持 MD5、SHA-1/256/384/512
  中单一算法的验证，`get/put/multipart` 支持 `ssecKey`，object 暴露 `ssecKeyMd5`，并使 Standard/
  InfrequentAccess 等 pinned stable storage class 可写、可读、可 list round-trip。计费和全球存储层不在
  目标内，但字段、加密失败、checksum mismatch 和 secret 不泄露属于目标。

- [ ] **CF-R2-03：实现 multipart。** 补齐 `createMultipartUpload`、`resumeMultipartUpload`、
  `uploadPart`、`complete`、`abort`、part number/etag/order/size 限制、并发 complete/abort 和无效 upload ID
  行为。authority、S3 multipart state、失败清理和 platform restart 后 resume 必须一致，不能继续用
  `R2_UNSUPPORTED_FEATURE`。

- [ ] **CF-R2-04：补齐条件写和 provider 映射。** 当前 `put` 拒绝 uploaded-before/after，需对齐 pinned
  Cloudflare 条件判定、失败返回 `null` 与异常边界；同时验证 S3 provider 的 ETag/version/checksum/range/
  metadata 差异被 facade 归一化，而不是直接泄露 provider response。

#### D1

- [ ] **CF-D1-01：对齐声明、result/meta 和错误。** 使用 upstream `D1Meta`、`D1Response`、prepared
  statement overload 和 raw column-name tuple；当前 facade 不得要求非上游 `served_by` 固定字段或拒绝
  `served_by_region/colo`、`timings`、`total_attempts` 等合法字段。本地 meta 可以稳定说明 local/primary，
  但类型、optional 性和数值语义必须匹配。

- [ ] **CF-D1-02：实现 session bookmark。** `getBookmark()` 不能永久返回 `null`；首次 query 后产生
  opaque、可校验的新鲜度 token，`withSession(bookmark)` 必须接受之前的 token，并保证新 session 看到
  至少该版本；公开合同不能要求 tenant 解析或排序 bookmark。单机无需 read-replica/region routing，但
  顺序一致性、无 query 时 `null`、无效/其他 DB bookmark、并发写、restart/crash 后 token 行为必须确定。

- [ ] **CF-D1-03：处理仍在 stable types 中的 deprecated `dump()`。** 不能因 deprecated 从自有类型删除。
  若 Day1 本地 D1 对应 Cloudflare 非 alpha 数据库，应通过 upstream/Cloudflare probe 固定该数据库类型的
  失败行为；只有上游 stable surface 允许成功时才实现 SQLite-compatible ArrayBuffer，不要臆造 alpha
  兼容模型。

- [ ] **CF-D1-04：扩充事务和恢复证据。** 对 prepare/bind/batch/exec/run/all/first/raw/session 的 SQL/
  parameter/blob/duplicate-column/size/timeout/authorizer 错误做 differential；验证 batch 原子性、session
  顺序、backup/restore 和 crash 后 bookmark/result 不回退。

### 7.5 Durable Objects 改造

- [ ] **CF-DO-01：对齐 namespace、ID 和 stub。** 补齐 typed RPC stub、`id.jurisdiction`、
  `namespace.jurisdiction()`、`newUniqueId({jurisdiction})`、`get/getByName({locationHint,routingMode})` 和
  `Fetcher.connect` 是否属于当前 pinned stub surface。location/jurisdiction 的真实地理效果是非目标；
  为源码可移植性仍需验证枚举、ID round-trip/隔离并提供稳定本地语义，不能像当前实现一样拒绝所有非空
  options 或直接缺方法。

- [ ] **CF-DO-02：用 upstream RPC 语义替换 plain DTO RPC。** 当前手写 wire 只允许 null/string/boolean/
  finite number/binary/plain array/object，无法代表完整 `Rpc.Serializable`、capability、stream 和异常传播。
  应尽量复用 workerd native RPC，并保留 deployment pin、stub ordering、retirement 和 sanitized error
  边界；对不可序列化值按 upstream 时机和错误类型拒绝。

- [ ] **CF-DO-03：逐成员验证 state/storage，而不是只广告五个方法。** 覆盖 state 的
  `waitUntil/exports/props/blockConcurrencyWhile/abort` 及 pinned stable 成员；storage 覆盖 multi-key
  get/put/delete、options、list ordering、deleteAll、transaction/rollback、sync、SQL cursor、sync KV、
  transactionSync、alarm 和 bookmark/PITR API。当前 facet/native storage 已存在的成员可以直接保留，
  但 alarm proxy 不得改变 method binding、transaction rollback、options 或 output-gate 行为。

- [ ] **CF-DO-04：实现 Hibernatable WebSocket。** 补齐 `acceptWebSocket`、tag/getWebSockets、auto-response、
  hibernatable event timeout、attachment serialization，以及 `webSocketMessage/Close/Error` handler。
  验证 idle eviction 后连接不断、constructor 重跑、attachment 恢复、auto-response 不唤醒、close/error、
  generation rollover、process restart 和资源清理；基础 WebSocket 不能替代此项。

- [ ] **CF-DO-05：证明 alarm/deleteAll/latest 语义。** 当前 alarm shim 已处理 async/sync transaction 和
  index flush，但仍需按唯一 latest baseline 固定 `deleteAll` 是否同时删除 alarm、set/delete option、retry
  和 abort 行为，删除历史 date branch。每个 mutation 必须继承 native DO output gate 并在 index 写失败时
  保持 storage/调度一致。

- [ ] **CF-DO-06：打通 DO 内 Queue/Workflow mutation output gate。** 当前
  `QUEUE_DO_OUTPUT_GATE_UNSUPPORTED` 与 `WORKFLOW_DO_OUTPUT_GATE_UNSUPPORTED` 是目标内缺口。实现必须证明
  storage transaction rollback 时外部 mutation 不提交、commit 后恰好进入 durable authority、runtime/
  platform crash 边界不产生未授权提交；不能简单移除 fail-closed 检查。

### 7.6 Queues 改造

- [ ] **CF-QUEUE-01：对齐 producer 和 consumer shape。** 自有 types 中 `send/sendBatch` 仍返回
  `Promise<void>` 且 metrics 字段错误，而 runtime 已返回 metadata。改为 upstream
  `QueueSendResponse/QueueSendBatchResponse/QueueMetrics`，补 `sendBatch` options，并把实时 backlog metadata
  传入 consumer `MessageBatch.metadata`；验证 oldest timestamp 的 sentinel/Date 行为。

- [ ] **CF-QUEUE-02：实现 stable `v8` content type。** 当前 facade 显式抛
  `QUEUE_CONTENT_TYPE_UNSUPPORTED`。按 pinned workerd structured-clone 行为编码、持久化和恢复 Date/Map/
  Set/typed array 等值，严格检查 content type 与 body、128 KB message、batch count/body 和 delay limits；
  JSON/text/bytes 也要验证 detached/resizable buffer 等边界。

- [ ] **CF-QUEUE-03：补齐 push consumer 语义。** 对 message/batch metadata、ack/retry/ackAll/retryAll
  冲突优先级、per-message/batch delay、handler throw、`waitUntil`、DLQ、attempt count、visibility/reclaim 和
  at-least-once redelivery建立 upstream fixture。SQLite scheduler 的单机吞吐和无全球扩缩容不是失败，但
  crash 前后 disposition/metrics 不一致是失败。

- [ ] **CF-QUEUE-04：完成所有 event-source/output-gate 组合。** 覆盖普通 Worker、DO、Workflow 和 Service
  binding 调用 Queue 的 mutation ordering、budget、permission、deployment retirement 和 process crash；
  其中 DO 路径依赖 CF-DO-06。

### 7.7 Workflows 改造

- [ ] **CF-WF-01：补齐 binding/instance API。** 实现 `createBatch`、`deleteBatch`、instance `delete`，修正
  `get` 的 Promise 类型、status/error union、batch limit/per-item error、retention 与 `locationHint`。
  locationHint 在单机可无地理效果，但合法输入和可观察返回不能被拒绝。

- [ ] **CF-WF-02：补齐 lifecycle options。** `terminate({rollback})` 必须可执行已登记 rollback handler，
  `restart({from:{name,count,type}})` 必须保留指定 step 之前的 durable results 并从准确 occurrence 重启；
  覆盖 paused/waiting/errored/terminated/complete 等状态的成功、幂等和拒绝矩阵。当前 facade 对两者只接受
  空对象，不能标记为 supported。

- [ ] **CF-WF-03：替换 canonical JSON payload。** create/event/step output/final output/replay 使用 pinned
  upstream 支持的 structured-clone/RPC serializable surface，并保留 size/sensitivity/security 边界；
  Workflow event 补齐 schedule、step event 的 timestamp/type/sensitive 和 payload readonly 语义。不能因
  SQLite 存储方便继续把 Date/Map/Set 等合法值判为 unsupported。

- [ ] **CF-WF-04：补齐 `WorkflowStep.do` overload/config/context。** 支持默认和显式 config、retry
  limit/backoff、duration/timeout、dynamic delay function、sensitivity、step name/count/attempt/resolved config
  以及 rollback handler/config；异常对象传给 dynamic delay/rollback 时必须经过明确定义的安全边界，
  不能把 tenant secret 写入 durable status/log。

- [ ] **CF-WF-05：实现通用 parallel step/Promise 图。** 当前 runner 只把同一 microtask 内同步声明的
  `step.do` 收集成有限 batch，parallel wait 或新的依赖图会抛 `WORKFLOW_PARALLEL_STEP_UNSUPPORTED`。需要
  持久化 dependency DAG，支持合法 `Promise.all`/交错 do/sleep/waitForEvent、失败传播、重放和 bounded
  scheduler；callback 完成顺序不能改变 durable dependency 或重复已提交输出。

- [ ] **CF-WF-06：完成 rollback、恢复和 DO output gate。** rollback 逆序/重试/timeout、restart-from-step、
  batch create/delete、event race、pause/resume/terminate 和 platform SIGKILL 后恢复必须通过 fresh-process
  Gate。DO mutation 路径依赖 CF-DO-06；外部 HTTP/第三方副作用不需要提供强于 Cloudflare 合同的
  exactly-once 保证，但 callback/replay 的 at-least-once 边界必须一致且文档化。

### 7.8 Conformance 与验收改造

- [ ] **CF-TEST-01：为 inventory 每个成员建立证据双射。** 每个目标 member/overload 至少对应一个
  compile fixture 和一个 real-runtime success/rejection case；stream、RPC、transaction、scheduler、
  encryption、hibernation、structured clone 等高风险项必须有独立语义 case，不能共享一个只检查
  `typeof method === "function"` 的 smoke test。

- [ ] **CF-TEST-02：直接复用匹配 revision 的 upstream tests。** 优先移植
  `references/workerd/src/workerd/api/tests` 中 KV/R2/Queue/WebSocket/SyncKV 等 fixture，以及
  workers-sdk/Miniflare 的 binding contract cases；只替换 harness/authority，不改期望来迎合当前实现。
  WDL/Localflare 可提供实现思路和额外 case，但不能成为正确性真值。

- [ ] **CF-TEST-03：扩大 portable Cloudflare differential。** 至少覆盖 Workers web/runtime 高风险行为、
  KV batch/cacheStatus、R2 conditional/checksum/multipart、D1 bookmark/meta、DO namespace/RPC/WebSocket、
  Queue serialization/disposition/metadata 和 Workflow lifecycle/step graph。fixture 必须同一源码、同一输入、
  归一化仅限第 4、5 节允许差异；Cloudflare 外部运行仍需显式 credential/写入授权。

- [ ] **CF-TEST-04：状态型 API 必须有恢复矩阵。** KV TTL、R2 multipart、D1 session、DO storage/alarm/
  hibernation、Queue delivery 和 Workflow execution 分别覆盖 restart、SIGKILL、提交前/后 fault、重复请求和
  generation rollover。只有无持久状态的 pure Web API 可以不做 crash case。

- [ ] **CF-TEST-05：同步 capability、matrix、deviation 和 docs。** 实现完成前目标缺口一律 `blocked`；
  完成后机器 catalog、`platformd capabilities --json`、本页、当前兼容矩阵和 Gate registry 必须由双向检查
  证明一致。不得把旧 product Gate 的通过记录当成新增 surface 的证据。

### 7.9 不进入兼容改造 denominator 的事项

以下内容不是待实现 gap，不应占用本清单的 `blocked -> supported` denominator：

| 不计入项 | 边界说明 |
| --- | --- |
| Edge/Anycast/colo/全球流量调度 | 不实现边缘部署和真实 colo 选择；若 stable tenant API 暴露相关属性，只要求稳定、文档化的本地 shape，不要求伪造 Cloudflare 地理事实 |
| 跨机/跨地域复制、多副本 HA、全球 failover | KV/R2/D1/DO/Queue/Workflow 只对单机 authority、restart/crash recovery 负责；不模拟全球一致性传播或跨区迁移 |
| placement/jurisdiction 的真实地理效果 | DO/Workflow 的合法 runtime 参数为源码兼容仍要接受或给出稳定本地语义；是否真的放到 `eu/enam` 不验收 |
| Cloudflare 管理面 | `/client/v4`、Dashboard、账号/组织/token/permission、billing/plan/quota、远程资源 lifecycle 和 Wrangler login/deploy/tail 不在目标内 |
| 外部 data-plane 协议 | R2 S3-compatible public endpoint、Queue pull HTTP、管理型 bulk/list message API 不因同产品名自动纳入；Worker binding API 仍必须完整 |
| 全球观测与产品分析 | Cloudflare dashboard analytics、Logpush、全球 traces/metrics aggregation 不要求复制；tenant runtime 的 `console`、error 和目标 binding metadata 仍属于兼容面 |
| 其他产品与 experimental | AI、Analytics Engine、Vectorize、Hyperdrive、Browser、Workers for Platforms 等非七项产品 binding，以及 `@cloudflare/workers-types/experimental` 不计入 |
| Cloudflare 商业限制 | 套餐、计费、Cloudflare account quota 和全球容量不复制；为单机安全/资源保护设置的本地 limit 必须公开、稳定，且不能改变 API shape |

反过来，R2 multipart、D1 session bookmark、DO Hibernatable WebSocket、Queue `v8`、Workflow batch/
rollback/parallel、latest Node surface 和 Workers raw TCP 都不是边缘部署或异地同步专属能力，不能放进此表
规避。raw TCP 当前受仓库安全不变量阻塞，处理方式见 CF-WKR-04。

### 7.10 依赖顺序与阶段出口

| 阶段 | 必须完成 | 出口条件 |
| --- | --- | --- |
| M0 upstream freeze | CF-BASE-01、CF-TYPE-01/02/03 | types/workerd/workers-sdk/date/flags 单一 pin；AST inventory 可重复生成 |
| M1 Day1 contract | CF-LATEST-01/02、CF-CAP-01、CF-DEV-01 | tenant date/flags 和旧分支从 schema 到 runtime 全部消失；所有目标缺口为 `blocked` |
| M2 runtime substrate | CF-CODEC-01、CF-WKR-*、CF-DO-01/02/03/05 | core runtime、RPC/storage/output-gate 基础明确；raw TCP 要么经授权实现，要么 Platform 保持 blocked |
| M3 product completion | KV/R2/D1、CF-DO-04/06、Queues、Workflows | 七项产品 stable inventory 无缺成员，单机 deviation 只剩允许项 |
| M4 qualification | CF-TEST-01..05 | types、real workerd、产品、failure、recovery 和获授权 differential 证据闭环，`blocked=0` |

实现应按 Day1 直接改当前 authority、schema 和协议；不保留旧 date/flag、旧自有 types、旧 wire format 或
子集 facade 的兼容层。只有 M4 完成后才能更新 Platform verdict；本文完成不代表实现完成。

## 8. 官方与仓库基线

- [Cloudflare Workers Runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/)
- [Cloudflare TypeScript 与 runtime types](https://developers.cloudflare.com/workers/languages/typescript/)
- [Cloudflare compatibility flags](https://developers.cloudflare.com/workers/configuration/compatibility-flags/)
- [Cloudflare Workers TCP sockets](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/)
- [Cloudflare KV Worker API](https://developers.cloudflare.com/kv/api/read-key-value-pairs/)
- [Cloudflare R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/)
- [Cloudflare D1 database/session API](https://developers.cloudflare.com/d1/worker-api/d1-database/)
- [Cloudflare Durable Object namespace](https://developers.cloudflare.com/durable-objects/api/namespace/)
- [Cloudflare Durable Object state/WebSocket](https://developers.cloudflare.com/durable-objects/api/state/)
- [Cloudflare Durable Object SQLite storage](https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/)
- [Cloudflare Queues JavaScript API](https://developers.cloudflare.com/queues/configuration/javascript-apis/)
- [Cloudflare Workflows Workers API](https://developers.cloudflare.com/workflows/build/workers-api/)
- [Cloudflare Workflows step context](https://developers.cloudflare.com/workflows/build/step-context/)
- [`@cloudflare/workers-types`](https://www.npmjs.com/package/@cloudflare/workers-types)
- [`packages/runtime/workerd.lock.json`](../packages/runtime/workerd.lock.json)
- [`references/workerd/types/generated-snapshot/index.d.ts`](../references/workerd/types/generated-snapshot/index.d.ts)
- [`references/workers-sdk`](../references/workers-sdk)
- [`references/wdl/docs/compatibility.zh.md`](../references/wdl/docs/compatibility.zh.md)
- [`references/localflare`](../references/localflare)

`references/workerd` 和 npm/types 决定 API shape；WDL、workers-sdk、Miniflare 和 Localflare 只提供
实现、fixture、配置或开发体验参考，不能覆盖官方类型和 stock workerd 行为。
