# Cloudflare Worker Runtime 全量兼容目标

状态：Completed，2026-09-01。本文定义的 Day1 runtime、binding、persistence、catalog 与本地验收实现
已经完成；真实 Cloudflare 上的 Workers、Cache API、KV、D1、R2、Durable Objects、Queues portable
fixtures 已通过。Workflow 托管端 differential 因 Wrangler OAuth `10000` 尚未 qualification，已从实现
范围拆分到[剩余验收计划](../cloudflare-runtime-compatibility-acceptance.md)。当前状态以
[`references/cloudflare-compatibility.md`](../references/cloudflare-compatibility.md)、机器可读 capability、
contract catalog 和[完成报告](cloudflare-runtime-compatibility-results.md)为准。

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

### 3.3 完成快照

截至 2026-09-01，本轮改造已把 baseline 协调到同一上游 revision：

- formal pin 为 `workerd v1.20260830.1`，revision
  `e9dda5963aba7ee4323960db795690ec78fec118`，`effectiveCompatibilityDate=2026-08-30`；
- stable types 为 `@cloudflare/workers-types@5.20260830.1`，其 npm `index.d.ts` 与同 revision
  workerd generated snapshot 字节和 AST digest 一致；
- `@open-compute/workers-types` 只剩 pinned upstream types reference 与 `open-compute:cache` 平台扩展，
  不再重声明 Cloudflare API；
- tenant project、upload、descriptor、SQLite authority、RuntimeSource 和公开 capability 已删除 date/flags
  selector；system Worker flags 只由 formal lock 决定。

single-latest Day1 基线与目标实现已经落地。当前 inventory 共 2,097 个 stable members/overloads：
1,585 个 `supported`，512 个 `supported_with_deviation`，`blocked=0`；2,097 条 `memberEvidence` 与
capability 成员双射。`OC-WKR-TCP-001`、`OC-WKR-LIMIT-001` 只记录已验证的 hosted/self-host 差异，
不掩盖成员缺口。以上版本号是本次冻结证据，不是永久常量；下一次 coordinated update 仍须整体重新固定。

## 4. 产品兼容边界

下表不复制上游 TypeScript 签名；具体成员和 overload 永远由第 2 节的 upstream 类型产物所有。

| 领域 | 必须兼容的 Worker 编程面 | 可接受的单机差异 |
| --- | --- | --- |
| Workers runtime | stable globals、Web APIs、Fetch、Streams、WebSocket、Crypto、HTMLRewriter、Cache API、scheduled、TCP sockets、RPC、ExecutionContext、module/handler、Service/Static Assets/Version Metadata tenant surface、latest default Node.js surface，以及七项产品需要的 module imports | 无 Anycast/colo/edge placement；无法真实提供的 edge metadata 必须有文档化、稳定且不泄密的本地语义；tenant TCP 保持 public-address-only，Cloudflare 自有 IP ownership/TCP-loop/SMTP abuse policy 由 CF-WKR-04 的 self-host deviation 明确隔离；Cloudflare plan/fleet 的 request-scoped CPU、subrequest 与并发连接 quota 不复制，stock OSS workerd 的实际限制能力必须如实登记 |
| KV | 完整 `KVNamespace` stable surface，包括单键/批量 overload、metadata、stream、list 和 cache-status shape | 单节点强一致替代全球 eventual consistency/edge cache；方法 shape、限制与错误仍须兼容 |
| R2 | 完整 Worker `R2Bucket`/object/body/multipart/options/checksum/SSE-C/storage-class surface | object bytes 由配置的 S3-compatible provider 持有；无全球 placement/replication |
| D1 | 完整 Worker database/session/prepared-statement/result/meta surface，包括 bookmark 与 session 顺序一致性 | 单个本地主 SQLite authority，无 read replica/region routing 或 hosted serving identity；bookmark 提供可观察的单机等价语义，`rows_read`/`rows_written` 是本地 SQLite authority 的稳定执行计数而非 Cloudflare 计费计数 |
| Durable Objects | 完整 namespace/ID/stub/RPC/state/storage/SQL/sync KV/transaction/alarm/WebSocket hibernation surface | object 固定在本地 workerd；location hint/global migration 无实际调度效果，但不能借此删掉非放置 API |
| Queues | 完整 Worker producer、push consumer、message/batch、ack/retry、metrics、delay 和 stable content types，包括 structured-clone `v8` | 本地 scheduler、at-least-once delivery；不承诺全球扩缩容、严格 FIFO 或 exactly-once |
| Workflows | 完整 Worker binding、instance lifecycle、batch/delete、step config/context、structured-clone payload、并行 step、event、restart-from-step 和 rollback surface | 本地 durable engine；不承诺跨地域执行、全球 placement 或 Cloudflare dashboard/observability 实现 |

一个差异只有同时满足以下条件才可登记为 deviation：它由单机拓扑直接导致，或属于官方 hosted
plan/fleet quota 而 pinned stock OSS runtime 明确不提供对应 enforcement；它不删除 API 成员或合法输入、
不改变安全/事务/持久化保证、有官方和 pinned-source 事实来源、有稳定 ID，并有适用的正负向及
restart/crash 回归。
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

## 7. 实现记录与兼容改造清单

### 7.1 完成口径与当前结论

本清单按 2026-09-01 冻结实现验收，不把“类型里有名字”“stock workerd 大概率支持”或单个 smoke test
当作全量兼容证据。主要证据是：

- [`packages/types/index.d.ts`](../../packages/types/index.d.ts) 及根 `package.json`；
- [`packages/runtime/workerd.lock.json`](../../packages/runtime/workerd.lock.json)、runtime loader 和各 binding
  facade；
- `crates/workers` 的 descriptor/runtime snapshot、`crates/storage` 的 Day1 schema 和各产品 authority；
- [`share/cloudflare-capabilities.json`](../../share/cloudflare-capabilities.json)、
  [`test/conformance/catalog.json`](../../test/conformance/catalog.json) 与当前 product Gates；
- pinned workerd generated types、workerd tests、workers-sdk/Miniflare 和 WDL 参考实现；
- 本次审计时的 Cloudflare 官方文档和 npm metadata。

