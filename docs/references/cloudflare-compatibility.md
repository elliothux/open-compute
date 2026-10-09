# Cloudflare Workers 兼容矩阵

本页是 [`share/cloudflare-capabilities.json`](../../share/cloudflare-capabilities.json) 与
[`test/conformance/catalog.json`](../../test/conformance/catalog.json) 的人类可读索引，不建立第二份
能力真值。`ocd capabilities --json`、类型 inventory、contract catalog 和 Gate 共同定义当前
支持面。完成设计和 conformance 方案见
[Cloudflare Runtime 全量兼容改造](../implemented/p3-0-cloudflare-runtime-compatibility.md)与
[P3.4 Cloudflare conformance](../implemented/p3-4-cloudflare-conformance.md)。当前 CLI 合同见
[P20 cf 迁移](../implemented/p20-cf-cli-migration.md)，此前管理协议证据见
[P6 实现与验证](../implemented/p6-cloudflare-v4-wrangler-compatibility.md)。当前账号权限不足以运行真实 Cloudflare
Workflow 与完整 P6 management 对照，因此不声明这两部分的托管端一致性。

固定契约输入见 [`baseline.json`](../../test/conformance/baseline.json)，正式 runtime 身份见
[`workerd.lock.json`](../../packages/runtime/workerd.lock.json)。每个普通 Script/Version 持久化上传的
`compatibility_date` 与 `compatibility_flags` 原值；正式 pinned workerd 以
`CompatibilityDateValidation::CODE_VERSION` 作为唯一 admission authority。平台不维护日期最小值、离散日期列表、flag
allowlist、alias 或默认改写。唯一 boundary default 是官方 Workers upload API 明确规定的行为：省略
`compatibility_date` 时使用 oldest date `2021-11-02`（[Compatibility dates](https://developers.cloudflare.com/workers/configuration/compatibility-dates/)）；该值在进入 immutable Version 前物化，之后仍由 workerd 验证。日期不得超过 binary 编译的 maximum 或当前 UTC 日期；格式、未知/重复/冲突 flag、日期默认值、显式
enable/disable、implication 和 experimental 条件都由同一个 workerd compile path 判断。Cloudflare 对日期与 flag 的公开语义见
[Compatibility flags](https://developers.cloudflare.com/workers/configuration/compatibility-flags/)。

构建从 exact binary 的 schema reflection 生成 deterministic compatibility catalog，并把 catalog digest、binary maximum、fork
revision 与四目标 binary/archive digest 一起固定。`ocd capabilities --json` 与
`GET /client/v4/open-compute/capabilities` 投影同一份内嵌 catalog；该输出用于发现，不替代候选 Version 的真实 workerd validation。
平台 system Workers 继续使用 formal lock 中独立的 `systemCompatibilityDate` / `systemCompatibilityFlags`，这些值不会注入 tenant
Version。compatibility date 只选择当前 binary 内的运行时行为，不选择旧 binary、旧 schema、旧 artifact 或旧持久化模型。

管理合同使用固定 cf `1.0.0-beta.12`、Cloudflare OpenAPI revision `780de88d0324b007c907a1782259b1a0e5e87c7d`（blob
`a37bda40bc108c44c135ee69ff83fe666ce4e25d`）与官方 SDK `cloudflare@7.2.0`。Script/Version
上传统一使用一个 JSON `metadata` multipart part 加具名 module parts；SDK 扩展覆盖官方 SDK 尚未声明的
Artifacts binding。正式 runtime lock 的 `workersSdk` 字段记录 integration reference 与 cf/Vite pin；CLI 版本只有
`cfVersion`，不接受旧字段或别名，执行路径只使用 cf。
当前 scanner 已发现 revision `01a855ec4bd180a1173f1b4587ef0fd0ca9f55e6` 新增 AI Search item schema，而 stable SDK 尚未同步，下一轮继续
保持 `blocked`，不改变上述 formal pin。

Worker bundle 使用通用本地结构限制：4096 modules、单 module 8 MiB、总 module bytes 32 MiB、manifest 1 MiB，
canonical artifact 默认 34 MiB。multipart streaming、离线 base64 encoder 和 framework importer 保持同一预算；
扩大 `workers.max_bundle_bytes` 不解除结构限制。Cloudflare 当前声明 64 MiB uncompressed Worker size，
本地差异记录于 [`OC-WKR-LIMIT-001`](p1-deviations.md)，不声明 hosted quota parity。Python 普通部署使用正式 R4 workerd pin、Pyodide `314.0.6_2026-08-17_6` 与未修改 SDK 1.9.2。Main、完整三框架、Services、Queues、self-owned DO、Workflows 与 Runtime 的九个 ordinary case 已在同一 workspace coverage 轮次通过；Django/FastAPI 包含流式响应，Flask 仅声明普通 HTTP/template。固定输入与测试合同见 [Python Workers](testing.md#python-workers)。最终未插桩 workspace 验收与具体报告由阶段完成记录拥有。

Python Runtime 的单轮产品 case 包含原版 Pyodide requests 2.33.1 与同步/异步 httpx 0.28.1 的实际 HTTP 路径、HTTP exception、流、超时、连接拒绝、取消后的有界清理、native subrequest quota 和 restart/rollback。取消 Python task 不作为底层 Fetch 立即 abort 的保证；TLS、数据库驱动、AI/API client 与 HTTP MCP 仍留在 [#128](https://github.com/elliothux/open-compute/issues/128)。当前 Dynamic 基线另记录 fresh child、cached-key 尝试、普通 preparation 后和 daemon restart 后的结果；不拿普通 snapshot 或缓存 key 当成功 warm child。正式输入、执行结果与复现命令由 [P21](../implemented/p21-python-workers.md) 拥有，Dynamic 支持仍留在 [#126](https://github.com/elliothux/open-compute/issues/126)。

Python Queue 的 ordinary case 覆盖 json/text/bytes/V8 Date、50,000 中文字符、metadata、ack/retry/DLQ、暂停与 fresh restart、两个 Version 和 rollback。正式 fork 使用官方单 batch 参数调用 Python handler；force 只授权 backlog purge，不绕过 live referrer 删除保护。Python scheduled 的 controller/env/ctx、type/noRetry/waitUntil、短签名与输入拒绝已有 source/SDK 组件资格；这些组件不单独证明完整 Python Cron scheduler recovery。

普通 Python DO case 分别使用两个语言各自 self-owned namespace，覆盖 native ID/name、fetch/RPC、SQL/KV、abort replacement、持久 alarm、WebSocket 消息/attachment/close、promotion/rollback 和 SIGKILL orphan recovery。公开上传拒绝 cross-Script DO/Workflow，不声明 PITR、hibernation eviction 或 live socket 跨进程死亡。

Dynamic Worker 的 `WorkerCode.compatibilityDate` / `compatibilityFlags` 是独立的官方
[Loader API 合同](https://developers.cloudflare.com/dynamic-workers/api-reference/)，并经过与普通 Version 相同的 pinned workerd
原生校验；它们只影响该 child，不改写 parent Version、平台 schema 或正式 pin。catalog 会标出 experimental input，而正式进程的
`--experimental` 决定该 binary 是否接受它；平台不再额外过滤。类型 fixture、原生 Loader 日期/flag 矩阵与专用产品用例覆盖该合同，
不引入 open-compute 历史版本选择。官方 [Dynamic Workers API reference](https://developers.cloudflare.com/dynamic-workers/api-reference/) 说明
`allowExperimental` 需要调用方自身的 `experimental` flag，且 experimental flags 不能在 hosted production 启用；当前 self-host formal pin
显式运行 experimental mode，因此其 catalog 可发现并由 binary 接受的 experimental 输入是本地 superset，不声明 hosted-production availability。

## 当前结论

目标 inventory 共 2,588 个 stable members/overloads：1,657 个 `supported`、597 个
`supported_with_deviation`、334 个 `blocked`。P22 新增 Browser 的 332 个 stable members/overloads，
当前完整 backend/protocol qualification 未完成，全部保持 `blocked`；已验证的固定客户端路径见下方 Browser Run。Dynamic Workers 的 23 个公开成员通过 compile 与专用真实产品
用例，其中 4 个 custom-limit 成员由 W2 完成配置、API、原生执行与恢复资格；仅 2 个
experimental-control 成员不在公开子集。
对应缺口显式登记在 catalog 的 `blockedGaps`；不得把旧 inventory 的历史验收当作当前 fork 的新验收。deviation 只描述单机 self-host 无法复制的 edge/全球拓扑、托管 fleet quota 或本地
authority 差异；它不代表缺方法、占位返回或半截实现。

| 产品                                         | 状态                                            |  成员 | 当前实现与证据                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            | deviation                                         |
| -------------------------------------------- | ----------------------------------------------- | ----: | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------- |
| Workers runtime                              | `supported_with_deviation`                      | 1,580 | 1,556 个成员直接支持；24 个 raw-TCP 成员保留完整 API，仅隔离 hosted TCP policy/fleet limit 差异。latest 默认 Node.js、Web APIs、handlers、RPC、Cache、raw TCP 和配套 surface 均有 compile/stock-workerd/runtime case                                                                                                                                                                                                                                                                                                                                                                                                      | `OC-WKR-TCP-001`、`OC-WKR-LIMIT-001`              |
| Dynamic Workers                              | `blocked`（23 个 API 成员已资格，2 个实验缺口） |    25 | native fork 提供 load/get、七类模块、scoped env/RPC、tail、facet、原生计数与 restart/delete。W2 的 limits authority 由当前 cf 配置归一化到 v4 snake-case、Settings clone、逐维 ceiling 与 delegated Loader attenuation；原生执行 invocation CPU/subrequest、128 MiB memory、1 s startup 和响应头阶段 6-slot connection limit，超限 isolate 从 immutable Version 重建，公开返回官方 1101/1102/10021 分类。Dynamic Python cold boot 因本地没有 Cloudflare hosted deploy-time 预计算而未资格化，不放宽官方 1 s startup limit；2 个实验 trust/streaming tails 不开放                                                          | `OC-WKR-LIMIT-001`                                |
| KV                                           | `supported_with_deviation`                      |    52 | 单键/批量 overload、metadata、stream、list、`cacheStatus`、错误时序和恢复均闭环                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           | `OC-KV-001`                                       |
| R2                                           | `supported_with_deviation`                      |   110 | object/body/list/options、全部 checksum、SSE-C、storage class、条件写、multipart、opaque physical key、持久 intent/reconcile 和 restart 均闭环；single/part/multipart ETag 公式及 lowercase-hex `ssecKeyMd5` 与官方 Worker API 一致                                                                                                                                                                                                                                                                                                                                                                                       | `OC-R2-001`                                       |
| D1                                           | `supported_with_deviation`                      |    36 | database/session/prepared statement/result/meta、opaque bookmark、原子 batch/exec、错误转换和非 alpha `dump()` 拒绝均闭环                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 | `OC-D1-001`                                       |
| Durable Objects                              | `supported_with_deviation`                      |   115 | namespace/ID/stub/native RPC facet、state、sync KV/SQL、transaction、alarm、hibernation、output gate、显式 connect tunnel，以及 Cache API/声明 binding 的对象内可用性均闭环；112 个成员使用 `OC-DO-001`，3 个 connect 成员使用 TCP/limit deviation                                                                                                                                                                                                                                                                                                                                                                        | `OC-DO-001`、`OC-WKR-TCP-001`、`OC-WKR-LIMIT-001` |
| DO Alarms                                    | `supported`                                     |     7 | get/set/delete、handler、retry/restart authority 均闭环                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   | —                                                 |
| Queues                                       | `supported_with_deviation`                      |    63 | producer、consumer、`v8`、metrics、delay、ack/retry、output gate、at-least-once recovery 均闭环                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           | `OC-QUEUE-001`                                    |
| Cron                                         | `supported_with_deviation`                      |    26 | scheduled handler、`noRetry()`、Workflow schedules、projection/recovery 均闭环                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            | `OC-CRON-001`                                     |
| Workflows                                    | `supported_with_deviation`                      |    72 | binding/instance/batch/delete、structured clone、step config、parallel DAG、event、restart-from-step、rollback、DO output gate，以及 Cache API/声明 binding 的 Workflow 内可用性均闭环                                                                                                                                                                                                                                                                                                                                                                                                                                    | `OC-WORKFLOW-001`                                 |
| Cache API                                    | `supported_with_deviation`                      |    14 | `Cache`/`CacheStorage`、vary/range/condition、purge、restart、自动 cache 协作及 Worker/DO/Workflow execution-context matrix 均闭环                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        | `OC-CACHE-001`、`OC-CACHE-002`                    |
| Version Metadata                             | `supported`                                     |     3 | `id`、`tag`、`timestamp` 由 immutable deployment authority 注入                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           | —                                                 |
| WebSocket hibernation                        | `supported`                                     |    19 | accept/tags/get、auto-response、serialize/deserialize attachment、reconstruction 和 restart 均闭环                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        | —                                                 |
| Vectorize                                    | `supported_with_deviation`                      |    27 | stable post-beta `Vectorize` 的 7 个方法、异步持久 mutation、三种公开 score/order、namespace、indexed metadata filter/projection、restart recovery 与全 stable response surface 均闭环；beta `VectorizeIndex` 不在当前 Day1 合同                                                                                                                                                                                                                                                                                                                                                                                          | `OC-VECTORIZE-001`                                |
| Workers AI / Markdown Conversion / AI Search | `supported_with_deviation`                      |    54 | 标准 `[ai]` 注入 `env.AI.aiGatewayLogId`/`toMarkdown`；统一 registry 覆盖 62 个 Cloudflare 文档候选并安全公布 59 个 AI Search／18 个 Markdown 格式，本地三语言 OCR、扫描 PDF、可选 OpenAI-compatible VLM、`chunk: false`、bounded durable parse cache 与同实例 R2 source 已接入同一 indexing contract；R2 pause、显式 item/job、extensionless MIME、`r2:<bucket>` source ID、metadata filter、排序、删除 payload 和 bounded completion wait 走同一 Day1 路径；namespaced `open-compute:manual` source 是隔离的 API superset，不进入这 54 个官方成员；完整 Workers AI inference、外部 R2/S3 source 与 AutoRAG 不在声明范围 | `OC-AI-MARKDOWN-001`、`OC-AI-SEARCH-001`          |
| Artifacts                                    | `supported_with_deviation`                      |    53 | namespace/repository/token、公开 HTTPS import、独立 fork、对象读取、Git Smart HTTP v1/v2、历史固定客户端 wire 与当前 pinned Worker binding 闭环；当前 CLI 使用 cf；bare Git repository 与 SQLite metadata 位于单机 data-dir                                                                                                                                                                                                                                                                                                                                                                                               | `OC-ARTIFACTS-001`                                |

当前 AI Search query surface 只声明文本输入：接受 text `query` 与 string-content messages；image、file 和 text+image
multimodal query 不在当前支持范围并在 public boundary fail closed。文档 ingestion 的图片、扫描 PDF、OCR 与可选 VLM description
只生成可索引文本，不代表支持多模态 query。所有成功文本 Search response 返回官方 `query_kind: "text"`。Embedding、rewrite 和
chat 使用 operator-pinned OpenAI-compatible endpoint；reranking 使用独立的 `cohere_rerank_v2` 或 `rerank_v1` catalog，不再通过
Chat Completions prompt。当前实现按 vector threshold → fusion → metadata boost → dedicated rerank → rerank threshold → final limit
执行，并分别保留 retrieval、raw keyword 与 reranking score。2026-09-30 使用 `cf` 对临时 hosted AI Search instance 的固定
differential 确认：普通 vector 结果的顶层 `score` 等于 vector score；keyword score 先按分支最大值归一化；metadata boost 的总权重为
`0.3`，与 retrieval score 相加后按候选最大值归一化；rerank input 可超过最终 `max_num_results`，最终限制在 rerank 后应用；默认 rerank
threshold 为 `0.4`；rerank 后顶层 `score` 等于 `reranking_score`，namespace merge 也按该分数排序；keyword-only candidate 不受 vector
`match_threshold` 过滤。临时 instance、items 与 metadata 均已删除并复查不存在。

Workers observability 是管理面与平台 collector 能力，不计入 stable runtime-member denominator。当前
[`workersObservability`](../../share/cloudflare-capabilities.json) authority 明确支持官方 Script
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
Analytics Engine、Hyperdrive、mTLS、Rate Limiting 与 Workers for Platforms 继续为非目标并在部署 authority 边界拒绝。
Browser Run 已实现 external CDP 和 managed backend：未配置可用 backend 时拒绝 Browser binding；managed
保留 Chrome 原生 sandbox，并在 CDP 边界限制宿主文件能力、由平台分配临时下载目录，不叠加外层 sandbox。
完整 stable overload 与协议证据仍存在 blocked gaps；网络过滤由 operator 管理，不声明 public-only egress。
公开 Browser Run 路由共用 `/browser-run` 与 `/browser-rendering` 两个前缀：前者依据
[2026-09-26 官方 session 文档](https://developers.cloudflare.com/browser-run/cdp/session-management/)，后者依据
固定 Cloudflare SDK 7.2.0 的 `resources/browser-rendering/devtools/browser/browser.mjs` 和
`resources/browser-rendering/json.mjs`。此官方合同不按 open-compute 历史版本或 compatibility date 选择实现；
`public_browser_api_preserves_raw_sessions_and_enforces_account_and_roles` 覆盖两者的 account、role、session 与 action 边界。
不能沿用旧 non-target 声明。完整 Workers AI inference 仍是非目标；存在标准
`env.AI` 只表示上表的 Markdown Conversion 与 AI Search 所需配置模型子集，不能因 upstream types 中存在其它 AI 名称而扩张能力声明。

### Browser Run

真实正式 pinned workerd 与 chrome-headless-shell 覆盖固定 cf 的上传/session/Live View、
`@cloudflare/puppeteer` 1.4.0、`@cloudflare/playwright` 1.3.6 默认页面及额外 context、
九项 Quick Actions、SDK 7.2.0 的原始响应、DevTools JSON/WebSocket 和 restart。
`p22-browser-run` 的三个产品用例拥有这些路径；权限、CDP/context/file fencing、下载策略、
生命周期、SQLite history、metrics、Custom AI 和 Dashboard 另有各自的回归。
这些证据不将 Browser 的全部 332 个 stable members/overloads 自动变为 qualified。

已知证据边界：

- [Screenshot 文档](https://developers.cloudflare.com/browser-run/quick-actions/screenshot-endpoint/)
  规定原始 PNG，SDK 返回类型却声明 JSON；平台返回原始 media body，SDK 使用 `.asResponse()`。
- [Scrape 文档](https://developers.cloudflare.com/browser-run/quick-actions/scrape-endpoint/)
  的 inner HTML/rendered text 与固定 types/SDK 的 outer HTML/text 和 results shape 存在冲突。
- [CDP 文档](https://developers.cloudflare.com/browser-run/cdp/) 的 target close 使用 DELETE，
  固定 SDK 使用 GET；当前采用固定 SDK 的 GET，不增加猜测的 method alias。
- **acceptance / accepted limitation（用户于 2026-10-08 接受）**：Puppeteer 的
  `connectionStartTime` 声明为 string，Playwright 声明为 number；binding 采用 Puppeteer
  类型，公开 SDK v4 projection 为 number。两客户端共享 `/v1/sessions` 且直接返回 JSON，
  本地实测共同得到 string；接受与 Playwright number 声明的类型差异，不增加客户端识别分支。
  已确认的是固定上游客户端声明冲突，尚未实测当前 CF 线上该字段的 JSON 类型。
- `history()` 采用固定 `ClosedSession` 类型，仅返回 closed/lost。Cloudflare 已认证只读 GraphQL
  introspection 的 `AccountBrowserRenderingEventsAdaptiveGroupsDimensions.browserCloseReason`
  描述明确给出 `0 / Unknown`、`1 / NormalClosure`、`2 / BrowserIdle`。lost 使用 `0 / Unknown`，
  不推断 Chromium crash 或 session eviction 的具体原因；整页 history 不再因 lost 返回 501。
  官方语义见[关闭原因](https://developers.cloudflare.com/browser-run/reference/browser-close-reasons/)。
- **acceptance / accepted limitation（用户于 2026-10-08 接受）**：Worker Download 字节接口
  不支持，不阻塞本次交付。`acceptDownloads` 默认/true 使用受控临时目录与 `allowAndName`，false 使用 `deny`。
  下载完成事件与文件字节交付分别验收。固定 Playwright `1.3.6` 的 `path()` 返回 Worker VFS
  `/tmp/playwright-artifacts-*`，其 `saveAs()` / `createReadStream()` 同样读取 Worker VFS；
  Chrome 的下载文件位于宿主受控目录，CDP 没有为这个客户端提供两者之间的字节传输。
  真实 Worker 已确认读文件与 `saveAs()` 报文件不存在，流为空；这些字节接口明确为
  **unsupported**，下载事件成功不计为文件可读。上游仓库的[下载路径问题](https://github.com/cloudflare/playwright/issues/93)
  是针对 `1.0.0` 的用户报告，不证明当前 CF 线上 `1.3.6` 的行为；当前结论来自固定版本的
  本地实测，未把历史报告的 skipped 用例当作证据，也不宣称该限制与 CF 线上一致。
- `/json` 的 `custom_ai` 复用 operator generation alias catalog 与既有 Provider 客户端，
  接受最多三项候选及顺序 fallback；不提供 Cloudflare 托管模型供应或任意供应商原生协议。
- public Browser create、DevTools JSON 与 Live View 统一使用 operator `browser.public_origin`，
  HTTPS 投影 WSS；未配置时使用受信任的 loopback control listener。Live View 路径显式携带 account，
  不依赖 localhost Host 选实例，不接受请求 Host/Forwarded 作为 URL authority。
  真实前端已验证 Console 日志显示及 Network/Elements 面板打开与选中；前端在自己的 CDP
  execution context 使用固定 DevTools 模块，保持其原生 CSP。全部 CDP/options/errors 与
  所有 DevTools 面板仍不作全量验证声明。

managed 保留 Chrome 原生 sandbox，在 CDP 边界限制宿主文件能力并隔离临时下载目录，
不叠加外层 sandbox，不承诺 Chrome 主进程被攻破后的 OS 文件隔离。IP 过滤由 operator 管理。

`open-compute:manual` 是明确的 open-compute AI Search API superset：只有
`open-compute:ai-search` 扩展类型、private binding 的 `openComputeCreateManual` / `openComputeUpsert` 和
operator-configured loopback provider 能访问它。官方 Cloudflare management adapter 不创建、列出或解析 manual
instance，官方 `PUT /items`、upload、sync、source enum、response field 与固定 stable-member denominator 均不改变。
它不使用 deviation ID，因为 deviation ID 只解释已声明官方 member 的可观察差异。

### Cloudflare Artifacts

Artifacts 按 [REST API](https://developers.cloudflare.com/artifacts/api/rest-api/)、
[Git protocol](https://developers.cloudflare.com/artifacts/api/git-protocol/)、
[Workers binding](https://developers.cloudflare.com/artifacts/api/workers-binding/) 与固定
`cf@1.0.0-beta.12` 实现。namespace/repository list 只接受官方 `limit` + opaque `cursor`；旧 Wrangler
专用的 `page` 分支已删除，未知 query field 直接拒绝。固定 cf 的两个 list 命令使用同一 cursor 合同。
token list 保持官方 `page` / `per_page`。repo token 精确采用
`art_v1_<40 lowercase hex>?expires=<unix_seconds>`；Bearer 使用完整值，Git Basic password 使用 `?expires`
之前的 secret，plaintext 只在创建响应出现，SQLite 只保存 keyed digest、scope、expiry 与 revoke metadata。

固定 `@cloudflare/workers-types@5.20260830.1` 是 runtime surface 的类型 authority：其 `ArtifactsRepo` 暴露
metadata、`createToken`、`listTokens`、`revokeToken` 和 `fork`，共 53 个 Artifacts members/overloads。
当前网页文档额外展示的 `log`、`readCommit`、`readTree` 不在该固定类型包中，因此本轮不手写扩展类型，也不把
这些 docs-only Worker methods 宣称为已支持；相同对象读取能力仍通过已声明的 REST routes 提供。待正式 pin
升级且类型、workerd、cf 与 differential evidence 一致时再直接更新唯一实现。

本地 authority、capacity 与 Cloudflare 托管服务的差异见 `OC-ARTIFACTS-001`。名称校验遵循官方规则：首字符
必须为 ASCII 字母或数字，其余只能为字母数字、`.`、`_`、`-`；jurisdiction 因单机无法提供真实 geographic
placement 而 fail closed。import 只允许无 credential/query 的公开 HTTPS remote，禁用 proxy/redirect，并把
一次 DNS 解析得到的公开地址固定到请求，拒绝 loopback、private、link-local、metadata 与 IPv4-mapped private
地址。Git push/import/fork、删除 lease drain、启动恢复、snapshot/restore 和完整性失败均保留 fail-closed
边界；upload-pack 的 `want` 还必须对应当前公告 ref，不能用已知 SHA-1 读取不可达对象。管理面与 Worker
binding 的应用错误使用官方 Artifacts `101xx`/`102xx`/`103xx`/`104xx` 数字码。具体实现与验收见
[P14 Artifacts](../implemented/p14-cloudflare-artifacts.md)。

deviation 规范文本、官方来源和边界见 [`p1-deviations.md`](p1-deviations.md)。其中 raw TCP 的 Day1
实现只有一个 `Network(allow = ["network", "local"], deny = ["unix", "unix-abstract"])` general-outbound authority；workerd 的 `network` 是 `local` 的反集，两者合并覆盖 IP，再显式排除 Unix socket；`fetch()`、
`cloudflare:sockets.connect()`、`node:net`、`node:tls` 共用该 IP 能力，可达宿主允许的 public、private、
loopback、link-local 与 metadata 地址，但不能访问 Unix socket。Service/DO `Fetcher.connect()` 仍只通过
deployment 明确声明的 capability tunnel。runtime-source、binding backend 和 workerd 内部 listener 仅监听
loopback 并独立验证 generation credential；operator 负责宿主 firewall／namespace／容器／VM 策略。

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

Python Workflow ordinary case 使用两个语言各自 self-owned Flow，覆盖 step retry、NonRetryableError/catch、batch、pause/event/restart、committed-step replay、旧实例保留原 Version 和 terminate。definition 的 current Version 独立于 HTTP deployment，rollback 后通过公开 PUT 显式重绑，旧实例继续固定其原始版本。平台使用已有脱敏 Error 兑现 Python FFI 的 rejected promise，仍由 private signal identity 与 controller verdict 决定中断；不读取业务 exception payload/getter。

Cache API 的 default/named 缓存是 Worker 级共享可变状态，跨 Version 保留；rollback 不回退第二版写入。自动 Workers Cache 的默认版本隔离是独立合同。Assets 保留 private wrapped transport，以官方 SDK 识别的公开 `Fetcher` 名称选择其既有 Request/Response 转换。Runtime 的 SDK/raw FFI/JavaScript 对照按声明 subset 验证 Images、Markdown、Vectorize、AI Search 和 Artifacts，不推广为 hosted 模型或全部 overload 资格。

### R2 上传调度与完整性

原生 `R2Bucket.put` 要求流具有已知长度：请求/响应 body 或 `FixedLengthStream` 的 readable 可用，
任意新建且长度未知的 `ReadableStream` 会被原生 workerd 拒绝，不能通过平台 facade 放宽。
这一合同来自正式 pin 的 upstream `d99bc6b777e35d72d71c2f1fe2fd1db53284528a`
[`r2-rpc.c++`](https://github.com/cloudflare/workerd/blob/d99bc6b777e35d72d71c2f1fe2fd1db53284528a/src/workerd/api/r2-rpc.c%2B%2B)
及官方 [FixedLengthStream 文档](https://developers.cloudflare.com/workers/runtime-apis/streams/transformstream/#fixedlengthstream)。
有效日期 `2026-09-08` 没有为此启用额外 flag。真实 R2 Gate 覆盖分块定长上传成功、
未知长度抛出 `TypeError` 且对象没有写入；原有 checksum、metadata、multipart、取消和重启断言继续保留。

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

控制面已注册路由的 4 KiB（v4 为 64 MiB）声明长度检查不作用于 Host-first tenant ingress。
租户 body 始终由 `WorkerdTransport` 按
固定的 `100000000` bytes 流式限额，旧 `workers.max_request_body_bytes` 配置已删除。
这个十进制 100 MB 值来自 [Cloudflare account-plan 请求大小最低 baseline](https://developers.cloudflare.com/workers/platform/limits/#request-and-response-limits)，
不代表复刻商业 plan。声明长度与 chunked overflow 的 413 定向回归已在缩小预算下通过；生产 100 MB 边界、最终
当前正式 pinned-workerd/cf 的该项生产边界 Gate 与 hosted differential 尚未通过，不能把配置值一致称为已完成兼容性验收。
测试代码可通过仅在 `test-support` 暴露的 setter 缩小预算；生产不能通过该路径改变 Standard 值。
现有 30 秒 host response-header deadline 仍是尚未资格化的本地 transport policy，其失败归类为
runtime unavailable，不宣称执行 CPU limit 或产生 `exceededCpu`。原生 limits、isolate 摘除、公开 API 与
supervisor 自恢复已经资格化，见 [workerd W2](../implemented/w2-standard-limits.md)。

### 本机 Worker origin

tenant Worker 使用 `http://<worker>.<instance-id>.localhost:<port>/` 的 exact-host origin，path 从 `/` 开始。它与 Cloudflare
[`workers.dev`](https://developers.cloudflare.com/workers/configuration/routing/workers-dev/)
`<worker>.<account-subdomain>.workers.dev` 的 Worker/account host identity 同形，但 `.localhost`、本机 HTTP、单机 SQLite authority
和只在 loopback listener 可达时发布 endpoint 都是自托管拓扑差异，不宣称提供 Cloudflare 公共 DNS、TLS、preview URL 或全球路由。
Host-first dispatch、canonical authority 拒绝、endpoint OpenAPI/SDK shape 和真实进程调用均有回归覆盖。可选 P18 Gateway 另投影 `https://<public-name>.<instance-id>.<base-domain>/`；这是单机 operator 域名、共享 Caddy DNS-01 和各实例 SQLite authority 的明确拓扑偏差，不声明 Cloudflare `workers.dev`、全球路由或托管证书服务。公网 endpoint 仅在当前受管 Caddy PID 完成 TLS 资格化时发布。

### 固定客户端的 Worker upload wire

Assets bulk upload 的 Axum multipart wire limit 在该路由显式设为 64 MiB，不再使用框架默认的
2 MiB；payload 仍按所有字段的 base64 bytes 累加执行 50 MiB budget，单文件解码后仍不超过
25 MiB。无 `Content-Length` 的 body 也受相同解析器与产品预算约束。固定 base64 multipart
路由回归包含大于 2 MiB 的二进制文件及超预算拒绝；这不是新的 Cloudflare 托管管理面差分证据。

当前固定 cf 将 D1 配置的 `id` 投影为 Worker multipart binding 的 `id`；固定
`cloudflare@7.2.0` 的 typed `workers.scripts.update()` 参数则声明 `database_id`。生成的 open-compute SDK client
只在 binding `type` 精确为 `d1` 时把该字段投影为单一 JSON `metadata` part 内的 canonical `id`；其它 binding
原样保留。服务端在单一解析边界接受当前 cf 的 `id` 与 SDK 的 `database_id`，归一化为同一
D1 authority identifier，同时提供两个字段则拒绝；这两项当前官方 wire 合同不构成旧 CLI 路径。该客户端 wire 差异没有 tenant runtime 可观察语义，因此不
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

固定 cf 的 Worker schema 把 `services[].props` 定义为传给目标 Worker `ctx.props` 的可选 object。
open-compute 在项目导入与 v4 multipart 边界要求 JSON object，执行 64 KiB、32 层深度上限和 canonical key
ordering；canonical bytes/digest 随 immutable Version 一起持久化。runtime admission 会重新验证 canonical bytes
与 descriptor digest，任何损坏都 fail closed；普通 Worker 目标通过 workerd 原生
`stub.getEntrypoint(name, { props })` 交付，`constructor`、`__proto__` 等普通 JSON key 不获得特殊含义。

W3 复用同一 `services + props` wire，把 operator 静态配置的本地扩展解析为隔离 Extension Worker，并只向该 facade
注入私有 `HOST` capability。`ctx.props` 与 RPC 调用语义仍分别遵守 Cloudflare 的
[Context](https://developers.cloudflare.com/workers/runtime-apis/context/)和
[Service Binding RPC](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/rpc/)合同；本机模块加载、
Provider 进程与 Host ABI 是明确的 open-compute superset，不声称 Cloudflare 提供相同行为，也不进入 stable runtime-member
denominator。这项本地实现不宣称 Cloudflare 的跨区域 placement，也不扩大 `remote` 支持范围。

operator 还可把具名 Service target 配置为固定私网 HTTP endpoint。租户仍只声明标准 `services` binding，并只观察
`fetch(Request) -> Response`；URL、DNS pin、方法/路径 allowlist、credential 与精确 caller grant 全由 operator authority
持有。host proxy 不跟随 redirect，移除租户与平台认证 header，并在每次调用重验 policy revision。该 target 不支持 RPC 或
`connect()`。这是 open-compute 的受限 operator superset，不是 Cloudflare runtime
member 或 hosted VPC claim。

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

Queue consumer 响应只投影官方 `script_name`，不再输出旧 CLI 专用的 `script` 别名。
当前固定 cf 的消费者命令与 SDK 使用同一 [REST response](https://developers.cloudflare.com/api/resources/queues/subresources/consumers/methods/list/)；
所属 CRUD 回归同时检查 create/list/get/update 的名称字段和旧别名缺失。

固定 cf 1.0.0-beta.12 所依赖的 `@cloudflare/config` Queue schema 仍声明 `deliveryDelay`；cf 的
upload builder 将 producer 的 `delivery_delay` 原样放入 queue multipart binding，内部 config validator
又明确警告该字段已弃用且无效果。它属于当前固定 cf 的可观察 upload 合同，不能仅因实现源自 workers-sdk
就当成旧 CLI shim 删除。两种官方 metadata wire 统一解析该数值并忽略它，不改写 Queue authority 或
immutable descriptor；`/queues/{queue_id}` settings API 才拥有 Queue-level 默认 delay。既有
`fixed_wrangler_deprecated_queue_delay_metadata_is_accepted` 和
`deprecated_queue_binding_delay_does_not_change_queue_authority` 两项所属回归保留；名称记录来源，行为按当前 cf
重新核对。证据是固定 cf 的 `chunk-KKDV4JPS-D3kwd1Nq.mjs`、`dist-CYFkGHYv.mjs` 与 config schema。
官方 producer 文档与 CLI warning 的差异仍未通过同版本 hosted trace 关闭，不宣称托管一致性。

### Dynamic Workers 生命周期

普通 Worker 的 public Loader namespace 由 InstanceId / Script / binding 的不可变身份派生，跨 Version
回滚保持一致；删除重建同名 Script 使用新身份。原生 cache 有界且可撤销，命中不是公共保证。
平台对已执行 Version 保留保守的 background-work hold，直到监督器证明 workerd generation 已退出；
普通 Script DELETE 在 hold 或真实在途执行存在时返回 409。`force=true` 持久化删除 intent 并 fence 新 admission，
必要时受控轮换所属实例的 workerd generation，再原子 tombstone Worker authority 与释放全部历史 Version referrer；
进程在轮换与提交之间退出时，下一次启动在 runtime admission 前幂等完成删除。轮换会短暂影响同机其他 Worker，
但外部 D1/KV/R2/Queue 资源不会随 Worker 删除。

`WorkerCode.env` 的 transfer boundary 接受 structured-clone value、Service Binding 和上游可序列化的
D1 `WrappedBinding`。D1 使用上游 `cloudflare-internal:d1-api` wrapper，转移的是原有 Fetcher capability，
子 Worker 仍受原有资源授权与 tenant scope 约束，只得到显式传入的 env；KV、R2、Queue 的直接转移继续抛
`DataCloneError`，需要 `ctx.exports` wrapper 或下述显式平台扩展。

此行为依据正式 workerd pin 的上游基线 `d99bc6b777e35d72d71c2f1fe2fd1db53284528a` 中
[D1Database](https://github.com/cloudflare/workerd/blob/d99bc6b777e35d72d71c2f1fe2fd1db53284528a/src/cloudflare/internal/d1-api.ts) 与
[WrappedBinding serialization](https://github.com/cloudflare/workerd/blob/d99bc6b777e35d72d71c2f1fe2fd1db53284528a/src/workerd/api/wrapped-binding.c%2B%2B)，
并由 `p6-cf-resources` 验证 parent 写入、child 绑定查询和精确 env keys。测试 compatibility date 为
`2026-09-08`，未启用实验性 `d1_binding_jsrpc`；D1 继续通过原生 Fetcher 的 HTTP 契约访问平台 authority。
这项证据限定于当前正式 pin，不声明 hosted Cloudflare 差分已通过。

`open-compute:worker-loader` 的 `getWorker/loadWorker` 是显式选择的本地扩展，允许同一 owner 下已登记的
typed binding root 通过 host-owned materializer 重建到 child；它不改变上述原生 `LOADER.get/load` 的
transfer 规则，也不声明为 Cloudflare 标准能力。该扩展不继承 private Loader、平台 env 或普通 Python
prepared artifact；Dynamic Python 的启动限制仍单独受 `OC-WKR-LIMIT-001` 约束。

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

管理面实现官方 `POST /queues/{queue_id}/messages` 与 `/messages/batch`，并由生成 SDK 暴露
`queues.messages.push` / `bulkPush`。它们复用同一 durable enqueue、大小/批次/content-type 校验和本机 metrics；enqueue
提交后的 30 秒超时按 result-unknown 返回。HTTP pull/ack/peek/purge 因缺少已资格的 lease/retry/crash-recovery 合同而继续
不在支持面。

Queue `v8` 的当前生产模型保留经过正式 workerd version-15 codec 验证的原生 bytes，consumer
使用同一原生 decoder；不再转换成平台自有 structured-clone 格式。128,000/256,000 bytes 预算
按[官方十进制 KB](https://developers.cloudflare.com/queues/platform/limits/)执行，托管 internal
metadata 额度仍不声明 parity。缺省格式由原生 date/flags 决定，`queues_json_messages` 在
2024-03-18 生效，`no_queues_json_messages` 可关闭，来源为固定 upstream compatibility-date
schema。当前 r4 private publication component 验证两种选择、大 Unicode、原始 byte 大小和
超限前无写入；两语言 Queue case 已通过真实 SQLite、fresh restart 与 Version rollback 验证。config-only
raw fetch 的 oversized batch cancellation 限制不构成该产品路径资格证据，详见
[Python Workers 测试合同](testing.md#python-workers)。全库兼容性状态与历史 inventory 不由组件结果提升。

### 资源生命周期

Cron activation generation 从该 Worker 的全部持久 activation（含 tombstone）取最大值后递增。
移除全部 triggers 不会重置代次；重新启用相同表达式或回滚旧 Version 会创建新 activation，
相同 Version 的当前 staging/active 重试则保持同一身份。该规则保留单机 restart/reconcile 与
stale-generation fencing；不模拟 [Cloudflare Cron 的全球传播延迟](https://developers.cloudflare.com/workers/configuration/cron-triggers/)。
回归覆盖清空后重新打开 control/scheduler SQLite、重新启用、幂等重试，以及 P0.2 真实 scheduled dispatch。

Worker tombstone 在同一事务中释放 generic、Queue producer 和 Workflow binding referrer；immutable
deployment declaration 仍保留为历史 authority。Queue/Workflow/R2/D1/KV/DO 删除按当前 Day1 tombstone
模型确认无 live resource 后才允许同名重建，不保留旧 schema 或兼容清理分支。

Worker Version upload 取得的 Workflow reservation 在共用 validation pipeline 中先 stage，逐 class 通过真实 runtime probe 后全部
publish，再把 Worker Version 标记 ready。确定性 probe 失败会拒绝已 stage Workflow versions 与 Worker Version；transient failure
保留 validating 状态供既有恢复路径重试，避免 ready Worker 引用 stale Workflow definition。官方 Beta Worker Version DELETE 仅
tombstone 非 active、无 pin/持久 referrer 的历史 Version，并释放 binding referrer；外部产品数据不级联删除。
Workflow definition 的 current Version 与 Worker HTTP deployment 分别管理。HTTP promotion/rollback 不自动重绑
definition；需要通过公开 `PUT /workflows/{name}` 选择当前 active Worker Version。已创建实例继续固定创建时的
Workflow/Worker Version。Python 用例验证这一显式管理流程，不据此宣称托管平台自动 rollback 管理语义一致。
官方 Beta Worker GET 按当前 account 的名称或 public Worker tag 读取；本地响应投影 immutable Worker identity、时间戳与空 references，
并明确返回 `subdomain.enabled=false`、`previews_enabled=false`，不伪造 workers.dev DNS 或 Preview 可达性。
upload 与 deployment 显式提供 `code_update_strategy` 时，本地 closed decoder 验证官方 mode、范围与毫秒精度，但单机 runtime 仍原子切换 generation，不模拟 Cloudflare 托管 Durable Object 的 hibernation rollout；
该语义差异归入 `OC-DEPLOY-001`。

AI Search upload 按官方 `namespace` 字段解析 public instance key；省略时使用 `default`。authority lookup 同时固定 namespace 与
instance，跨 namespace 同名 instance 不再依赖内部 Resource name，也不会互相解析。

### 官方 account prerequisite 的历史证据

历史固定 Wrangler 4.143.0 在 `workers_dev:false` 的 Workflow deploy 中，于 Worker upload 后、Workflow
PUT 前读取 `GET /accounts/{account_id}/workers/subdomain`，并丢弃返回值。open-compute 将该只读 route 标为
`supported_with_deviation`：它返回以 `_` 开头、按 account 稳定派生的非 DNS label，只满足固定 CLI 的顺序
prerequisite，不创建 workers.dev DNS、listener、route 或注册 authority；对应 `PUT/DELETE` 继续不支持。
真实本地入口仍以 vendor Worker endpoints route 为准。该 route 与 capability 的关联 deviation 为
`OC-ACCOUNT-SUBDOMAIN-001`。

历史固定 Wrangler 4.143.0 创建 AI Search instance 前读取
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
已经与 Cloudflare 托管管理面实测一致，因此这些托管端一致性不在当前声明范围。

此外，产品专项验收已记录 Vectorize、AI Search 的真实 Cloudflare 高风险 differential，以及 Workers
Observability 的 authenticated Dashboard network differential。README 因此按“存在真实 Cloudflare 直接对照证据”的
产品 surface 口径列为十项；这不把专项 probe 外推成完整 hosted management qualification，也不改变上述 portable runner
仍为七项的事实。

Workflow portable fixture 已实现并通过 open-compute 本地真实进程路径，但此前对照使用的 OAuth session 对
Cloudflare Workflow inventory API 返回 `Authentication error [code: 10000]`，在 preflight 阶段即停止，
没有创建 Workflow 或 Worker。源码冻结后的七项合并复查又在 D1 inventory preflight 收到同一错误；该次
运行已先完成 Cache API 对照并精确清理，D1 及后续 fixture 未创建资源。此前已完成的 D1 和其它分项
qualification 仍是有效证据，但当前 token 不能生成新的合并报告。这个外部限制不使本地实现重新变为
`blocked`；账号权限条件解除前，不得声称 Workflow 已完成真实 Cloudflare differential qualification，
也不得把其它七项结果外推为“所有产品均与 Cloudflare 托管端实测一致”。

本地证据由 `p3-contract` 的 type/catalog/config/deviation/source 双射、产品 Gates、真实 pinned
workerd、SQLite 与选定的 Local/S3 object authority、restart/crash tests 和最终 workspace/coverage 共同组成。最终命令、报告和
实际限制记录在归档完成报告中；机器可读 capability/catalog 仍是支持状态的唯一 authority。

[P20](../implemented/p20-cf-cli-migration.md) 当前应用入口为项目内 cf 与官方 Vite 插件 v2，配置为 `cloudflare.config.ts`，构建产物为 Cloudflare Build Output。内部 CI 固定 cf 1.0.0-beta.12；用户的非 1.0.x 版本警告后仍执行。旧 Wrangler 项目需显式使用 `cf migrate <exact-file> --bundler vite`。终端流式 tail 尚无等价 cf 入口，使用 Dashboard Live Tail。

P21 临时例外只限开发侧 Python 构建：调用用户安装的 PyWrangler，不校验版本、不自动安装；
Worker 配置仍为 `cloudflare.config.ts`，上游构建的 Build Output 只由 cf `--prebuilt` 上传/部署。
认证与资源管理保持 cf。官方 cf Python builder 可用且资格通过后删除桥接，不保留双构建路径；
fresh sync/build 与 cf loopback prebuilt capture 已通过并保留真实静态输入；七类 ordinary 场景已在完整 workspace coverage 中执行。Flask context-bearing stream 仍受 SDK 1.9.2 gap 影响并按已知问题暂缓。
移除 TODO 与具体资格边界见 [P25 构建桥接移除待办](../p25-platform-follow-ups.md)。
支持范围以本页的普通部署合同和已接受限制为准。

2026-10-04 的 P21 扩展源码审查核对了 shared materializer、核心 binding policy 与 prepared artifact 的
authority/加密/恢复边界，未发现已审路径的新确定性 mismatch；逐表面证据与未验收项目保留于
`.temp/p21-cf-full-review/iteration-01/review-01.md`。这不代表全部 diff 或完整 Python contract 已通过。
各 ordinary case 的完整生命周期和 workspace coverage 已验证；最终未插桩验收记录与 coverage 报告分别保留。当前补充复核包含 Fetcher 构造、Workflow FFI rejection、Cache API scope 与工具边界，逐项证据保留在 `.temp/p21-final-preflight/`。Flask 流式上下文限制见下述已知问题。

### Python 已知上游问题：Flask 流式上下文

2026-10-04 用户明确暂缓处理 [cloudflare/workers-py#287](https://github.com/cloudflare/workers-py/issues/287)。
SDK 1.9.2 的 WSGI adapter 在首次读取与异步 `pull`/`cancel` 之间未保持同一个 Python Context，
Flask `stream_with_context` 因此出现 request/app ContextVar 错误，可能截断响应或使清理失败。
官方 stock workerd `v1.20260929.1` 配合未修改 SDK、直接 service binding 已复现；这不是托管端验证。
Flask 带 request/app context 的流式响应暂不在本轮支持与验收范围，不能从普通 Flask HTTP/template
Gate 推断 streaming 已通过。原应用路由、失败日志与诊断补丁保留，正式 SDK 不改动。
其余 Flask HTTP、模板、不可变部署、restart/rollback、密文损坏拒绝和日志安全已在完整三框架聚合中通过。
TODO(P25)：上游发布修复后重新验证该路由的完整 body、并发上下文与 cancellation cleanup，恢复流式验收并移除该限制。
证据见 `.temp/p20-python-flask/diagnostic-01.json` 和 `.temp/p21-flask-context-diagnostic/qualification-01.json`。

P20 支持 cf 当前 Beta Version GET/list 的扁平响应、分页与 `include=modules`；模块来自校验后的不可变 artifact，secret 只返回 binding 名称。版本与 Service 元数据以 `open-compute` 标识平台 producer，不冒充某个客户端品牌或 Cloudflare Dashboard。cf/Vite 动态导入分片的 `./` 模块前缀在上传入口规范化；路径穿越与规范化后的重复名称仍拒绝。Workflow export 与同 Script binding 引用复用现有 definition reservation 和 WorkerId，不开放跨 Script 引用。

P20 的应用 Env 由官方 cf 生成（其 runtime types 输入为 workerd 1.20261001.1）；这不替换平台固定 stable workers-types 或扩大 runtime inventory。新声明全集尚未资格化；实际绑定仍受服务端 admission 和既有 capability 子集约束。

cf 1.0.0-beta.12 的已有 Worker redeploy 先 POST Version（带 observability），再创建 Deployment 并 PATCH script-settings。该非版本化字段在上传入口完整验证；单独 Version POST 不更改现有 Script 日志策略，Script PUT 或显式 script-settings PATCH 才更新。正式 OpenAPI 的 Version POST metadata 尚未列出该字段；这是当前官方 cf producer 的已记录差异，不引入客户端版本选择或另一路持久化实现。

AI Search 实例创建使用 `cf ai-search create <namespace> <id> --body '{"id":"<id>","embedding_model":"<operator-alias>","chunk_size":64}'`。当前 cf 的逐项 flags 自动加入已明确不支持的 hosted cache / hybrid 字段，并将 `@cf/…` 形式的模型别名按文件参数读取；显式 JSON body 保留已声明配置，服务端仍拒绝 unsupported 字段，不静默剥除或忽略。其余 instance 查询、更新、搜索及 jobs 操作按官方命令执行。
