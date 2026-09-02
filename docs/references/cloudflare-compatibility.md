# Cloudflare Workers 兼容矩阵

本页是 [`share/cloudflare-capabilities.json`](../../share/cloudflare-capabilities.json) 与
[`test/conformance/catalog.json`](../../test/conformance/catalog.json) 的人类可读索引，不建立第二份
能力真值。`ocd capabilities --json`、类型 inventory、contract catalog 和 Gate 共同定义当前
支持面。完成设计和 conformance 方案见
[Cloudflare Runtime 全量兼容改造](../implemented/cloudflare-runtime-compatibility.md)与
[P3.4 Cloudflare conformance](../implemented/p3-4-cloudflare-conformance.md)；尚待外部账号条件解除的
验收只记录在[剩余验收计划](../cloudflare-runtime-compatibility-acceptance.md)。

固定契约输入见 [`baseline.json`](../../test/conformance/baseline.json)。当前 formal pin 是
`workerd v1.20260830.1`，revision `e9dda5963aba7ee4323960db795690ec78fec118`，唯一
`effectiveCompatibilityDate` 为 `2026-08-30`；stable types 是
`@cloudflare/workers-types@5.20260830.1`。tenant 不得选择 compatibility date 或 flags，也不保留旧
open-compute schema、descriptor、runtime 或 API 的兼容路径。

## 当前结论

目标 inventory 共 2,178 个 stable members/overloads：1,585 个 `supported`，593 个
`supported_with_deviation`，`blocked=0`。catalog 的 2,178 条 `memberEvidence` 与 capability 成员双射，
`blockedGaps=[]`。deviation 只描述单机 self-host 无法复制的 edge/全球拓扑、托管 fleet quota 或本地
authority 差异；它不代表缺方法、占位返回或半截实现。

| 产品 | 状态 | 成员 | 当前实现与证据 | deviation |
| --- | --- | ---: | --- | --- |
| Workers runtime | `supported_with_deviation` | 1,580 | 1,556 个成员直接支持；24 个 raw-TCP 成员保留完整 API，仅隔离 hosted TCP policy/fleet limit 差异。latest 默认 Node.js、Web APIs、handlers、RPC、Cache、raw TCP 和配套 surface 均有 compile/stock-workerd/runtime case | `OC-WKR-TCP-001`、`OC-WKR-LIMIT-001` |
| KV | `supported_with_deviation` | 52 | 单键/批量 overload、metadata、stream、list、`cacheStatus`、错误时序和恢复均闭环 | `OC-KV-001` |
| R2 | `supported_with_deviation` | 110 | object/body/list/options、全部 checksum、SSE-C、storage class、条件写、multipart、opaque provider key、持久 intent/reconcile 和 restart 均闭环 | `OC-R2-001` |
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

Deployments、Static Assets、Service Binding、Workers Cache 与 Images 是平台配套能力，没有进入上述
stable-member denominator；AI 的 54 个目标 members/overloads 已进入 denominator，并按当前本地合同登记为
`supported_with_deviation`。
Analytics Engine、Browser Rendering、Hyperdrive、mTLS、Rate Limiting 与 Workers for
Platforms 明确为本轮非目标并在部署 authority 边界拒绝。完整 Workers AI inference 仍是非目标；存在标准
`env.AI` 只表示上表的 Markdown Conversion 与 AI Search 所需配置模型子集，不能因 upstream types 中存在其它 AI 名称而扩张能力声明。

deviation 规范文本、官方来源和边界见 [`p1-deviations.md`](p1-deviations.md)。其中 raw TCP 的 Day1
实现只有一个 `Network(allow = ["public"])` general-outbound authority；
`cloudflare:sockets.connect()`、`node:net`、`node:tls` 共用该地址层。Service/DO `Fetcher.connect()` 只能
通过 deployment 明确声明的 capability tunnel，不能成为第二条通用出网路径。runtime-source、binding
backend 和 workerd 内部 listener 仍仅监听 loopback。

## 关键实现说明

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

Worker tombstone 在同一事务中释放 generic、Queue producer 和 Workflow binding referrer；immutable
deployment declaration 仍保留为历史 authority。Queue/Workflow/R2/D1/KV/DO 删除按当前 Day1 tombstone
模型确认无 live resource 后才允许同名重建，不保留旧 schema 或兼容清理分支。

## Differential 与本地证据

2026-09-01 的同源 portable fixtures 已在真实 Cloudflare 与 open-compute 对照以下七项：Workers、Cache
API、KV、D1、R2、Durable Objects 和 Queues。公开 status/JSON 经合同允许的归一化后逐字段一致；每次只
创建唯一 `oc-p34-*` Worker 及 fixture 自有 binding，按精确 name/ID 删除并复查 absent，没有修改账号中
已有服务。DO fixture 包含递归 nested facet clone/delete；Queue fixture 包含 metrics、五类 producer 错误
和消费响应。

Workflow portable fixture 已实现并通过 open-compute 本地真实进程路径，但当前 Wrangler OAuth 对
Cloudflare Workflow inventory API 返回 `Authentication error [code: 10000]`，在 preflight 阶段即停止，
没有创建 Workflow 或 Worker。源码冻结后的七项合并复查又在 D1 inventory preflight 收到同一错误；该次
运行已先完成 Cache API 对照并精确清理，D1 及后续 fixture 未创建资源。此前已完成的 D1 和其它分项
qualification 仍是有效证据，但当前 token 不能生成新的合并报告。这个外部限制不使本地实现重新变为
`blocked`；账号权限条件解除前，不得声称 Workflow 已完成真实 Cloudflare differential qualification，
也不得把其它七项结果外推为“所有产品均与 Cloudflare 托管端实测一致”。

本地证据由 `p3-contract` 的 type/catalog/config/deviation/source 双射、产品 Gates、真实 pinned
workerd、SQLite/S3 provider、restart/crash tests 和最终 workspace/coverage 共同组成。最终命令、报告和
实际限制记录在归档完成报告中；机器可读 capability/catalog 仍是支持状态的唯一 authority。