| 领域 | 完成证据 | 状态 |
| --- | --- | --- |
| Types | pinned upstream declaration 由薄桥直接消费；npm/snapshot 字节与 AST digest、一致生成、compile fixtures 与 2,097 条成员 evidence 双射通过 | `supported` |
| Runtime baseline | formal lock、tenant loader 与工具链统一为 `v1.20260830.1` / `2026-08-30`；tenant date/flags selector 和历史分支已删除 | `supported` |
| Workers core | 1,580 个成员全部有 compile/stock-workerd/runtime evidence；latest Node、handlers/RPC、Web APIs、raw TCP 与配套 surface 均闭环 | `supported` / `supported_with_deviation` |
| KV | 52 个成员覆盖 batch overload、metadata、stream、list、`cacheStatus`、限制、错误时序与恢复；portable fixture 与 Cloudflare 一致 | `supported_with_deviation` |
| R2 | 110 个成员覆盖 object/body/list/options/checksum/SSE-C/storage class、conditional put、multipart 与 restart；portable fixture 与 Cloudflare 一致 | `supported_with_deviation` |
| D1 | 36 个成员覆盖 database/session/prepared statement/result/meta、opaque bookmark、原子 batch 与 `dump()` 拒绝；portable fixture 与 Cloudflare 一致 | `supported_with_deviation` |
| Durable Objects | 115 个成员覆盖 namespace/ID/stub/RPC/facet、state/storage/SQL、alarm、hibernation、output gate 与 connect tunnel；portable fixture 与 Cloudflare 一致 | `supported` / `supported_with_deviation` |
| Queues | 63 个成员覆盖 producer/consumer、`v8`、metrics、delay、ack/retry、output gate 与 crash recovery；portable fixture 与 Cloudflare 一致 | `supported_with_deviation` |
| Workflows | 72 个成员覆盖 lifecycle、batch/delete、structured clone、step graph、event、restart、rollback 与 output gate；本地真实进程和恢复 Gate 通过 | `supported_with_deviation`；托管端 differential 待账号权限 |
| Conformance | inventory/capability/catalog 双射、结构类型 Gate、compile fixtures、case registry、portable runner、193 个 JS tests、802 个最终 workspace cases 与 90.17% Rust 行覆盖率 | `blocked=0`；远端 Workflow qualification 单独跟踪 |

勾选按严格完成定义：实现、类型、capability、正负向行为和适用恢复证据同时完成。托管端 Workflow
differential 不改变本地 member 状态，但在完成前不能声称“全部产品均经真实 Cloudflare 实测”。

### 7.2 横切基线与类型改造（必须最先完成）

- [x] **CF-BASE-01：建立一个协调一致的 upstream pin。** 在 formal runtime lock 中固定
  `workerd` release/revision、archive/binary digest、stable workers-types version/gitHead/digest、
  workers-sdk revision、`effectiveCompatibilityDate` 和 tenant 所需的内部 flags。当前仓库的
  `v1.20260830.1`/`5.20260830.1` 与同一 revision/digest，package stable AST 与 workerd generated
  snapshot 已证明字节及结构一致。后续升级仍不得只升级其中一个。

- [x] **CF-TYPE-01：删除自有 Cloudflare API declarations。** 将
  [`packages/types/index.d.ts`](../../packages/types/index.d.ts) 中对 Headers、Request、Response、Streams、
  WebSocket、Workers modules 和七项产品接口的声明全部移除。tenant 默认直接消费 pinned
  `@cloudflare/workers-types` stable 入口；若保留 `@open-compute/workers-types`，它只能引用该入口、
  生成 deployment-specific `Env`，以及在 `open-compute:*` module 中声明真正的平台扩展。

- [x] **CF-TYPE-02：生成而不是手写 `Env` 和 RPC 组合类型。** 从经 authority 验证的 vars、secrets、
  KV/R2/D1/DO/Queue/Workflow/Service/Assets 等 binding descriptor 生成精确 `Env`；未绑定产品不能凭
  upstream 全局 type name 出现在 `Env`。Service RPC、DO stub 和 named entrypoint 使用 upstream
  `wrangler types` 同类组合规则，不使用 `[method: string]: unknown`。

- [x] **CF-TYPE-03：把 types Gate 改为结构完整性 Gate。** 删除
  [`test/conformance/check.ts`](../../test/conformance/check.ts) 中“必须引用自有 types、不得引用 upstream
  types”的检查和 interface-name 正则检查，改为版本/digest、AST declaration/member/overload/generic/
  optional/readonly 比对、两次生成字节一致和 compile fixtures。stable AST 中属于目标的成员如果没有
  runtime case，catalog 必须是 `blocked`，不能从声明中删掉。

- [x] **CF-LATEST-01：从所有 tenant 输入删除 compatibility date/flags。** 一次性更新
  `packages/toolchain` project schema/build/deploy/framework importer、Worker upload DTO、descriptor、bundle
  canonical hash、deployment/runtime-source DTO、SQLite Day1 schema、loader snapshot、examples 和 tests。
  Wrangler/framework importer 可以读取上游配置以导入项目，但只能归一到当前唯一 baseline；请求旧行为、
  opt-out 或 experimental flag 时必须明确拒绝，不能持久化成 tenant runtime selector。

- [x] **CF-LATEST-02：让所有 tenant isolate 使用 formal lock 的同一语义。** dynamic Worker、DO class、
  Queue/Cron event target 和 Workflow class 都从同一个 built runtime identity 获得 date/内部 flags；移除
  `COMPATIBILITY_DATE_MIN/MAX`、tenant flag allowlist、runtime snapshot 中的 date/flags 以及 Assets 等代码
  的历史 date/flag 分支。system Worker 的 `experimental`/host flags 继续是内部实现细节，不能进入 tenant
  capability。

- [x] **CF-CAP-01：把 capability/catalog 从产品名清单改为 upstream-member inventory。** 从 pinned
  stable AST 生成带 stable symbol、member/overload、所属产品和 source identity 的 inventory，覆盖
  interface/class/type-literal、复合别名、union 分支、function/constructor type，以及目标符号上可观察的
  heritage/composition；将当前 `methods: ["get", ...]` 之类的粗粒度记录展开到合法参数、返回 shape
  和关键语义。公开 capability 删除 date min/max 和 allowed flags，改为不可变 runtime contract identity
  与逐项 `supported`/`blocked` 状态。新发现的目标成员保持 `blocked`，不得用占位证据标成支持。

