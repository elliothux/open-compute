# Cloudflare Workers 兼容矩阵

本页是 [`share/cloudflare-capabilities.json`](../../share/cloudflare-capabilities.json) 与
[`test/conformance/catalog.json`](../../test/conformance/catalog.json) 的人类可读索引，不建立第二份
能力真值。`ocd capabilities --json`、类型 inventory、contract catalog 和 Gate 共同定义当前
支持面。完成设计和 conformance 方案见
[Cloudflare Runtime 全量兼容改造](../implemented/cloudflare-runtime-compatibility.md)与
[P3.4 Cloudflare conformance](../implemented/p3-4-cloudflare-conformance.md)。P6 当前管理合同及本地证据见
[归档设计](../implemented/p6-cloudflare-v4-wrangler-compatibility.md)与
[完成记录](../implemented/p6-cloudflare-v4-wrangler-compatibility-results.md)；尚待外部账号条件解除的 runtime
Workflow 与 P6 management qualification 分别只记录在[既有剩余验收](../acceptance/cloudflare-runtime-compatibility-acceptance.md)
和 [P6 远端差分验收](../acceptance/p6-cloudflare-v4-differential-acceptance.md)。

固定契约输入见 [`baseline.json`](../../test/conformance/baseline.json)。当前 formal pin 是
`workerd v1.20260830.1`，revision `e9dda5963aba7ee4323960db795690ec78fec118`，唯一
`effectiveCompatibilityDate` 为 `2026-08-30`；stable types 是
`@cloudflare/workers-types@5.20260830.1`。tenant 不得选择其它 compatibility date 或任意 flags，也不保留旧
open-compute schema、descriptor、runtime 或 API 的兼容路径。官方在 compatibility date `2026-08-04`
起默认启用 Node.js compatibility，并明确此日期后的 `nodejs_compat` 是被 Wrangler/runtime 忽略的冗余
正向 flag（[官方 changelog](https://developers.cloudflare.com/changelog/post/2026-08-04-nodejs-compat-default/)、
[Compatibility Flags](https://developers.cloudflare.com/workers/configuration/compatibility-flags/)）。因此 P6 wire
只额外接受并逐 Version 原样持久化精确的单值 `["nodejs_compat"]`，其与空数组在 pinned
`workerd v1.20260830.1` 下使用同一最新平台语义；其它 flag、组合与所有其它日期继续 fail closed。对应
multipart、descriptor、runtime-source/loader 回归防止它扩成 tenant 可选历史模式。

## 当前结论

目标 inventory 共 2,178 个 stable members/overloads：1,585 个 `supported`，593 个
`supported_with_deviation`，`blocked=0`。catalog 的 2,178 条 `memberEvidence` 与 capability 成员双射，
`blockedGaps=[]`。deviation 只描述单机 self-host 无法复制的 edge/全球拓扑、托管 fleet quota 或本地
authority 差异；它不代表缺方法、占位返回或半截实现。

| 产品 | 状态 | 成员 | 当前实现与证据 | deviation |
| --- | --- | ---: | --- | --- |
| Workers runtime | `supported_with_deviation` | 1,580 | 1,556 个成员直接支持；24 个 raw-TCP 成员保留完整 API，仅隔离 hosted TCP policy/fleet limit 差异。latest 默认 Node.js、Web APIs、handlers、RPC、Cache、raw TCP 和配套 surface 均有 compile/stock-workerd/runtime case | `OC-WKR-TCP-001`、`OC-WKR-LIMIT-001` |
| KV | `supported_with_deviation` | 52 | 单键/批量 overload、metadata、stream、list、`cacheStatus`、错误时序和恢复均闭环 | `OC-KV-001` |
| R2 | `supported_with_deviation` | 110 | object/body/list/options、全部 checksum、SSE-C、storage class、条件写、multipart、opaque physical key、持久 intent/reconcile 和 restart 均闭环；single/part/multipart ETag 公式及 lowercase-hex `ssecKeyMd5` 与官方 Worker API 一致 | `OC-R2-001` |
| D1 | `supported_with_deviation` | 36 | database/session/prepared statement/result/meta、opaque bookmark、原子 batch/exec、错误转换和非 alpha `dump()` 拒绝均闭环 | `OC-D1-001` |
| Durable Objects | `supported_with_deviation` | 115 | namespace/ID/stub/native RPC facet、state、sync KV/SQL、transaction、alarm、hibernation、output gate 和显式 connect tunnel 均闭环；112 个成员使用 `OC-DO-001`，3 个 connect 成员使用 TCP/limit deviation | `OC-DO-001`、`OC-WKR-TCP-001`、`OC-WKR-LIMIT-001` |
| DO Alarms | `supported` | 7 | get/set/delete、handler、retry/restart authority 均闭环 | — |
| Queues | `supported_with_deviation` | 63 | producer、consumer、`v8`、metrics、delay、ack/retry、output gate、at-least-once recovery 均闭环 | `OC-QUEUE-001` |
| Cron | `supported_with_deviation` | 26 | scheduled handler、`noRetry()`、Workflow schedules、projection/recovery 均闭环 | `OC-CRON-001` |
| Workflows | `supported_with_deviation` | 72 | binding/instance/batch/delete、structured clone、step config、parallel DAG、event、restart-from-step、rollback、DO output gate 均闭环 | `OC-WORKFLOW-001` |
| Cache API | `supported_with_deviation` | 14 | `Cache`/`CacheStorage`、vary/range/condition、purge、restart 和自动 cache 协作均闭环 | `OC-CACHE-001`、`OC-CACHE-002` |
| Version Metadata | `supported` | 3 | `id`、`tag`、`timestamp` 由 immutable deployment authority 注入 | — |
| WebSocket hibernation | `supported` | 19 | accept/tags/get、auto-response、serialize/deserialize attachment、reconstruction 和 restart 均闭环 | — |
| Vectorize | `supported_with_deviation` | 27 | stable post-beta `Vectorize` 的 7 个方法、异步持久 mutation、三种公开 score/order、namespace、indexed metadata filter/projection、restart recovery 与全 stable response surface 均闭环；beta `VectorizeIndex` 不在当前 Day1 合同 | `OC-VECTORIZE-001` |
| Workers AI / Markdown Conversion / AI Search | `supported_with_deviation` | 54 | 标准 `[ai]` 注入 `env.AI.aiGatewayLogId`/`toMarkdown`；AI Search namespace/instance/items/jobs、durable async 上传索引、keyword/vector/hybrid retrieval、chat/SSE 与配置内 OpenAI-compatible provider 闭环；完整 Workers AI inference 与 AutoRAG 不在声明范围 | `OC-AI-MARKDOWN-001`、`OC-AI-SEARCH-001` |

Workers observability 是管理面与平台 collector 能力，不计入 stable runtime-member denominator。当前
[`workersObservability`](../../share/cloudflare-capabilities.json) authority 明确支持固定 Wrangler 4.127.1 Script
Tails（`trace-v1`）、Workers Logs persistence、Telemetry keys/values、events/invocations query，以及 2026-09-03
真实 Cloudflare Dashboard wire 冻结的 Live Tail/heartbeat。日志由单机有界 `observability.sqlite` 保存，实时 session
在进程内且不 replay；每个执行 target 独立归属，caller tail 不聚合 nested target；不承诺全球顺序、hosted
retention/region metadata 或 exactly-once。Tail Workers、Streaming Tail
Workers、traces、非空 destinations、Logpush、calculations 和 saved queries 明确 unsupported，详见
[`OC-OBSERVABILITY-001`](p1-deviations.md)和[P7 完成设计](../implemented/p7-workers-logs-realtime-tail.md)。

Deployments、Static Assets、Service Binding、Workers Cache 与 Images 是平台配套能力，没有进入上述
stable-member denominator。Service Binding 的固定 P6 upload 已支持可选、受界、canonical JSON object
`props`；它是 immutable Version identity 的一部分，并只向目标 entrypoint 投影为 `ctx.props`。`remote` 仍不在
server 子集，单机 placement/discovery 边界继续由 `OC-SERVICE-001` 描述。AI 的 54 个目标
members/overloads 已进入 denominator，并按当前本地合同登记为 `supported_with_deviation`。
Analytics Engine、Browser Rendering、Hyperdrive、mTLS、Rate Limiting 与 Workers for
Platforms 明确为本轮非目标并在部署 authority 边界拒绝。完整 Workers AI inference 仍是非目标；存在标准
`env.AI` 只表示上表的 Markdown Conversion 与 AI Search 所需配置模型子集，不能因 upstream types 中存在其它 AI 名称而扩张能力声明。

deviation 规范文本、官方来源和边界见 [`p1-deviations.md`](p1-deviations.md)。其中 raw TCP 的 Day1
实现只有一个 `Network(allow = ["public"])` general-outbound authority；
`cloudflare:sockets.connect()`、`node:net`、`node:tls` 共用该地址层。Service/DO `Fetcher.connect()` 只能
通过 deployment 明确声明的 capability tunnel，不能成为第二条通用出网路径。runtime-source、binding
backend 和 workerd 内部 listener 仍仅监听 loopback。

## 关键实现说明

### 租户请求体预算

控制面已注册路由的 4 KiB（v4 为 64 MiB）声明长度检查不作用于 tenant ingress fallback，
包括 `/__workers/` 和自定义域名路由。租户 body 始终由 `WorkerdTransport` 按
`workers.max_request_body_bytes` 流式限额，声明长度与 chunked 请求都不能绕过；默认 16 MiB，
配置最大 64 MiB。这是单机 operator budget，不声称匹配
[Cloudflare account-plan 请求大小配额](https://developers.cloudflare.com/workers/platform/limits/#request-and-response-limits)。
回归由 HTTP 路由单测与 P0.2 真实 workerd/Wrangler 场景中的上传边界共同覆盖。

### 固定客户端的 Worker upload wire

Assets bulk upload 的 Axum multipart wire limit 在该路由显式设为 64 MiB，不再使用框架默认的
2 MiB；payload 仍按所有字段的 base64 bytes 累加执行 50 MiB budget，单文件解码后仍不超过
25 MiB。无 `Content-Length` 的 body 也受相同解析器与产品预算约束。固定 base64 multipart
路由回归包含大于 2 MiB 的二进制文件及超预算拒绝；这不是新的 Cloudflare 托管管理面差分证据。

固定 Wrangler 4.127.1 将 D1 配置的 `database_id` 投影为 Worker multipart binding 的 `id`；固定
`cloudflare@7.1.0` 的 typed `workers.scripts.update()` 则以 bracket field 发送 `database_id`。生产边界只在
该 SDK bracket wire、且 binding `type` 精确为 `d1` 时归一为内部唯一 `id`，同时出现两个字段、无法唯一分组
或其它 binding 使用该字段都会失败。binding 分组不依赖 JavaScript object 属性顺序；只有 closed P6 schema
存在唯一无损分区时才进入标准 Version authority。该客户端 wire 差异没有 tenant runtime 可观察语义，因此不
登记 runtime deviation ID；固定 SDK 真实 `ocd` Gate 同时验证 D1 binding 的持久投影和上传源码下载。
该 SDK Gate 的回读发生在同一个 ready `ocd` 进程内，本次只证明写入 authority 后的立即持久投影，不单独
声称 official SDK wrapper 已完成重启后回读资格；Version authority 的通用重启/恢复仍由独立真实进程 Gate 所有。

### D1 Time Travel retention

[Cloudflare D1 Time Travel](https://developers.cloudflare.com/d1/reference/time-travel/) 是自动启用、分钟级且保留
7/30 天的 PITR；普通 D1 Session bookmark 也可作为同一历史中的恢复位置。open-compute 的单机 SMB 合同不模拟
该日志型历史：普通 Worker mutation 只提交 live SQLite，不同步生成整库副本；export/import/time-travel
显式管理操作才建立 completed checkpoint。每个数据库硬限制为 8 个 checkpoint；transfer/restore intent 引用的
durable evidence 不会被提前回收，terminal transfer capability 过期后会删除其 authority 与 exact file 并释放 pin；
每库同时最多保留 8 个未过期 terminal transfer file。若尚未过期的 evidence 使系统无可回收点或 transfer file
达到上限，新显式操作会在复制或 mutation 前拒绝。timestamp 只解析仍保留的
显式点，restore 只接受精确 retained checkpoint；普通 Session bookmark 继续提供同库顺序可见性，但不因此自动
成为 restore point。两个 official time-travel route 因此标记为 `supported_with_deviation` 并关联 `OC-D1-001`，
不能外推成 Cloudflare always-on PITR 已实现。checkpoint/expired-transfer authority row 删除后若极低概率的 exact
file unlink 失败，会留下不可达 orphan；单机 SMB 当前接受该磁盘清理长尾，不引入启动扫描或日志型 GC 状态机。

### Service Binding `props`

固定 Wrangler 4.127.1 的 schema 把 `services[].props` 定义为传给目标 Worker `ctx.props` 的可选 object。
open-compute 在项目导入与 v4 multipart 边界要求 JSON object，执行 64 KiB、32 层深度上限和 canonical key
ordering；canonical bytes/digest 随 immutable Version 一起持久化。runtime admission 会重新验证 canonical bytes
与 descriptor digest，任何损坏都 fail closed；成功路径通过 stock workerd 的
`stub.getEntrypoint(name, { props })` 交付，`constructor`、`__proto__` 等普通 JSON key 不获得特殊含义。
这项本地实现不宣称 Cloudflare 的跨区域 placement，也不扩大 `remote` 支持范围。

### Queue producer `delivery_delay`

Cloudflare 当前 Queues/Wrangler 配置文档仍展示 producer binding 的 `delivery_delay`，但固定
Wrangler 4.127.1 的实际 validator 明确警告该字段已弃用且无效果，并要求通过 `wrangler queues update`
管理 Queue-level setting。P6 按固定客户端的可观察行为接受并忽略 upload metadata 中的该字段，不让它改写
Queue authority 或 immutable descriptor；`/queues/{queue_id}` 的 settings API 才是队列默认 delay 的
authority。官方文档与固定 CLI 的冲突在取得同版本 hosted management trace 前保持显式记录，不能用旧的
producer 文档文字推翻 pinned CLI，也不能把本地无效果行为写成已经完成的托管端一致性证据。

### Durable Object nested facets

pinned `workerd v1.20260830.1` 在 nested facet 上执行 clone/delete 会触发上游
`parent == kj::none` 失败。open-compute 不保留旧 facade 或版本分支，而是把 Cloudflare 可观察的逻辑 facet
path 直接映射为同一 object 下的稳定 hashed physical facet name；clone/delete 递归遍历逻辑 registry，
tenant 仍观察到原始嵌套 path、独立内容和删除语义。focused nested clone/delete 回归与真实 Cloudflare
portable fixture 的递归结果逐字段一致，因此该实现不是 observable deviation。

### Queue 托管行为

Queue producer 的 delay/body/batch 限制、content type 与异常类别按真实 Cloudflare 固定：invalid content
type 和空 batch 为 `TypeError`，超大 batch、负 delay 和超大 delay 为 `Error`。metrics 只比较托管端最终
可观察的不变量（backlog count/bytes 为正，oldest timestamp 缺省或为 `Date`），不把 hosted metrics 的
异步可见时机伪装成单机同步合同。

### 资源生命周期

Cron activation generation 从该 Worker 的全部持久 activation（含 tombstone）取最大值后递增。
移除全部 triggers 不会重置代次；重新启用相同表达式或回滚旧 Version 会创建新 activation，
相同 Version 的当前 staging/active 重试则保持同一身份。该规则保留单机 restart/reconcile 与
stale-generation fencing；不模拟 [Cloudflare Cron 的全球传播延迟](https://developers.cloudflare.com/workers/configuration/cron-triggers/)。
回归覆盖清空后重新打开 control/scheduler SQLite、重新启用、幂等重试，以及 P0.2 真实 scheduled dispatch。

Worker tombstone 在同一事务中释放 generic、Queue producer 和 Workflow binding referrer；immutable
deployment declaration 仍保留为历史 authority。Queue/Workflow/R2/D1/KV/DO 删除按当前 Day1 tombstone
模型确认无 live resource 后才允许同名重建，不保留旧 schema 或兼容清理分支。

### Wrangler Workflow 部署 prerequisite

固定 Wrangler 4.127.1 在 `workers_dev:false` 的 Workflow deploy 中，仍会于 Worker upload 后、Workflow
PUT 前读取 `GET /accounts/{account_id}/workers/subdomain`，并丢弃返回值。open-compute 将该只读 route 标为
`supported_with_deviation`：它返回以 `_` 开头、按 account 稳定派生的非 DNS label，只满足固定 CLI 的顺序
prerequisite，不创建 workers.dev DNS、listener、route 或注册 authority；对应 `PUT/DELETE` 继续不支持。
真实本地入口仍以 vendor Worker endpoints route 为准。该 route 与 capability 的关联 deviation 为
`OC-ACCOUNT-SUBDOMAIN-001`。

固定 Wrangler 4.127.1 创建 AI Search instance 前还会读取
`GET /accounts/{account_id}/ai-search/tokens`。单机实现只返回一个 account-scoped、稳定、无 secret 的
installation-managed metadata；不暴露 bearer token、provider credential 或 ciphertext，也不开放 token mutation。
该 route 标为 `supported_with_deviation` 并关联 `OC-AI-SEARCH-TOKEN-001`。

## Differential 与本地证据

2026-09-01 的同源 portable fixtures 已在真实 Cloudflare 与 open-compute 对照以下七项：Workers、Cache
API、KV、D1、R2、Durable Objects 和 Queues。公开 status/JSON 经合同允许的归一化后逐字段一致；每次只
创建唯一 `oc-p34-*` Worker 及 fixture 自有 binding，按精确 name/ID 删除并复查 absent，没有修改账号中
已有服务。DO fixture 包含递归 nested facet clone/delete；Queue fixture 包含 metrics、五类 producer 错误
和消费响应。这批证据属于 portable runtime/product differential，不是新的 P6 management qualification；它
没有证明 P6 `/client/v4` 资源命令、固定官方 SDK wire、multipart/Assets 上传或两个只读 prerequisite route
已经与 Cloudflare 托管管理面实测一致。后者仅由独立的
[P6 远端差分验收](../acceptance/p6-cloudflare-v4-differential-acceptance.md)关闭。

Workflow portable fixture 已实现并通过 open-compute 本地真实进程路径，但当前 Wrangler OAuth 对
Cloudflare Workflow inventory API 返回 `Authentication error [code: 10000]`，在 preflight 阶段即停止，
没有创建 Workflow 或 Worker。源码冻结后的七项合并复查又在 D1 inventory preflight 收到同一错误；该次
运行已先完成 Cache API 对照并精确清理，D1 及后续 fixture 未创建资源。此前已完成的 D1 和其它分项
qualification 仍是有效证据，但当前 token 不能生成新的合并报告。这个外部限制不使本地实现重新变为
`blocked`；账号权限条件解除前，不得声称 Workflow 已完成真实 Cloudflare differential qualification，
也不得把其它七项结果外推为“所有产品均与 Cloudflare 托管端实测一致”。

本地证据由 `p3-contract` 的 type/catalog/config/deviation/source 双射、产品 Gates、真实 pinned
workerd、SQLite 与选定的 Local/S3 object authority、restart/crash tests 和最终 workspace/coverage 共同组成。最终命令、报告和
实际限制记录在归档完成报告中；机器可读 capability/catalog 仍是支持状态的唯一 authority。
