# Cloudflare Workers 兼容矩阵

本页是 [`share/cloudflare-capabilities.json`](../../share/cloudflare-capabilities.json) 与
[`test/conformance/catalog.json`](../../test/conformance/catalog.json) 的人类可读索引，不建立第二份
能力真值。`ocd capabilities --json`、类型 inventory、contract catalog 和 Gate 共同定义当前
支持面。完成设计和 conformance 方案见
[Cloudflare Runtime 全量兼容改造](../implemented/p3-0-cloudflare-runtime-compatibility.md)与
[P3.4 Cloudflare conformance](../implemented/p3-4-cloudflare-conformance.md)。P6 当前管理合同及本地证据见
[P6 实现与验证](../implemented/p6-cloudflare-v4-wrangler-compatibility.md)；尚待外部账号条件解除的 runtime
Workflow 与 P6 management qualification 分别只记录在[既有剩余验收](../acceptance/p3-0-cloudflare-runtime-compatibility-acceptance.md)
和 [P6 远端差分验收](../acceptance/p6-cloudflare-v4-differential-acceptance.md)。

固定契约输入见 [`baseline.json`](../../test/conformance/baseline.json)。当前 formal pin 是
`workerd v1.20260905.0-open-compute-p1.b3e1a278`，revision
`b3e1a27840299f493d9425dc4d9972381d02ef23`，唯一
`effectiveCompatibilityDate` 为 `2026-09-08`；stable types 是
`@cloudflare/workers-types@5.20260830.1`。普通 Script/Version 配置不得选择其它 compatibility date 或任意 flags，也不保留旧
open-compute schema、descriptor、runtime 或 API 的兼容路径。官方在 compatibility date `2026-08-04`
起默认启用 Node.js compatibility，并明确此日期后的 `nodejs_compat` 是被 Wrangler/runtime 忽略的冗余
正向 flag（[官方 changelog](https://developers.cloudflare.com/changelog/post/2026-08-04-nodejs-compat-default/)、
[Compatibility Flags](https://developers.cloudflare.com/workers/configuration/compatibility-flags/)）。因此 P6 wire
只额外接受并逐 Version 原样持久化精确的单值 `["nodejs_compat"]`，其与空数组在 pinned
上述 fork 下使用同一平台语义；其它 flag、组合与所有其它日期继续 fail closed。对应
multipart、descriptor、runtime-source/loader 回归防止它扩成普通 Script 的可选历史模式。
`2026-09-08` 同时是官方 Python 3.14 / Pyodide 314.0.6 默认日期（[官方 changelog](https://developers.cloudflare.com/changelog/?product=workers)）；
正式 lock 内嵌该日期对应的唯一 bundle。

Dynamic Worker 的 `WorkerCode.compatibilityDate` / `compatibilityFlags` 是独立的官方
[Loader API 合同](https://developers.cloudflare.com/dynamic-workers/api-reference/)，由固定 fork 的
原生校验执行；它们只影响该 child，不改写 parent Version、平台 schema 或正式 pin。原生 upstream
日期/flag 分支继续保留，公开 child 不获 experimental trust。类型 fixture、原生 Loader 日期变体与
专用产品用例覆盖这一例外；不引入 open-compute 历史版本选择。

## 当前结论

目标 inventory 共 2,203 个 stable members/overloads：1,600 个 `supported`、597 个
`supported_with_deviation`、6 个 `blocked`。Dynamic Workers 的 19 个常用成员通过专用真实产品用例；
4 个 custom-limit 成员由 W2 实现，2 个 experimental-control 成员不在公开 W1 子集。
对应缺口显式登记在 catalog 的 `blockedGaps`；不得把原 2,178 个成员的历史验收当作 fork 的新验收。deviation 只描述单机 self-host 无法复制的 edge/全球拓扑、托管 fleet quota 或本地
authority 差异；它不代表缺方法、占位返回或半截实现。

| 产品                                         | 状态                           |  成员 | 当前实现与证据                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         | deviation                                         |
| -------------------------------------------- | ------------------------------ | ----: | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------- |
| Workers runtime                              | `supported_with_deviation`     | 1,580 | 1,556 个成员直接支持；24 个 raw-TCP 成员保留完整 API，仅隔离 hosted TCP policy/fleet limit 差异。latest 默认 Node.js、Web APIs、handlers、RPC、Cache、raw TCP 和配套 surface 均有 compile/stock-workerd/runtime case                                                                                                                                                                                                                                                                                                                   | `OC-WKR-TCP-001`、`OC-WKR-LIMIT-001`              |
| Dynamic Workers                              | `blocked`（19 已资格，6 缺口） |    25 | 三个正式平台的 native fork；load/get、七类模块、scoped env/RPC、tail、facet、4/10 原生计数与 restart/delete 产品路径。当前认证日期的 Python 使用 formal-lock 固定并随 `ocd` gzip 内嵌的 Pyodide bundle；其它官方 child 日期/flag 组合不属于单 bundle 离线资格。macOS Intel 与 Windows 仅手动编译，不属于 release 资格；custom limits 归 W2；实验 trust/streaming tails 不开放                                                                                                                                                          | `OC-WKR-LIMIT-001`                                |
| KV                                           | `supported_with_deviation`     |    52 | 单键/批量 overload、metadata、stream、list、`cacheStatus`、错误时序和恢复均闭环                                                                                                                                                                                                                                                                                                                                                                                                                                                        | `OC-KV-001`                                       |
| R2                                           | `supported_with_deviation`     |   110 | object/body/list/options、全部 checksum、SSE-C、storage class、条件写、multipart、opaque physical key、持久 intent/reconcile 和 restart 均闭环；single/part/multipart ETag 公式及 lowercase-hex `ssecKeyMd5` 与官方 Worker API 一致                                                                                                                                                                                                                                                                                                    | `OC-R2-001`                                       |
| D1                                           | `supported_with_deviation`     |    36 | database/session/prepared statement/result/meta、opaque bookmark、原子 batch/exec、错误转换和非 alpha `dump()` 拒绝均闭环                                                                                                                                                                                                                                                                                                                                                                                                              | `OC-D1-001`                                       |
| Durable Objects                              | `supported_with_deviation`     |   115 | namespace/ID/stub/native RPC facet、state、sync KV/SQL、transaction、alarm、hibernation、output gate、显式 connect tunnel，以及 Cache API/声明 binding 的对象内可用性均闭环；112 个成员使用 `OC-DO-001`，3 个 connect 成员使用 TCP/limit deviation                                                                                                                                                                                                                                                                                     | `OC-DO-001`、`OC-WKR-TCP-001`、`OC-WKR-LIMIT-001` |
| DO Alarms                                    | `supported`                    |     7 | get/set/delete、handler、retry/restart authority 均闭环                                                                                                                                                                                                                                                                                                                                                                                                                                                                                | —                                                 |
| Queues                                       | `supported_with_deviation`     |    63 | producer、consumer、`v8`、metrics、delay、ack/retry、output gate、at-least-once recovery 均闭环                                                                                                                                                                                                                                                                                                                                                                                                                                        | `OC-QUEUE-001`                                    |
| Cron                                         | `supported_with_deviation`     |    26 | scheduled handler、`noRetry()`、Workflow schedules、projection/recovery 均闭环                                                                                                                                                                                                                                                                                                                                                                                                                                                         | `OC-CRON-001`                                     |
| Workflows                                    | `supported_with_deviation`     |    72 | binding/instance/batch/delete、structured clone、step config、parallel DAG、event、restart-from-step、rollback、DO output gate，以及 Cache API/声明 binding 的 Workflow 内可用性均闭环                                                                                                                                                                                                                                                                                                                                                 | `OC-WORKFLOW-001`                                 |
| Cache API                                    | `supported_with_deviation`     |    14 | `Cache`/`CacheStorage`、vary/range/condition、purge、restart、自动 cache 协作及 Worker/DO/Workflow execution-context matrix 均闭环                                                                                                                                                                                                                                                                                                                                                                                                     | `OC-CACHE-001`、`OC-CACHE-002`                    |
| Version Metadata                             | `supported`                    |     3 | `id`、`tag`、`timestamp` 由 immutable deployment authority 注入                                                                                                                                                                                                                                                                                                                                                                                                                                                                        | —                                                 |
| WebSocket hibernation                        | `supported`                    |    19 | accept/tags/get、auto-response、serialize/deserialize attachment、reconstruction 和 restart 均闭环                                                                                                                                                                                                                                                                                                                                                                                                                                     | —                                                 |
| Vectorize                                    | `supported_with_deviation`     |    27 | stable post-beta `Vectorize` 的 7 个方法、异步持久 mutation、三种公开 score/order、namespace、indexed metadata filter/projection、restart recovery 与全 stable response surface 均闭环；beta `VectorizeIndex` 不在当前 Day1 合同                                                                                                                                                                                                                                                                                                       | `OC-VECTORIZE-001`                                |
| Workers AI / Markdown Conversion / AI Search | `supported_with_deviation`     |    54 | 标准 `[ai]` 注入 `env.AI.aiGatewayLogId`/`toMarkdown`；统一 registry 覆盖 62 个 Cloudflare 文档候选并安全公布 59 个 AI Search／18 个 Markdown 格式，本地三语言 OCR、扫描 PDF、可选 OpenAI-compatible VLM、`chunk: false`、bounded durable parse cache 与同 account R2 source 已接入同一 indexing contract；R2 pause、显式 item/job、extensionless MIME、`r2:<bucket>` source ID、metadata filter、排序、删除 payload 和 bounded completion wait 走同一 Day1 路径；完整 Workers AI inference、外部 R2/S3 source 与 AutoRAG 不在声明范围 | `OC-AI-MARKDOWN-001`、`OC-AI-SEARCH-001`          |
| Artifacts                                    | `supported_with_deviation`     |    53 | namespace/repository/token、公开 HTTPS import、独立 fork、对象读取、Git Smart HTTP v1/v2、固定 Wrangler 4.127.1 与 pinned Worker binding 闭环；bare Git repository 与 SQLite metadata 位于单机 data-dir                                                                                                                                                                                                                                                                                                                                | `OC-ARTIFACTS-001`                                |

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
`props`；它是 immutable Version identity 的一部分，并只向目标 entrypoint 投影为 `ctx.props`。默认及命名
Service fetch 返回的 WebSocket 使用 workerd 原生 handoff；目标为 hibernatable Durable Object 时不插入
JavaScript relay，Service invocation/version pin 随最终公开 socket tunnel 存活并在连接关闭后释放。`remote` 仍不在
server 子集，单机 placement/discovery 边界继续由 `OC-SERVICE-001` 描述。AI 的 54 个目标
members/overloads 已进入 denominator，并按当前本地合同登记为 `supported_with_deviation`。
Analytics Engine、Browser Rendering、Hyperdrive、mTLS、Rate Limiting 与 Workers for
Platforms 明确为本轮非目标并在部署 authority 边界拒绝。完整 Workers AI inference 仍是非目标；存在标准
`env.AI` 只表示上表的 Markdown Conversion 与 AI Search 所需配置模型子集，不能因 upstream types 中存在其它 AI 名称而扩张能力声明。

### Cloudflare Artifacts

Artifacts 按 [REST API](https://developers.cloudflare.com/artifacts/api/rest-api/)、
[Git protocol](https://developers.cloudflare.com/artifacts/api/git-protocol/)、
[Workers binding](https://developers.cloudflare.com/artifacts/api/workers-binding/) 与固定
`wrangler@4.127.1` 实现。namespace/repository list 的公开 REST 合同使用 `limit` + opaque `cursor`；固定
Wrangler 4.127.1 的通用分页客户端仍发送 `page` 并读取 page metadata，因此同一路由仅为该固定客户端接受
`page`，不能扩成历史 API 模式。token list 保持官方 `page` / `per_page`。repo token 精确采用
`art_v1_<40 lowercase hex>?expires=<unix_seconds>`；Bearer 使用完整值，Git Basic password 使用 `?expires`
之前的 secret，plaintext 只在创建响应出现，SQLite 只保存 keyed digest、scope、expiry 与 revoke metadata。

固定 `@cloudflare/workers-types@5.20260830.1` 是 runtime surface 的类型 authority：其 `ArtifactsRepo` 暴露
metadata、`createToken`、`listTokens`、`revokeToken` 和 `fork`，共 53 个 Artifacts members/overloads。
当前网页文档额外展示的 `log`、`readCommit`、`readTree` 不在该固定类型包中，因此本轮不手写扩展类型，也不把
这些 docs-only Worker methods 宣称为已支持；相同对象读取能力仍通过已声明的 REST routes 提供。待正式 pin
升级且类型、workerd、Wrangler 与 differential evidence 一致时再直接更新唯一实现。

本地 authority、capacity 与 Cloudflare 托管服务的差异见 `OC-ARTIFACTS-001`。名称校验遵循官方规则：首字符
必须为 ASCII 字母或数字，其余只能为字母数字、`.`、`_`、`-`；jurisdiction 因单机无法提供真实 geographic
placement 而 fail closed。import 只允许无 credential/query 的公开 HTTPS remote，禁用 proxy/redirect，并把
一次 DNS 解析得到的公开地址固定到请求，拒绝 loopback、private、link-local、metadata 与 IPv4-mapped private
地址。Git push/import/fork、删除 lease drain、启动恢复、snapshot/restore 和完整性失败均保留 fail-closed
边界；upload-pack 的 `want` 还必须对应当前公告 ref，不能用已知 SHA-1 读取不可达对象。管理面与 Worker
binding 的应用错误使用官方 Artifacts `101xx`/`102xx`/`103xx`/`104xx` 数字码。具体实现与验收见
[P14 Artifacts](../implemented/p14-cloudflare-artifacts.md)。

deviation 规范文本、官方来源和边界见 [`p1-deviations.md`](p1-deviations.md)。其中 raw TCP 的 Day1
实现只有一个 `Network(allow = ["public"])` general-outbound authority；
`cloudflare:sockets.connect()`、`node:net`、`node:tls` 共用该地址层。Service/DO `Fetcher.connect()` 只能
通过 deployment 明确声明的 capability tunnel，不能成为第二条通用出网路径。runtime-source、binding
backend 和 workerd 内部 listener 仍仅监听 loopback。

## 关键实现说明

### Worker、Durable Object 与 Workflow capability matrix

自动 Workers Caching 只包裹普通 Worker 的 HTTP `fetch` entrypoint；Durable Object 调用和 Workflow
执行不进入该自动缓存层，`ctx.cache` 也不向这两类执行暴露。全局 `caches.default` / `caches.open()` 是与其
独立的编程式 Cache API，因此在 Worker、Durable Object 和 Workflow 三种环境中均可用。配置在 immutable
Version 上的 Images、当前声明子集内的 AI、Version Metadata 及其它产品 binding 同样按原名注入 DO 的
`this.env` 与 Workflow 的 `this.env`。官方依据是 [Workers Caching invocation limitations](https://developers.cloudflare.com/workers/cache/limitations/)、
[Cache API](https://developers.cloudflare.com/workers/runtime-apis/cache/)、[bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/)
以及官方 Workflow 中直接使用 `this.env.AI` 的[示例](https://developers.cloudflare.com/workflows/examples/wait-for-event/)。

本地真实 pinned-workerd 回归在同一 immutable Version 上验证 DO 与 Workflow 的 default/named Cache API、
Images/AI/Version Metadata binding 可见性、Service Binding 共存，并断言两种 context 的自动 caching 仍关闭。
Cloudflare-hosted Workflow differential 仍受下文账号权限限制；本地结果不外推成尚未执行的 hosted 证据。

### R2 上传调度与完整性

`uploadPart` 的 staging source 只携带路径和精确长度，不计算不被后端消费的五种完整对象摘要。
S3 adapter 仍计算并发送 `Content-MD5`，Local backend 仍执行自己的内容完整性校验；part ETag、
complete ETag、SSE-C、分片大小校验和 SQLite multipart authority 不变。
当前 Day1 对象 metadata 使用必需的 `oc-r2-http-fields` 六位 presence mask 记录 tenant 显式设置的
HTTP 字段；S3 provider 自行补入的默认 header 不成为 R2 用户 metadata。显式字段在后端丢失仍拒绝，
缺少/损坏该标记也拒绝，不对旧开发对象 backfill。这避免无 `httpMetadata` 的 multipart 在 Adobe S3Mock
自动返回 `Content-Type: application/octet-stream` 时被误判为 complete 元数据损坏。
普通 Worker/管理面 PUT 所需的 MD5、SHA-1/256/384/512 计算移入有界 blocking task，CPU 并发上限沿用
`r2.max_concurrent_uploads`。任务持有临时文件、staging 字节配额及 CPU permit，直到计算真正结束；
取消等待不把尚在执行的哈希误当成已经清理。body staging 继续流式写入并受总字节预算约束。
成功响应仍等待后端保存及元数据提交，未改成后台接受任务；30 秒 runtime response-header deadline 未延长。
这些实现调整不改变 [Cloudflare R2 Worker API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/)
的 PUT checksum、multipart 返回字段及 complete 后可见性合同，也不新增 topology deviation。

### 租户请求体预算

控制面已注册路由的 4 KiB（v4 为 64 MiB）声明长度检查不作用于 tenant ingress fallback，
包括 `/__workers/` 和自定义域名路由。租户 body 始终由 `WorkerdTransport` 按
固定的 `100000000` bytes 流式限额，旧 `workers.max_request_body_bytes` 配置已删除。
这个十进制 100 MB 值来自 [Cloudflare account-plan 请求大小最低 baseline](https://developers.cloudflare.com/workers/platform/limits/#request-and-response-limits)，
不代表复刻商业 plan。声明长度与 chunked overflow 的 413 定向回归已在缩小预算下通过；生产 100 MB 边界、最终
stock-workerd/Wrangler Gate 与 hosted differential 尚未通过，不能把配置值一致称为已完成兼容性验收。
测试代码可通过仅在 `test-support` 暴露的 setter 缩小预算；生产不能通过该路径改变 Standard 值。
现有 30 秒 host response-header deadline 仍是尚未资格化的本地 transport policy，其失败归类为
runtime unavailable，不宣称执行 CPU limit 或产生 `exceededCpu`。原生 limits 已选择用户 fork 路线，
执行器与完整验收仍待完成，局部实施记录见 [workerd W2](../workerd/w2-standard-limits.md)。

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

### Service Binding WebSocket handoff

Cloudflare 的 [Service Binding HTTP contract](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/http/)
允许调用方把目标 Worker 的响应直接返回；[Durable Object WebSocket Hibernation API](https://developers.cloudflare.com/durable-objects/best-practices/websockets/)
则要求客户端连接在对象 eviction 后继续存在，并在后续消息到达时重建对象。open-compute 因此把 Service fetch
返回的原生 `Response.webSocket` 沿调用链直接交给最终 workerd/`ocd` upgrade tunnel，不再通过已 `accept()` 的
普通 `WebSocketPair` 做 JavaScript 双向转发。私有 handoff handle 只在系统模块与 loopback response header
之间传递；tenant facade 会移除该 header，最终公开响应也由 Rust sanitizer 移除。Rust tunnel 持有 Service
operation lease，30 秒普通调用 deadline 不回收活跃 socket 的 target/caller pins；连接 EOF、upgrade 失败或
workerd generation 退出时幂等释放。

本地 pinned-workerd 产品回归覆盖默认和命名 Service fetch 到 `ctx.acceptWebSocket()` Durable Object，默认路径
保持 65 秒后再发送 text/binary frame，证明连接跨过原 30 秒调用 deadline 后仍可用；同时检查 target pin 在连接
期间保留、客户端关闭后归零。该证据验证单机原生 handoff 与 hibernation-compatible ownership，不外推为
Cloudflare 跨区域 placement 行为。

### Queue producer `delivery_delay`

Cloudflare 当前 Queues/Wrangler 配置文档仍展示 producer binding 的 `delivery_delay`，但固定
Wrangler 4.127.1 的实际 validator 明确警告该字段已弃用且无效果，并要求通过 `wrangler queues update`
管理 Queue-level setting。P6 按固定客户端的可观察行为接受并忽略 upload metadata 中的该字段，不让它改写
Queue authority 或 immutable descriptor；`/queues/{queue_id}` 的 settings API 才是队列默认 delay 的
authority。官方文档与固定 CLI 的冲突在取得同版本 hosted management trace 前保持显式记录，不能用旧的
producer 文档文字推翻 pinned CLI，也不能把本地无效果行为写成已经完成的托管端一致性证据。

### Dynamic Workers 生命周期

普通 Worker 的 public Loader namespace 由 account / Script / binding 的不可变身份派生，跨 Version
回滚保持一致；删除重建同名 Script 使用新身份。原生 cache 有界且可撤销，命中不是公共保证。
平台对已执行 Version 保留保守的 background-work hold，直到监督器证明 workerd generation 已退出；
期间 Script DELETE 返回 409，而不是把响应结束当作全部工作结束。退出后删除走正常 drain、SQLite
删除与 namespace revoke；不自动重启其他 Worker 来完成删除。专用产品用例验证拒绝后仍可调用、
重启后删除、独立 Script 可用及同名重建。`force=true` 仍不支持。

### Durable Object nested facets

此前 stock pin `workerd v1.20260830.1` 在 nested facet 上执行 clone/delete 会触发上游
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