- [x] **CF-DEV-01：拆分当前 deviation 中混入的功能缺口。** `OC-WS-001`、`OC-D1-001` 中的 bookmark
  缺口、`OC-QUEUE-001` 中的 `v8`/metadata 缺口，以及 `OC-WORKFLOW-001/002` 中的 batch、rollback、
  structured clone、parallel 和 output-gate 缺口都应转成 `blocked` 工作项。deviation 只保留第 4、5 节
  允许的单机拓扑差异。

- [x] **CF-CODEC-01：建立一个受测的 structured-clone/RPC serialization 基础。** runtime 已有单一 Day1
  durable-value codec（显式 `queue-v8`/`workflow` profile），固定循环/共享引用、typed array、Date/Map/Set/Error
  等值的支持或 fail-closed 拒绝，以及大小限制和稳定字节表示；不以更窄 JSON DTO 冒充 upstream
  `Rpc.Serializable` 或 structured clone。Queue `v8` 与 Workflow payload/result 已接入该 codec；DO RPC
  改为 workerd native facet/RPC capability path；三个 observable-boundary members 已纳入逐项 qualification。

### 7.3 Workers runtime 改造

- [x] **CF-WKR-01：逐项验证 stock workerd stable runtime。** 以 upstream AST 和 runtime API index 为
  inventory，覆盖 globals、Request/Response/Headers/URL、encoding、timers、console、Abort、Web Crypto、
  Streams/BYOB、WebSocket、HTMLRewriter、EventSource、MessageChannel、performance/scheduler、Cache、
  handler/event、module rules、WebAssembly、RPC 和 `cloudflare:*` stable modules。stock workerd 已提供的
  能力以直接暴露和回归为主；loader/wrapper 改写、隐藏或改变错误的能力必须修正。

- [x] **CF-WKR-02：补齐 handler、context 和 module class 语义。** 验证 module Worker 的 fetch、scheduled、
  queue、alarm/DO、Workflow 入口，`ExecutionContext.waitUntil/passThroughOnException/props/exports`，以及
  `WorkerEntrypoint`、`DurableObject`、`WorkflowEntrypoint`、`RpcTarget` 的 constructor、inheritance、RPC
  和生命周期。完成证据使用 compile fixture、stock-workerd case 与产品事件源矩阵，不以 interface 名 smoke
  test 代替行为验证。

- [x] **CF-WKR-03：按 latest 默认启用 Node.js surface。** Cloudflare 在 2026-08-04 之后默认启用
  `nodejs_compat` 和 v2；tenant 不得选择 date 或 flags。toolchain 已无条件保留 `node:` 为
  external，compile 使用 pinned `@types/node`。focused stock-workerd 用例已证明代表 builtin、process/env
  隔离、unsupported stub 与 host `fs` 失败关闭；`cloudflare:sockets`/`node:net` 由 CF-WKR-04 验收。

- [x] **CF-WKR-04：开放受限公网 raw TCP。** fetch-only `OutboundGateway` 已删除，tenant general outbound
  `fetch()`、`cloudflare:sockets.connect()` 和 `node:net` 已直接共享唯一 `Network(allow=["public"])`
  capability；命名 Service/DO 的 `Fetcher.connect()` 走显式 capability tunnel。获授权的隔离 Linux
  public/TLS matrix、TLS/half-open/lifecycle、完整 p0-7、跨 Queue/Cron/DO fetch/Workflow 的 Service connect，
  以及 DO alarm/fetch 和 Workflow 的 direct general-outbound 成功/拒绝路径均已闭环；27 个 raw-TCP catalog
  members 是 `supported_with_deviation`，raw-TCP blocked gap 已删除。冻结源码后的最终 workspace Gate
  按用户明确要求只执行完整一轮，802/802 cases 通过；Workers 其余成员也已由各自 evidence 闭环。
  不得重新引入 HTTP-only 旧路径、空 stub、类型先行占位或双 outbound 实现。

#### CF-WKR-04 Day1 raw TCP 方案

**架构决策与新安全不变量。** Cloudflare Workers 的 raw TCP 是“受限公网 TCP”，不是“不受限主机
socket”，也不是 HTTP(S)-only。open-compute 的 Day1 不变量改为：tenant 的所有通用 outbound 必须经
同一个 platform-owned、public-address-only workerd Network capability；tenant 不得获得 private network、
平台内部 Fetcher、任意 external service、Unix socket 或宿主网络 capability。`fetch()` 与 `connect()` 的
协议不同，但地址安全边界相同。该决策替换 HTTP(S)-only 旧规则；实现时必须同步更新仓库政策、当前 API
matrix、安全文档和测试，不能靠本计划与旧规则长期并存。

选择 stock workerd 的 Network 作为唯一地址 authority，理由和上游证据如下：

- [`references/workerd/src/workerd/server/workerd.capnp`](../../references/workerd/src/workerd/server/workerd.capnp)
  定义 `Network.allow/deny`，默认只允许 `public`；DNS hostname 的解析结果在连接层逐地址过滤，没有允许
  地址时按解析失败处理。不要在 TypeScript 中先解析 hostname 再拨号，避免产生第二套易受 TOCTOU/DNS
  rebinding 影响的检查；
- [`references/workerd/src/workerd/server/server.c++`](../../references/workerd/src/workerd/server/server.c++)
  通过 `kj::Network::restrictPeers()` 构造受限 Network；同一个 service 原生处理 `fetch()` 和 `connect()`；
- WDL 将 tenant `WorkerCode.globalOutbound` 直接固定为
  [`PUBLIC_NETWORK`](../../references/wdl/runtime/load.js)，其配置是
  [`allow=["public"]`](../../references/wdl/runtime/config-user.capnp)，并用真实
  [`cloudflare:sockets.connect()` 负向用例](../../references/wdl/tests/integration/network-boundary.test.js)
  证明 Redis 和 runtime mesh 不可达；
- Miniflare 只作为 API/lifecycle fixture 来源，不是生产安全基线。它在
  [`core/index.ts`](../../references/workers-sdk/packages/miniflare/src/plugins/core/index.ts) 中为本地开发显式
  允许 `public`、`private` 和保留地址，并用 `connect_pass_through` 穿过 fetch interceptor；open-compute
  不复制这条宽松路径，也不启用该 experimental flag。

**唯一 general-outbound 图。** 最终代码只保留以下通用公网路径：

```text
tenant fetch() / cloudflare:sockets.connect() / node:net
    -> WorkerCode.globalOutbound
    -> host-only PUBLIC_NETWORK capability
    -> workerd Network(allow = ["public"])
    -> DNS result address filtering
    -> public TCP peer
```

命名 Service/DO 的 `Fetcher.connect(authority)` 不进入该图：它只能从 deployment 中显式声明的 capability
发起，由 workerd 把 `authority` 原样放入目标 `connect(socket)` 的 `SocketInfo.localAddress`，再经内部
capability tunnel 转发字节。它不是 tenant 可选择目标的第二条通用 outbound，也不扩大 public Network。
在当前 pinned workerd 中，主动 `connect()` 返回的 outbound socket 以 `remoteAddress` 精确报告请求 authority，
`localAddress` 是 optional 且通常缺省；目标 Service/DO 的 inbound `connect(socket)` 则以 `localAddress`
精确报告同一 authority，`remoteAddress` 缺省。测试不得把 outbound `localAddress` 强制断言为 string，也
不得把 inbound authority 当成解析后的 peer 地址。

实现必须作为一个原子重构完成：

1. 保留 [`packages/runtime/config.capnp`](../../packages/runtime/config.capnp) 中唯一的 `internet` Network，补齐
   TLS browser CA 配置，并把它作为 host-only `PUBLIC_NETWORK` binding 提供给 loader host、DO host、
   Workflow host 和 Service transport 的 WorkerLoader 所有者；内部 runtime-source、binding-backend、DO
   router 等仍只能通过命名 service binding 到达；
2. 所有非 validation 的 dynamic `WorkerCode` 都直接设置 `globalOutbound: env.PUBLIC_NETWORK`；validation
   isolate 继续使用 `globalOutbound: null`。public request、Service binding、DO、Queue/Cron 和 Workflow
   重入必须使用同一个 helper，禁止某个 event source 留在旧 gateway 或获得更宽能力；
3. 删除 [`OutboundGateway`](../../packages/runtime/src/loader/host.ts)、Cap'n Proto 的
   `outbound-gateway` service、`ctx.exports.OutboundGateway(...)` 调用、`deploymentId/policyVersion` props 及
   对应 export/test。它当前只校验 HTTP(S) scheme，且 props 没有形成额外 authority；保留它再为 TCP 加
   pass-through 会制造两套路径并依赖 experimental flag；
4. `PUBLIC_NETWORK` 只进入 system Worker env，再作为 `WorkerCode.globalOutbound` capability 传递；不得加入
   tenant `env`、descriptor、RuntimeSource、capability JSON、日志或错误。tenant 能使用全局 API，但不能取回
   或 RPC 转移底层 Network service；
5. `cloudflare:sockets` 和 `node:` 继续由 toolchain externalize，运行时直接使用 pinned workerd 模块；不写
   wrapper、不改写 import、不模拟 `Socket`/`node:net.Socket`，避免 half-open、backpressure、TLS 和错误
   生命周期与上游分叉。

截至 2026-09-01，上述唯一 outbound 图已经落到当前代码：Cap'n Proto 只保留一个启用 browser CA 的
`internet`/public Network，loader 与 DO host 只在 system env 中持有 `PUBLIC_NETWORK`，所有非 validation
动态 Worker 都经过 `tenantGlobalOutbound()` 设置 `globalOutbound`，validation 仍为 `null`；旧
`outbound-gateway` service、Worker、props 与 runtime source 已删除。toolchain 仍 externalize
`cloudflare:sockets`、`node:net` 和 `node:tls`，没有实现 tenant-visible socket wrapper。不过 Rust
deployment descriptor 中无 runtime consumer 的 `GLOBAL_OUTBOUND_POLICY_VERSION`/
`globalOutboundPolicyVersion` 也已连同 producer 和 byte-identity test 删除，没有保留旧 gateway 版本 shim。
普通 stock-workerd focused case 已证明 private/loopback/metadata/IPv4-mapped private IPv6 与 `node:net`
不能绕过 public Network；隔离 Linux arm64 user/mount/network namespace 中运行了与授权脚本等价的临时
public-classified IPv4/IPv6、DNS 和 TLS fixture，没有修改宿主地址、`/etc/hosts` 或已有服务。注册的
`p0-2::p0_2_real_worker_create_validate_dispatch_promote_rollback_restart` 完整通过 public literal/DNS、
192 KiB 多 chunk、half-open、TLS/StartTLS、证书拒绝、`expectedServerHostname`、旧 socket neuter、
`node:net`/`node:tls`、Queue/Cron、私网/DNS-to-private 拒绝和 runtime restart。临时测试 CA 只注入
stock-workerd 测试配置；产品配置仍只有 browser CA。

Service Binding 显式 capability tunnel 上的 native `Fetcher.connect()`/`connect(socket)` 双向字节、
deployment pin 清理和跨 Queue/Cron/DO fetch/Workflow 调用均有真实 runtime evidence；p0-7 完整 case 也已
通过 DO stub/object connect、hibernation、reconstruction 和 restart。Workflow 冷启动过程中发现并修复了
漏嵌 `workflows/duration.js` 的单路径模块装配缺陷。`p0-8` 已证明 DO alarm 与 DO fetch 直接调用
`cloudflare:sockets.connect()` 的公网 echo 和 DNS-to-private 拒绝；`workflow-runtime` 已证明 Workflow
直接调用的相同成功/拒绝语义，并修正统一 `claim-batch` 路径对 waitForEvent suspended/failed verdict 的
误判。旧 `register-sleep`/`register-wait` 私有端点及其重复 scheduler authority 已删除。27 个 raw-TCP
成员现已登记 compile/runtime evidence 与 `OC-WKR-TCP-001`、`OC-WKR-LIMIT-001`，不再属于 blocked gap。
专项兼容性复核、TypeScript/production/static checks、90.17% Rust 行覆盖率与冻结源码的最终 Gate 均已
闭环；其余 Workers/DO 成员由相应条目和最终 inventory evidence 独立闭环。

**地址与平台差异。** `allow=["public"]` 必须在普通 Gate 和受控网络 Gate 中证明拒绝 IPv4/IPv6
loopback、RFC private/link-local、metadata、IPv4-mapped IPv6 private、Unix/abstract Unix 与 DNS-to-private；
runtime-source、binding-backend 和 workerd internal listener 强制 loopback，因此落在该地址拒绝边界内。
redirect 的每一跳仍经过同一 Network。raw TCP 没有 redirect 语义。任何只按
hostname string、初始 URL 或 tenant header 判断的实现都不合格。

Cloudflare 生产还拒绝 Cloudflare 自有 IP 段、连接回发起 Worker 的 TCP loop，并默认拒绝 SMTP 25；这些
规则依赖 Cloudflare 自有 edge/address/abuse-control 基础设施。单机 self-host 中，runtime-source、
binding-backend 和 workerd internal listener 强制 loopback；control/data listener 默认 loopback，但 operator
可以显式暴露为公开入口，public Network 不按“open-compute 所有权”额外拒绝其公网地址。外部 reverse
proxy、公开 control/data 入口、Cloudflare 公网地址和宿主 SMTP egress policy 由 operator 网络拥有。实现完成时登记
`OC-WKR-TCP-001`：open-compute 不复制 Cloudflare IP ownership、TCP-loop detector 或默认 25 端口封禁，
因此 operator 显式暴露的入口不能被宣传为由 self-connect detector 保护。portable Cloudflare differential
对这些目标记录预期 deviation，不能把它们伪装成同结果。若未来要求复制 Cloudflare 的运营级端口/IP policy，
应另行设计 platformd-owned egress proxy；不得在当前 direct-Network 路径旁增加半截代理。

Cloudflare account 套餐和 fleet capacity 不复制。Cloudflare 当前把 subrequest、CPU 和“等待初始响应”的
并发连接作为不同限制，其中 `connect()` 会计入同时等待建立的连接，Service binding 触发的 Worker 与顶层
请求共享该连接限制。固定 revision 的 stock OSS workerd standalone server 与 Cloudflare 托管实现不同：
[`LimitEnforcer`](../../references/workerd/src/workerd/server/server.c++) 明确标注“不执行限制”，
`newSubrequest()` 是空实现，`getLimitsExceeded()` 永远返回 none；WorkerLoader 虽暴露
`ResourceLimits.cpuMs/subRequests`，该 standalone path 也不会执行这些数值。

Day1 已删除会制造“已限制”假象的 TypeScript `PROFILE`/`WorkerCode.limits`，Rust upload、SQLite Day1
schema、deployment descriptor、RuntimeSnapshot、API response 和 tests 中只接受 `{profile:"default"}`、运行时
不消费的 `limits` 字段也已一起删除。这个 single-machine stock-workerd capacity 差异登记为
`OC-WKR-LIMIT-001`：API shape 与地址安全边界不变，但 open-compute 当前不承诺 Cloudflare plan/fleet 的
request-scoped CPU、subrequest 或 simultaneous-open-connection enforcement。不得再加一个只能包住
`cloudflare:sockets`、却会被 `node:net` 或其他 native outbound 绕过的 TypeScript 计数器；将来若要提供
统一本机 execution budget，必须在覆盖所有 native outbound/event source 的 authority 层实现并另行验证。
连接、stream reader/writer、TLS upgrade 和 client disconnect 仍必须释放本机 socket/handle；不得在
module global 缓存 tenant socket 或跨 invocation 共享。

**API 与生命周期验收矩阵。** 至少覆盖：

| 面 | 必须证明 |
| --- | --- |
| 地址输入 | string/object、IPv4/IPv6、hostname、缺 host/port、非法字符、端口范围和稳定异常类型 |
| 建连 | `opened` 成功/拒绝、connect refused、DNS failure、关闭前失败、重复 close |
| Streams | 双向字节、backpressure、大于一个 chunk、reader/writer lock、EOF、取消和 peer reset |
| Half-open | `allowHalfOpen=false/true` 的对端半关闭行为，不用平台 timeout 掩盖错误 |
| TLS | `secureTransport=off/on/starttls`、`startTls()` 一次性升级、expected hostname、证书失败和旧 socket neuter |
| Node | `node:net`/`node:tls` 的 connect、data/end/error/timeout/destroy，以及与 `cloudflare:sockets` 相同的地址拒绝 |
| Event source | public fetch、Service binding、scheduled、Queue、DO alarm/fetch、Workflow 中成功和拒绝路径一致 |
| DO lifetime | socket 活跃、显式关闭、对端关闭、alarm completion、eviction/reconstruction 和 platform restart 无泄漏 |
| 安全输出 | tenant error 不含解析后的内部地址列表、平台 listener、路径、token、source 或宿主拓扑 |
| 本机容量 | capability/docs 明确 `OC-WKR-LIMIT-001`，删除无消费的 deployment `limits`/`WorkerCode.limits`，不把局部 JS 计数器宣传为 Cloudflare CPU/subrequest/并发连接限制 |

**测试与完成出口。** 开发阶段只跑一轮相关 focused cases；实现冻结后再进入最终 Gate。测试分层如下：

- compile fixture 覆盖 `cloudflare:sockets`、`node:net`、`node:tls` 和 `Fetcher.connect()`；27 个 inventory
  members 已绑定精确注册 runtime cases、两项 deviation，并从 `workers.raw-tcp-security-boundary` gap 移入
  catalog `memberEvidence`；
- crate-local/runtime focused tests 固定 loader 各 event source 都使用同一 `PUBLIC_NETWORK`、validation
  仍为 `null`、tenant env 不泄露 capability，且仓库中不再存在 `OutboundGateway`/HTTP-only 分支；
- 普通 stock-workerd Gate 覆盖 malformed、private/loopback/metadata/Unix 拒绝、capacity deviation、
  cleanup 和 sanitized error；
- [`test/test-p0-2-egress-linux.sh`](../../test/test-p0-2-egress-linux.sh) 的获授权 Linux fixture 已扩展为在
  临时 public-classified IPv4/IPv6 地址上运行 bounded TCP echo/half-close/TLS 服务；对应 Worker fixture
  覆盖 literal、DNS、DNS-to-private、192 KiB 多 chunk、地址信息、half-open、TLS/StartTLS 证书拒绝与旧
  socket neuter，以及 `node:net`/`node:tls` echo、timeout/destroy 和私网拒绝。真实宿主脚本继续要求显式
  sudo 授权并清理 alias、hosts、证书、listener 和进程；本轮使用隔离 Linux user/mount/network namespace
  执行同一注册 p0-2 case，未修改宿主网络，因此这些断言已有 qualification，但仍不成为普通非特权 Gate
  的隐式步骤；
- portable differential 使用同一 Worker 源码比较 Cloudflare 和 open-compute；只有
  `OC-WKR-TCP-001` 与 `OC-WKR-LIMIT-001` 已进入 deviation authority、capability 和边界测试后才允许按它们
  归一化；Cloudflare credential/deploy 仍需单独外部写入授权；
- 最终完成要求：相关 focused、format/typecheck/static checks、coverage 和最终 Gate 按仓库政策通过，27
  个成员不再 `blocked`，docs/API matrix/capabilities/catalog 一致，且退出后没有 workerd、TCP listener、
  socket、临时地址、hosts 条目或 secret 遗留。任一项未满足时 CF-WKR-04 保持未勾选，不保留部分开放。

- [x] **CF-WKR-05：对齐已提供的 Workers 配套 surface。** Service bindings/RPC、Static Assets binding、
  Version Metadata、Cache API、scheduled 和当前已广告的 Images binding 必须使用 upstream 名称、签名、
  error 和 lifecycle；`OpenComputeCache`、`OpenComputeExecutionContext` 等本地扩展只能移到
  `open-compute:*` namespace，不能替换同名 Cloudflare 合同。真实 colo、Anycast 和 edge cache 命中来源
  不要求复制，但对应 tenant 属性若属于 stable API，必须返回文档化且稳定的本地 shape。

### 7.4 KV、R2 与 D1 改造

#### KV

- [x] **CF-KV-01：对齐全部 overload 和返回 shape。** 批量 `get`/`getWithMetadata`、generic/type/options
  overload、批量 `Map`、list discriminated union 和 `getWithMetadata`/`list.cacheStatus` 已落地；52 个
  inventory members 均由 `namespace-surface.ts`、`p0-4::p0_4_real_kv_matrix` 和
  `p3-cf-diff::kv/portable/namespace` 建立 compile/真实 runtime evidence。

- [x] **CF-KV-02：对齐 options、限制和错误。** 已覆盖 `null` prefix/cursor、`cacheTtl`、metadata、TTL
  优先于同时提供的 absolute expiration、key/value/metadata/batch 大小、stream/BufferSource/string chunk、
  JSON parse failure、DOMString 转换与未配对 surrogate；真实 Cloudflare differential 固定了异常类型、文本
  和 Promise rejection 时序。

- [x] **CF-KV-03：保留且收窄单机 deviation。** SQLite authority 提供单节点强一致，不模拟全球
  eventual consistency 或 edge cache 传播；但同进程并发、permission、cursor、TTL/expiration、restart/
  crash、metadata 和 stream 可观察行为由 P0.4 与 portable differential 覆盖，唯一差异登记为
  `OC-KV-001`。

#### R2

- [x] **CF-R2-01：修正 object/body/options/list 基础 shape。** upstream `R2Object`、`R2ObjectBody`、
  `R2Checksums.toJSON()`、`R2Objects`、`startAfter`、range/conditional overload，以及 `include`/`onlyIf`/
  Headers 严格验证已落地；110 个 inventory members 的逐项 evidence 已完成。

- [x] **CF-R2-02：实现完整 checksum、SSE-C 和 storage class。** `put` 已支持 MD5、SHA-1/256/384/512
  中单一算法的验证，`get/put/multipart` 支持 `ssecKey`，object 暴露 `ssecKeyMd5`，并使 Standard/
  InfrequentAccess 等 pinned stable storage class 可写、可读、可 list round-trip。计费和全球存储层不在
  目标内，但字段、加密失败、checksum mismatch 和 secret 不泄露属于目标。

- [x] **CF-R2-03：实现 multipart。** `createMultipartUpload`、`resumeMultipartUpload`、
  `uploadPart`、`complete`、`abort`、part number/etag/order/size 限制、并发 complete/abort 和无效 upload ID
  行为已接入 Day1 authority 和 S3 multipart state，并覆盖失败清理、并发 complete/abort 和 platform
  restart 后 resume；upstream member-level qualification 已完成。

- [x] **CF-R2-04：补齐条件写和 provider 映射。** uploaded-before/after 条件判定、失败返回 `null`、
  ETag/version/checksum/range/metadata provider 归一化、异常边界与 upstream member-level qualification
  已完成。

#### D1

- [x] **CF-D1-01：对齐声明、result/meta 和错误。** 直接消费 upstream `D1Meta`、`D1Response`、prepared
  statement overload 和 raw column-name tuple；facade 对齐 hosted result/error/cause、bind 转换、`first`/`raw`
  宽容参数和 stable meta 字段。本地只返回可证明的 local-primary 数值语义，不伪造 `served_by`、region/colo
  或 hosted timing；真实 Cloudflare portable fixture 逐字段归一化验证公开结果。

- [x] **CF-D1-02：实现 session bookmark。** 首次 query 后产生
  opaque、可校验的新鲜度 token，`withSession(bookmark)` 必须接受之前的 token，并保证新 session 看到
  至少该版本；公开合同不能要求 tenant 解析或排序 bookmark。opaque token、无 query 时 `null`、无效/其他
  DB bookmark、输入 trim/null/blank、显式 bookmark 的立即可见性、并发写和 restart/crash 行为均已落地并
  纳入 P0.6 与 portable differential。

- [x] **CF-D1-03：处理仍在 stable types 中的 deprecated `dump()`。** stable type 保留该方法；Day1 本地
  D1 对齐当前 hosted 非 alpha 数据库的 `D1_DUMP_ERROR: Status + 400` 与 cause，而不保留或臆造旧 alpha
  dump 实现。该结果由当前 compatibility date 的真实 Cloudflare probe 固定。

- [x] **CF-D1-04：扩充事务和恢复证据。** prepare/bind/batch/exec/run/all/first/raw/session 的 SQL、
  parameter/blob/duplicate-column、100 KiB SQL、100 parameter、2 MiB row、30 秒 VM deadline、authorizer、
  malformed input 和错误前缀均有正负向覆盖；batch 原子性、跨 binding statement 的 receiving-database
  语义、session 顺序以及 restart/crash 后 bookmark/result 不回退由 P0.6 和 portable differential 验证。

### 7.5 Durable Objects 改造

- [x] **CF-DO-01：对齐 namespace、ID 和 stub。** typed RPC stub、`id.jurisdiction`、
  `namespace.jurisdiction()`、`newUniqueId({jurisdiction})`、`get/getByName({locationHint,routingMode})` 和
  `Fetcher.connect` 是否属于当前 pinned stub surface。location/jurisdiction 的真实地理效果是非目标；
  为源码可移植性的枚举、ID round-trip/隔离与稳定本地语义已落地。`Fetcher.connect` 纳入
  CF-WKR-04；其余成员也已逐项 qualification。

- [x] **CF-DO-02：用 upstream RPC 语义替换 plain DTO RPC。** transport 已改为 workerd native
  Entrypoint/facet RPC，并保留 deployment pin、stub ordering、retirement 与 sanitized error 边界；完整
  `Rpc.Serializable`、capability、stream 和异常传播的三个 observable-boundary members 已 qualification。

- [x] **CF-DO-03：逐成员验证 state/storage，而不是只广告五个方法。** 覆盖 state 的
  `waitUntil/exports/props/blockConcurrencyWhile/abort` 及 pinned stable 成员；storage 覆盖 multi-key
  get/put/delete、options、list ordering、deleteAll、transaction/rollback、sync、SQL cursor、sync KV、
  transactionSync、alarm 和 bookmark/PITR API。当前 facet/native storage 已存在的成员可以直接保留，
  但 alarm proxy 不得改变 method binding、transaction rollback、options 或 output-gate 行为。

- [x] **CF-DO-04：实现 Hibernatable WebSocket。** `acceptWebSocket`、tag/getWebSockets、auto-response、
  hibernatable event timeout、attachment serialization，以及 `webSocketMessage/Close/Error` handler。
  验证 idle eviction 后连接不断、constructor 重跑、attachment 恢复、auto-response 不唤醒、close/error、
  generation rollover、process restart 和资源清理路径已落地 focused fixture；19 个 inventory members
  已逐项 qualification，基础 WebSocket 没有被用来替代此项。

- [x] **CF-DO-05：证明 alarm/deleteAll/latest 语义。** alarm path 已处理 async/sync transaction 和
  index flush，并按唯一 latest baseline 固定 `deleteAll`、set/delete option、retry 和 abort 行为，删除了
  历史 date branch。每个 mutation 继承 native DO output gate 并在 index 写失败时
  保持 storage/调度一致。

- [x] **CF-DO-06：打通 DO 内 Queue/Workflow mutation output gate。** Queue/Workflow mutation 已接入
  native DO output gate 和 durable operation/finalize 协议；最终 evidence 仍必须证明
  storage transaction rollback 时外部 mutation 不提交、commit 后恰好进入 durable authority、runtime/
  platform crash 边界不产生未授权提交；不能简单移除 fail-closed 检查。

### 7.6 Queues 改造

- [x] **CF-QUEUE-01：对齐 producer 和 consumer shape。** upstream
  `QueueSendResponse/QueueSendBatchResponse/QueueMetrics`、`sendBatch` options、实时 backlog metadata 和
  oldest timestamp sentinel/Date 行为已落地；63 个 inventory members 的逐项 evidence 已完成。

- [x] **CF-QUEUE-02：实现 stable `v8` content type。** facade 已按 pinned structured-clone profile 编码、
  持久化和恢复 Date/Map/
  Set/typed array 等值，严格检查 content type 与 body、128 KB message、batch count/body 和 delay limits；
  JSON/text/bytes 也要验证 detached/resizable buffer 等边界。

- [x] **CF-QUEUE-03：补齐 push consumer 语义。** 对 message/batch metadata、ack/retry/ackAll/retryAll
  冲突优先级、per-message/batch delay、handler throw、`waitUntil`、DLQ、attempt count、visibility/reclaim 和
  at-least-once redelivery建立 upstream fixture。SQLite scheduler 的单机吞吐和无全球扩缩容不是失败，但
  crash 前后 disposition/metrics 不一致是失败。

- [x] **CF-QUEUE-04：完成所有 event-source/output-gate 组合。** 覆盖普通 Worker、DO、Workflow 和 Service
  binding 调用 Queue 的 mutation ordering、budget、permission、deployment retirement 和 process crash；
  其中 DO 路径依赖 CF-DO-06。

### 7.7 Workflows 改造

- [x] **CF-WF-01：补齐 binding/instance API。** `createBatch`、`deleteBatch`、instance `delete`，
  `get` 的 Promise 类型、status/error union、batch limit/per-item error、retention 与 `locationHint`。
  locationHint 在单机可无地理效果，但合法输入和可观察返回不能被拒绝。

- [x] **CF-WF-02：补齐 lifecycle options。** `terminate({rollback})` 可执行已登记 rollback handler，
  `restart({from:{name,count,type}})` 必须保留指定 step 之前的 durable results 并从准确 occurrence 重启；
  paused/waiting/errored/terminated/complete 等状态的成功、幂等和拒绝矩阵及 upstream member-level
  qualification 已完成。

- [x] **CF-WF-03：替换 canonical JSON payload。** create/event/step output/final output/replay 已使用 pinned
  upstream 支持的 structured-clone/RPC serializable surface，并保留 size/sensitivity/security 边界；
  Workflow event 补齐 schedule、step event 的 timestamp/type/sensitive 和 payload readonly 语义。不能因
  SQLite 存储方便继续把 Date/Map/Set 等合法值判为 unsupported。

- [x] **CF-WF-04：补齐 `WorkflowStep.do` overload/config/context。** 支持默认和显式 config、retry
  limit/backoff、duration/timeout、dynamic delay function、sensitivity、step name/count/attempt/resolved config
  以及 rollback handler/config；异常对象传给 dynamic delay/rollback 时必须经过明确定义的安全边界，
  不能把 tenant secret 写入 durable status/log。

- [x] **CF-WF-05：实现通用 parallel step/Promise 图。** runner 与 authority 已持久化 batch frontier、
  dependency count 和 durable ordinal，支持合法 `Promise.all`/交错 do/sleep/waitForEvent、失败传播、重放
  和 bounded scheduler；完整 upstream graph matrix 已 qualification。

- [x] **CF-WF-06：完成 rollback、恢复和 DO output gate。** rollback 逆序/重试/timeout、restart-from-step、
  batch create/delete、event race、pause/resume/terminate 和 platform SIGKILL 后恢复必须通过 fresh-process
  Gate。DO mutation 路径依赖 CF-DO-06；外部 HTTP/第三方副作用不需要提供强于 Cloudflare 合同的
  exactly-once 保证，但 callback/replay 的 at-least-once 边界必须一致且文档化。

### 7.8 Conformance 与验收改造

- [x] **CF-TEST-01：为 inventory 每个成员建立证据双射。** 每个目标 member/overload 至少对应一个
  compile fixture 和一个 real-runtime success/rejection case；stream、RPC、transaction、scheduler、
  encryption、hibernation、structured clone 等高风险项必须有独立语义 case，不能共享一个只检查
  `typeof method === "function"` 的 smoke test。

- [x] **CF-TEST-02：直接复用匹配 revision 的 upstream tests。** 优先移植
  `references/workerd/src/workerd/api/tests` 中 KV/R2/Queue/WebSocket/SyncKV 等 fixture，以及
  workers-sdk/Miniflare 的 binding contract cases；只替换 harness/authority，不改期望来迎合当前实现。
  WDL/Localflare 可提供实现思路和额外 case，但不能成为正确性真值。

- [x] **CF-TEST-03：扩大 portable Cloudflare differential。** 至少覆盖 Workers web/runtime 高风险行为、
  KV batch/cacheStatus、R2 conditional/checksum/multipart、D1 bookmark/meta、DO namespace/RPC/WebSocket、
  Queue serialization/disposition/metadata 和 Workflow lifecycle/step graph。fixture 必须同一源码、同一输入、
  归一化仅限第 4、5 节允许差异；Cloudflare 外部运行仍需显式 credential/写入授权。Workers、Cache API、
  KV、D1、R2、Durable Objects、Queues 已闭环；Workflow fixture 与安全清理实现已完成，但托管端运行因
  OAuth `10000` 拆分为独立外部验收，不据此宣称 Workflow 已与托管端实测一致。

- [x] **CF-TEST-04：状态型 API 必须有恢复矩阵。** KV TTL、R2 multipart、D1 session、DO storage/alarm/
  hibernation、Queue delivery 和 Workflow execution 分别覆盖 restart、SIGKILL、提交前/后 fault、重复请求和
  generation rollover。只有无持久状态的 pure Web API 可以不做 crash case；各状态型产品的对应矩阵均已闭环。

- [x] **CF-TEST-05：同步 capability、matrix、deviation 和 docs。** 实现完成前目标缺口一律 `blocked`；
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
规避。raw TCP 必须按 CF-WKR-04 将安全不变量更新为 public-address-only，并完成真实网络资格验证。

### 7.10 依赖顺序与阶段出口

| 阶段 | 必须完成 | 出口条件 |
| --- | --- | --- |
| M0 upstream freeze | CF-BASE-01、CF-TYPE-01/02/03 | types/workerd/workers-sdk/date/flags 单一 pin；AST inventory 可重复生成 |
| M1 Day1 contract | CF-LATEST-01/02、CF-CAP-01、CF-DEV-01 | tenant date/flags 和旧分支从 schema 到 runtime 全部消失；所有目标缺口为 `blocked` |
| M2 runtime substrate | CF-CODEC-01、CF-WKR-*、CF-DO-01/02/03/05 | core runtime、RPC/storage/output-gate 基础明确；raw TCP 的 direct public Network、生命周期和安全 qualification 已闭环，其它 CF-WKR 项仍按成员证据推进 |
| M3 product completion | KV/R2/D1、CF-DO-04/06、Queues、Workflows | 七项产品 stable inventory 无缺成员，单机 deviation 只剩允许项 |
| M4 qualification | CF-TEST-01..05 | types、real workerd、产品、failure、recovery 和获授权 differential 证据闭环，`blocked=0` |

实现应按 Day1 直接改当前 authority、schema 和协议；不保留旧 date/flag、旧自有 types、旧 wire format 或
子集 facade 的兼容层。M0–M4 的本地实现与 evidence 已完成；托管端 Workflow qualification 保留在独立
验收计划中，因此本记录不把外部条件未满足改写为完整 Cloudflare 托管端 Platform Go。

## 8. 官方与仓库基线

- [Cloudflare Workers Runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/)
- [Cloudflare TypeScript 与 runtime types](https://developers.cloudflare.com/workers/languages/typescript/)
- [Cloudflare compatibility flags](https://developers.cloudflare.com/workers/configuration/compatibility-flags/)
- [Cloudflare Workers TCP sockets](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/)
- [Cloudflare Workers limits](https://developers.cloudflare.com/workers/platform/limits/#simultaneous-open-connections)
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
- [`packages/runtime/workerd.lock.json`](../../packages/runtime/workerd.lock.json)
- [`references/workerd/types/generated-snapshot/index.d.ts`](../../references/workerd/types/generated-snapshot/index.d.ts)
- [`references/workers-sdk`](../../references/workers-sdk)
- [`references/wdl/docs/compatibility.zh.md`](../../references/wdl/docs/compatibility.zh.md)
- [`references/localflare`](../../references/localflare)

`references/workerd` 和 npm/types 决定 API shape；WDL、workers-sdk、Miniflare 和 Localflare 只提供
实现、fixture、配置或开发体验参考，不能覆盖官方类型和 stock workerd 行为。
