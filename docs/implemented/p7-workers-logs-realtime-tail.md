# P7：Workers Logs 与 realtime tail 兼容设计

状态：Implementation GO；固定客户端、本地 Day 1 核心与仓库级验收完成

日期：2026-09-04

本文是 [`P6 Cloudflare v4 API 与 Wrangler 子集兼容设计`](p6-cloudflare-v4-wrangler-compatibility.md)
的 observability 专项设计，细化以下能力：

- 固定版 Wrangler 的 `wrangler tail`；
- Cloudflare Workers Logs 的采集、持久化和查询；
- Cloudflare Workers Observability Telemetry 的 `keys`、`values`、`query`、`live-tail` 与 heartbeat 子集；
- Dashboard Logs 页和自动化客户端共用的 API；
- 私有部署下的容量、故障与权限边界；部署方容量与 LynxOS 产品默认值不属于 Cloudflare logs 合同。

本文不改变主设计的一套 `/client/v4` 管理协议、一个 `ocd`、一个 stock workerd child、SQLite metadata
authority 和 S3 artifact authority。日志不是 Worker artifact，也不是 control metadata；其高写入量状态进入独立、
有界的 `observability.sqlite`，不能拖慢 `control.sqlite` 或改变 tenant invocation 的结果。

## 1. 结论

Day 1 采用“一个采集内核、三种官方入口”的结构：

| 能力 | 官方入口 | Day 1 状态 |
| --- | --- | --- |
| Wrangler realtime tail | Script Tails API + `trace-v1` WebSocket | 必须实现；`wrangler@4.127.1 tail` 是硬 Gate |
| Workers Logs | Telemetry `keys`/`values`/`query` | 实现 `cloudflare-workers` dataset 的 `events`、`invocations` 子集 |
| Dashboard realtime tail | Telemetry `live-tail` + heartbeat + WebSocket | 必须在真实 Dashboard/SDK wire trace 冻结后实现 |
| Tail Workers | `tail_consumers`、`tail()` handler | 本阶段不支持；不能与平台内建 collector 混为一谈 |
| Streaming Tail Workers | `streaming_tail_consumers`、`tailStream()` | 本阶段不支持 |
| Traces | `observability.traces`、trace query | 只接受明确 disabled；启用时失败 |
| 外部导出 | `observability.*.destinations`、OTel、Logpush | 只接受空数组或 disabled；非空时失败 |
| Saved/shared queries | Observability Queries API | 本阶段不支持 |

三种已支持入口共用同一个 canonical invocation/event model、filter evaluator、redaction、quota 和 metrics，
但 wire contract 彼此独立：

- `wrangler tail` 使用 Scripts 下的 tails 资源和 `trace-v1` frame；
- Telemetry query 使用 Observability Telemetry 的 request/response schema；
- Telemetry Live Tail 的 WebSocket payload 以固定 Cloudflare trace 为准，不能直接假设等同于 `trace-v1`。

`wrangler tail` 与 Workers Logs 相互独立。即使 `observability.enabled=false` 或 `logs.persist=false`，授权用户仍可
启动 realtime tail；反之，没有实时客户端时，已启用的 Workers Logs 仍会按采样策略持久化。

本次实现直接替换了此前尚未落地的 observability 路径，没有保留旧 schema、旧协议、双写、fallback 或兼容分支。
`control.sqlite` 只保存 Script setting/generation 与审计，日志唯一写入独立的 `observability.sqlite`；tail session
始终是 process-local ephemeral state。固定 Wrangler、官方 SDK 和 Dashboard Live Tail 共用同一真实 `ocd + stock
workerd` Gate，重启后已提交日志继续可查、实时 session 为空。

## 2. 合同 authority 与证据等级

实施使用以下固定输入，优先级从高到低：

1. 冻结 revision 和 SHA-256 的 Cloudflare OpenAPI schema；
2. 仓库直接 pin 的 `wrangler@4.127.1` package、`config-schema.json` 和真实 HTTP/WebSocket trace；
3. `cloudflare@7.1.0` 官方 SDK 的生成类型与真实 wire trace；
4. `@cloudflare/workers-types@5.20260830.1` 的 `TraceItem`、`WorkerLoaderWorkerCode` 与 handler 类型；
5. Cloudflare Workers Logs、Real-time Logs 和 Tail Handler 官方文档。

网页只用于发现当前合同，不能成为可复现 build input。M0 必须保存 schema/package hash、无敏感信息的 trace fixture
和字段 inventory。若 OpenAPI、SDK、Wrangler 或托管端行为冲突，以固定客户端能否工作和差异登记为准，不能靠宽松
JSON parser 同时接受多个猜测版本。

本文中的事实分为三类：

- **已验证**：当前 pin 的类型或 Wrangler 源码已经证明；
- **待 differential**：需要 Cloudflare credential 或 Dashboard network trace 才能冻结；
- **本地决策**：单机容量、数据库、队列和恢复策略，不伪装成 Cloudflare hosted limit。

当前已验证：

- `WorkerLoaderWorkerCode` 原生提供 `tails?: Fetcher[]` 与 `streamingTails?: Fetcher[]`；
- `TraceItem` 包含 invocation event、logs、exceptions、script version、outcome、CPU/wall time 和 truncated；
- 固定 Wrangler 通过 `POST .../tails` 创建会话，使用返回的 URL 连接 `trace-v1` WebSocket，连接后发送
  `{"debug":false}`，退出时 `DELETE .../tails/{id}`；
- 固定 Wrangler 每 10 秒发送 WebSocket ping，连接异常时以 1、2、4、8、16 秒退避创建新 tail；
- 固定 Wrangler 的 filters 是 `sampling_rate`、`outcome`、`method`、`header`、`client_ip`、`query` 和
  `scriptVersion`；
- `observability` 是非 Version setting；Worker code、bindings、compatibility metadata 仍属于 Version。
- 2026-09-03 的真实 Cloudflare Dashboard capture 证明 Telemetry Live Tail prepare body 使用 `scriptId`、filters 与
  `filterCombination`，WebSocket 不使用 subprotocol/首帧，事件 frame 顶层为
  `source,dataset,timestamp,$workers,$metadata`，Dashboard 每约 15 秒调用 heartbeat；脱敏 fixture 已固定在
  [`test/fixtures/p7/observability-wire-v1.json`](../../test/fixtures/p7/observability-wire-v1.json)。
- 当前 pin 的 stock workerd hard spike 证明：只给 lifecycle root 配置 tail 时，nested Service target 的日志不会
  进入 root callback。生产实现因此为每个实际执行的 dynamic target 挂 collector，并使用该 target 的可信 snapshot
  identity 记账；当前 realtime session 只接收自身 Script 的事件，不把 nested target 日志聚合进 caller/root session。

仍须 differential 冻结：

- Script Tail 的精确 TTL、GET/list 字段、每种错误 code 与 delete-not-found 行为；
- `trace-v1` 对所有 event type、truncation、overload 和 hidden `debug=true` 的完整 frame；
- Telemetry query 的 required/default/null/empty 字段以及 Workers Logs 对 `console.*` 参数的 `source` 映射。

上述剩余托管端长尾不阻塞本文声明的固定客户端子集；它们由
[`P7 observability 扩展差分与发行验收`](../acceptance/p7-observability-extended-acceptance.md)继续跟踪，不能据此扩大 capability。

上述待冻结项不允许用“近似兼容”补齐；未由当前 fixture 和固定客户端证明的字段、event type 或 error variant 保持
`unsupported`，不扩大已声明的固定客户端子集。

## 3. 用户可见配置

### 3.1 `wrangler.jsonc`

Day 1 支持固定 Wrangler schema 中以下 logs 配置：

```jsonc
{
  "observability": {
    "enabled": true,
    "head_sampling_rate": 1,
    "logs": {
      "enabled": true,
      "head_sampling_rate": 1,
      "invocation_logs": true,
      "persist": true,
      "destinations": []
    },
    "traces": {
      "enabled": false,
      "persist": false,
      "destinations": []
    }
  }
}
```

字段处理规则：

| 字段 | Day 1 行为 |
| --- | --- |
| `observability.enabled` | 控制持久化 observability 总开关；不关闭 realtime tail |
| `observability.head_sampling_rate` | logs 未单独设置时的 sampling rate，范围 `0..=1` |
| `observability.logs.enabled` | 控制 logs collection/persistence；不关闭 realtime tail |
| `observability.logs.head_sampling_rate` | 优先于顶层 rate，范围 `0..=1` |
| `observability.logs.invocation_logs` | `false` 时不写 invocation summary，但已采样的 custom logs/exceptions 仍可写入 |
| `observability.logs.persist` | `false` 时不写本地 Workers Logs；不关闭 realtime tail |
| `observability.logs.destinations` | 只接受 omitted 或 `[]`；非空数组明确 unsupported |
| `observability.traces.enabled` | 只接受 `false` 或 omitted；`true` 明确 unsupported |
| `observability.traces.persist` | 只接受 `false` 或 omitted |
| `observability.traces.destinations` | 只接受 omitted 或 `[]` |

effective sampling rate 为：

```text
logs.head_sampling_rate
  ?? observability.head_sampling_rate
  ?? 1
```

一次 head-sampling decision 作用于整个 invocation；被选中的 invocation 内日志不能再逐行随机采样。采样使用
`invocation_id` 与部署时冻结的 policy generation 做稳定 hash，重试 query 不改变结果。rate `0` 不持久化，rate
`1` 全量持久化。

observability setting 属于 Script，而不是 immutable Version。`GET/PATCH script-settings` 和 Wrangler deploy
写入 Script setting authority；每次 runtime-source resolution 把 setting generation 与当前 Version/Deployment 一起
冻结到 runtime snapshot。已有 warm Worker 的 setting 改变必须产生新的 runtime key/generation，不能继续使用旧 policy。

全新 Script 在请求完全未提供 observability 时，按当前 Workers Logs 官方语义默认启用 logs persistence、
invocation logs 和 rate `1`；M0 用固定 OpenAPI/hosted trace 冻结 omitted/null 的精确 wire 行为。若固定 Wrangler
明确发送 `{enabled:false}`，必须关闭持久化，不能被 server-side 默认重新打开。

### 3.2 平台配置

`PlatformConfig` 增加严格的 `[observability]` 段，所有字段 `deny_unknown_fields`、有 compiled hard ceiling，并在
`share/default-config.toml` 完整展开。Cloudflare 合同常量与部署方 capacity 必须分开；本文不按 LynxOS 用户数给
open-compute 指定数据库、queue 或 backlog 默认值：

| 配置 | 合同值/配置规则 | 性质 |
| --- | --- | --- |
| `retention_ms` | 最大 `604800000`（7 天） | Workers Logs 合同；更短值是 installation capability |
| `max_database_bytes` | 部署方显式 capacity；无 Cloudflare 对应值 | 不进入 official limits，不在本文给人数相关默认值 |
| `max_invocation_log_bytes` | `262144`（256 KiB） | 官方每 request 全部 log data 上限，超出后停止记录更多 context |
| `ingest_queue_events` | 部署方显式 capacity，必须大于零且有 compiled ceiling | 进程内 bounded queue，不是 CF limit |
| `ingest_batch_events` | 部署方显式 capacity，必须大于零且不超过 queue | 单 transaction batch，不是 CF limit |
| `ingest_flush_ms` | 部署方显式 latency/capacity 参数 | 不是 CF limit |
| `max_tail_sessions_per_script` | `10` | 与官方 realtime client limit 对齐 |
| `tail_client_queue_bytes` | 部署方显式 capacity；必须有 compiled ceiling | 单 WebSocket client bounded backlog，不是 CF limit |
| `query_max_events` | `2000` | 与 Telemetry query event limit 对齐 |
| `query_max_timeframe_ms` | `604800000` | 不允许扫描 retention 之外时间 |

Script Tail TTL 不猜测 hosted 常量：固定 Wrangler trace 冻结本地默认值，并写入 capability。TTL 可以作为安装配置
收紧，但 response 的 `expires_at`、ticket expiry 和清理任务必须使用同一个 authority。Dashboard Live Tail 按已冻结的
15 秒 heartbeat wire 使用本地 45 秒 eligibility ceiling；prepare 后未连接或停止 heartbeat 的 session 会在该窗口内回收，
这是 `OC-OBSERVABILITY-001` 的 process-local 行为，不冒充尚未取得的 hosted expiry 常量。

## 4. 运行时架构

```text
tenant invocation in stock workerd
  HTTP / Queue / Cron / Alarm / DO / Service / Workflow / RPC
                         |
                         | WorkerLoaderWorkerCode.tails
                         v
       platform ObservabilityTail WorkerEntrypoint
       props = account/script/version/deployment/policy generation
                         |
                         | redacted canonical ingest envelope
                         v
       generation-authenticated observability-backend
                         |
             +-----------+-------------+
             |                         |
             v                         v
      bounded live fan-out       bounded ingest queue
             |                         |
     Script Tail / Live Tail     observability.sqlite
                                       |
                                       v
                         Telemetry keys/values/query
```

不得采用以下替代方案：

- 抓取 workerd stdout/stderr：它们是 child process diagnostics，不是 tenant `console.*` contract；
- 在 tenant source 中注入 `console` monkey patch：无法完整覆盖 exception、CPU outcome、DO/Queue 等事件，且会改变用户代码；
- 修改或 fork workerd：当前 pin 已提供原生 tail hook；
- 把日志写入 S3 artifact bucket：日志不是 immutable code artifact，查询、retention 和 quota 语义也不同；
- 为每个 Worker 启动独立 collector/container：平台保持一个 `ocd` 和一个 stock workerd child 的部署约束。

### 4.1 平台 Tail Consumer

在 system Worker 中增加 `ObservabilityTail extends WorkerEntrypoint`，实现 `tail(events)`。每个 execution root
使用的动态 Worker 在 `WorkerLoaderWorkerCode` 中设置：

```text
tails: [ctx.exports.ObservabilityTail({ props: frozenIdentityAndPolicy })]
```

当前 pin 的 stock workerd 实测并不会把 nested target 日志自动送进 root callback。生产实现因此让每个实际执行
target 都有一个 collector attachment：

- HTTP、Queue、Cron/scheduled、alarm、Service、DO、Workflow 和 RPC target 的 collector 都携带自身可信 snapshot
  identity；batch item 的 `scriptName` 只能是该 collector 自身的 external Script name 或同一
  account/Worker/Version loader identity，不能借机替另一 target 归属日志；
- observability setting generation 进入 immutable runtime key，warm Worker cache 不会混用不同 target policy；
- backend 按 collector event ID 与 item 序号生成幂等 invocation identity，只持久化 target event 一次，并只投影给
  target Script 的实时 session。

因此 caller/root 的 tail 不聚合 nested target 日志；授权用户需要对 target Script 建立独立 tail。Hosted
Service/DO/Workflow/Queue attribution 尚未完成，归入 `OC-OBSERVABILITY-001` 和扩展验收，不虚构 Cloudflare 的聚合语义。

必须覆盖的 root 与 nested 组合包括：

- loader-host 的 HTTP/default/named entrypoint；
- Queue 与 scheduled custom event；
- Durable Object direct/root、nested call 与 alarm；
- Service Binding direct/root 与 lazy nested target；
- Workflow root/runner 与 nested call；
- alarm、RPC、hibernatable WebSocket 等当前 stock workerd 能生成的 TraceItem。

validation/probe Worker 不挂 collector，避免把 compile/load probe 写成 tenant log。平台 system Worker 自身也不进入
tenant Workers Logs，防止 collector 递归采集自己。

所有生产 `WorkerLoaderWorkerCode` assembly point 现在都通过 typed observability attachment helper 生成 tails 与
immutable identity；validation/probe/system path 不挂 collector。该 topology 与 stock-workerd spike 一并固定在上述
P7 fixture，storage 不依赖内容 hash 猜测去重。

### 4.2 身份与 policy props

不能把 workerd 自动生成的 `TraceItem.scriptName` 直接当 external identity；动态 loader key 是内部实现细节。
RuntimeSnapshot 增加只供 system Worker 使用、不会进入 tenant env 的观测 identity：

```text
schemaVersion
accountId
workerId                 internal only, never returned as script_name
scriptName               Cloudflare external identity
versionId                Cloudflare Version ID
deploymentId             active Deployment ID
routeGeneration
observabilityGeneration  Script setting generation
effectiveLogPolicy
```

identity 由 Rust RuntimeSource 从 `control.sqlite` authority 生成并签入现有 authenticated snapshot。同时，`ocd`
维护 generation-scoped、由 RuntimeSource resolution 填充的 `workerd runtime name -> external identity/effective policy`
registry。Tail WorkerEntrypoint 的 props 必须是冻结、secret-free、size-bounded 的 target identity；batch 内每个
`TraceItem.scriptName` 只作为该可信 registry 的 lookup key，不能直接返回给客户端。lookup miss、跨 account 或 stale
generation 时丢弃对应 item 并计数，不能回退成 root Script，也不接受 tenant 传入的 account/script/version header。

Service Binding 和 DO 子调用按真正执行代码的 target Script/Version 记账；不能归到 root ingress Worker。target
Script 的 session 只接收映射到自己的 item，producer/root session 不聚合它。若 Cloudflare hosted trace 证明存在额外
root/subrequest 关联，先冻结 differential，再决定是否增加公开 trace/span 字段；不能先用错误的 script identity 或
重复 event 代替。

### 4.3 Collector 到 Rust backend

workerd config 增加仅回环的 `observability-backend` external service，和现有 `runtime-source`、`binding-backend` 一样由
`ocd` 注入 generation-local address。请求必须带独立 generation credential，不能复用 tenant binding token。

collector 显式把 `TraceItem` 规范化为版本化 ingest envelope；不得直接 `JSON.stringify()` 后让 Rust 接受任意对象。
envelope 只包含 allowlist 字段，执行以下限制：

- 最大 batch、event、log count、exception count、header count、key/value bytes 和 nesting depth；
- number 必须 finite，timestamp 必须在合理时钟偏差内；
- 不调用 `request.getUnredacted()`；
- 丢弃函数、symbol、prototype、循环引用和非 allowlist `cf` 字段；
- structured log 使用 bounded structured-clone projection；不能执行 tenant getter 或 `toJSON`；
- envelope 带 schema version、collector event ID 和 props identity；Rust 再验证 account/script/version authority。

backend 只在 workerd generation credential 当前有效时接收。旧 child 或重启前的 credential 不能写入新 generation。

## 5. Canonical invocation 与 event model

一个 workerd `TraceItem` 对应一个 canonical invocation。Workers Logs 将其投影为零个或多个 event：

1. `invocation_logs=true` 时产生一条 `cf-worker-event`；
2. 每条 `console.debug/info/log/warn/error` 产生一条 `cf-worker-log`；
3. 每条 uncaught exception 产生一条 error event；
4. diagnostic channel event 在 Day 1 保存在 invocation detail 中，不独立开放任意 dataset；
5. realtime tail 始终发送整个 canonical invocation，而不是持久化后的 event rows。

canonical invocation 至少包含：

```text
invocation_id / event_id
account_id / script_name / version_id / deployment_id
event_timestamp_ms / received_at_ms
event_type / event payload / entrypoint
outcome / execution_model / cpu_time_ms / wall_time_ms
request metadata / response status
logs[] / exceptions[] / diagnostics[]
durable_object_id / truncated
```

public Telemetry event 使用官方字段：

- dataset 固定为 `cloudflare-workers`；
- `$metadata.cloudService="workers"`；
- `$metadata.service` 与 `$workers.scriptName` 使用 external `script_name`；
- `$metadata.requestId` 使用 opaque `invocation_id`；
- `$workers.scriptVersion.id` 使用 Cloudflare Version ID；
- `$workers.eventType`、outcome、CPU/wall time 从 canonical invocation 映射；
- invocation log 的 `$metadata.type="cf-worker-event"`；custom log 的 type 为 `cf-worker-log`。

未在本地拓扑存在的 region、ray ID、colo、cost、provider 字段省略，不填假值。optional 字段的 omitted/null 差异以
固定 OpenAPI 和 differential fixture 为准。

### 5.1 Structured logs

Cloudflare Workers Logs 会保留单个 structured object 并发现其 keys。Day 1 规则在 M0 fixture 冻结后实现：

- `console.log({ ... })` 的单一 plain object 投影为 object `source`；
- primitive 或多参数调用投影为 Cloudflare-compatible string `source`；
- realtime `trace-v1` 仍保留 `logs[].message` array，不复用持久化 `source`；
- object 最多 32 层、256 个 indexed leaf；超出部分被截断并置 `$cloudflare.truncated=true`；
- key 长度、value bytes、array length 和总 event bytes 受平台 hard ceiling 约束。

具体字符串格式不能使用 Node `util.inspect` 的偶然版本行为；必须由 fixture 定义稳定 serializer。

### 5.2 Redaction

Cloudflare TailRequest 已默认 redaction，平台仍需在 system collector 和 Rust ingestion 两侧 fail closed：

- header name 为 `cookie`/`set-cookie`，或大小写不敏感地包含 `auth`、`key`、`secret`、`token`、`jwt` 时，
  value 固定为 `REDACTED`；
- URL 按官方 Tail Handler 的长 hex/base64-like identifier 规则 redaction；
- internal generation token、binding token、tail ticket、upload token 和 `x-open-compute-*` internal headers 永不进入 event；
- request/response body、secret binding value、D1/KV/R2 value、Queue body不采集；Vectorize vector/metadata、
  AI Search query/messages/item 原文/parsed text/chunks/results、Markdown Conversion 输入输出、provider
  request/response/endpoint/credential 也不进入 canonical invocation；
- collector 永不调用 `getUnredacted()`；
- auth/tail tickets 不进入 access log、metrics label、SQLite、error 或 support bundle。

`console.log()` 本身可以包含应用 secret，平台无法可靠判断任意业务数据。Logs API 因此属于 privileged management
surface；文档和 Dashboard 必须提示不要记录 secret。support bundle 默认不包含 `observability.sqlite` 或日志内容。

### 5.3 Truncation

按以下顺序裁剪，保证 JSON 始终有效：

1. 去除非 allowlist `cf` 和 diagnostics 字段；
2. 截断 structured value 的深度、leaf 和 string/bytes；
3. 截断单条 log/exception；
4. 截断 logs/exceptions 尾部；
5. 最后保留 invocation summary 与 `truncated=true`。

不得按 raw bytes 切开 UTF-8 或 JSON。`trace-v1` 和 persisted event 都暴露 truncation；不能在 storage 层静默丢尾部。

## 6. `observability.sqlite`

### 6.1 Authority 边界

`control.sqlite` 继续拥有 Script/Version/Deployment/setting metadata。`observability.sqlite` 是有 retention 的日志
event store：丢失不会改变 Worker routing、binding、secret 或部署状态，但会丢失历史 logs。因此它单独 migration、
quota、integrity check 和 maintenance，不加入 S3 artifact GC。

当前 Day 1 schema：

```text
observability_invocations
  invocation_id PK
  account_id, script_name, version_id, deployment_id
  event_timestamp_ms, received_at_ms, event_type, outcome
  cpu_time_ms, wall_time_ms, truncated, event_json, byte_size

observability_events
  event_id PK
  invocation_id FK
  account_id, script_name, version_id
  timestamp_ms, sequence, metadata_type, level
  source_json, metadata_json, byte_size

observability_fields
  event_id FK
  key, value_type, string_value, number_value, boolean_value

observability_maintenance
  singleton, accounted_bytes, last_gc_at_ms

SQLite user_version + observability_meta(data_format)
```

必要索引至少覆盖 `(account_id, timestamp_ms, event_id)`、`(account_id, script_name, timestamp_ms)`、
`(account_id, invocation_id)`、`(account_id, version_id, timestamp_ms)` 和 field key/type。所有 query 首先以 account 和
timeframe 限定候选，不能对全库执行无界 JSON scan。

`observability_fields` 只索引 allowlist metadata 和 bounded structured leaf。原始 `source_json` 仍保留在 event row；
keys/values/query 不承诺 Cloudflare hosted 的无限 cardinality 或任意深度索引，这是明确的本地 deviation。

### 6.2 写入与 quota

Rust backend 验证 envelope 后先 fan-out live sessions，再尝试把需要持久化的事件放入有界 ingest queue。写入 task
按 batch size 或 flush deadline 在单 transaction 中写 invocation、events 和 fields。

关键语义：

- tenant invocation 已经结束，日志写入失败不能把原 invocation 改成 error；
- queue full、SQLite busy/corrupt、disk guard 或 quota 触发时，丢日志并增加 metrics，不能无限等待或 OOM；
- realtime client 不等待 SQLite commit；Workers Logs query 只读取已提交数据；
- persistence sampling 在入 queue 前决定；live tail session sampling 独立决定；
- `max_database_bytes` 是 hard ceiling。先清理过 retention 数据，仍超限时按 received time 驱逐最旧 invocation；单个
  invocation 已超过 ceiling 或 SQLite physical page ceiling 时拒绝该 persisted copy，不删除 control metadata，也不影响
  Worker traffic；
- retention 表示“最多保留”，hard quota 可以使实际 retention 缩短，Dashboard/capability 必须显示当前 oldest event；
- maintenance 使用 bounded batch，避免大 delete transaction 长时间锁库。

startup 先完成 schema migration 和 quick integrity check，再开放 Workers Logs query。observability DB 失败不阻止
tenant ingress readiness，但 `/client/v4/.../observability/...` 返回 retryable unavailable，capability/status 显示
degraded。operator 可以显式修复或清空 logs DB；该破坏性操作属于 vendor maintenance endpoint，不伪装成 CF API。

## 7. Script Tails API：`wrangler tail`

Day 1 注册官方路径：

```text
GET    /client/v4/accounts/{account_id}/workers/scripts/{script_name}/tails
POST   /client/v4/accounts/{account_id}/workers/scripts/{script_name}/tails
DELETE /client/v4/accounts/{account_id}/workers/scripts/{script_name}/tails/{tail_id}
```

`POST` 接受固定 Wrangler 生成的 body：

```json
{
  "filters": [
    { "sampling_rate": 0.25 },
    { "outcome": ["ok", "exception"] },
    { "method": ["GET", "POST"] },
    { "header": { "key": "content-type", "query": "json" } },
    { "client_ip": ["self"] },
    { "query": "invoice" },
    { "scriptVersion": "<version-id>" }
  ]
}
```

成功 response 采用 v4 envelope，result 精确包含固定 schema/trace 要求的：

```json
{
  "id": "<opaque-tail-id>",
  "expires_at": "<timestamp>",
  "url": "wss://compute.example.internal/<opaque-path-and-ticket>"
}
```

返回 URL 是 opaque data，不要求属于 `/client/v4` path。固定 Wrangler 的 WebSocket handshake 不携带 Bearer token，
因此 URL 必须包含短期、不可猜、scope 到 account/script/tail/expiry 的 signed ticket。ticket：

- 不落入 SQLite、access log、metrics 或 error；
- 只允许一个 active WebSocket，且仅在 session TTL 内有效；
- 使用每进程独立随机 signing key 和 `v1` ticket claim，不复用 API token/master encryption key；进程重启会使旧 ticket
  与 ephemeral session 同时失效；
- session delete、expiry 或 Script delete 立即失效；配置中的 API token 变更随 daemon restart 生效并同时回收 session；
- URL 生成时使用经过 allowlist 的 configured external control origin，不能信任 Host/X-Forwarded-Host。

tail session 是 process-local ephemeral state，不写 `control.sqlite`。`ocd` 重启后 GET 返回空 collection，旧
WebSocket 关闭，Wrangler 按自身 reconnect 流程创建新 session。创建会话不改变 Script setting、Deployment 或
runtime generation，因为平台 collector 始终挂载。

### 7.1 `trace-v1` WebSocket

handshake 必须：

- 要求 `Sec-WebSocket-Protocol: trace-v1` 并回显同一 subprotocol；
- 验证 signed ticket、session、origin-independent expiry 和 single active connection；
- 只接受固定 Wrangler 连接后的首个 text frame `{"debug":false}`；
- 接受固定 Wrangler 对该首帧使用的实际 masking 行为；若 WebSocket library 默认拒绝 unmasked client frame，必须在
  L0 证明可安全配置或替换协议层，不能要求 fork Wrangler；
- 正确响应 WebSocket ping/pong，至少通过 Wrangler 10 秒 ping Gate；
- server expiry 使用正常关闭并立即回收 session；protocol、auth、oversize 和 slow-consumer 使用明确 close code；
- binary frame、压缩炸弹、额外 control JSON 和超大 frame fail closed。

每个 data frame 是一个固定 `trace-v1` JSON event，至少保持 Wrangler printer 使用的字段：

```text
outcome
scriptName
exceptions[] { name, message, stack?, timestamp? }
logs[]       { level, message, timestamp? }
eventTimestamp
event
entrypoint?
scriptVersion?
executionModel?
truncated?
cpuTime?
wallTime?
```

必须为 HTTP、scheduled/cron、alarm、queue、email、tail、RPC、hibernatable WebSocket 和 unknown event 建立 golden
fixture。不存在的字段按 trace 冻结为 omitted 或 null，不能为了 Rust DTO 方便全填空值。

hidden `--debug` 会发送 `{"debug":true}`，但其 filter diagnostics frame 尚未冻结。Day 1 capability 明确标记
`tail.debug=false-only`；收到 true 时以 policy violation 关闭，不能假装支持并泄漏被 filter 排除的事件。

### 7.2 Filter 语义

filter array 之间为 AND；单个 array value 内为 OR。固定 Wrangler 已经把 CLI status 映射为 outcome：

```text
ok       -> ok
canceled -> canceled
error    -> exception | exceededCpu | exceededMemory | unknown
```

server 实现：

| filter | 语义 |
| --- | --- |
| `sampling_rate` | 每 session、每 canonical invocation 稳定 sampling；只接受 `0 < rate < 1` |
| `outcome` | 精确匹配官方 outcome enum；未知值创建 session 时拒绝 |
| `method` | 大小写标准化后匹配 HTTP method；非 HTTP event 不匹配 |
| `header` | header name 大小写不敏感，query 按固定 trace 语义；只在 redacted view 上匹配 |
| `client_ip` | 匹配可信 ingress peer；`self` 在创建 session 时解析为管理客户端 peer IP，不信任 XFF |
| `query` | 只搜索 canonical `console.*` message，大小写/structured serialization 由 golden fixture 冻结 |
| `scriptVersion` | 精确匹配 Cloudflare Version ID，而非 Deployment ID 或内部 worker ID |

请求包含重复同类 filter、空数组、未知 key、错误类型、NaN/Infinity、过长 query/header 或超出总 filter 数时，在
创建 session 前返回 CF-style validation error 和 JSON pointer。不能创建后静默忽略。

### 7.3 慢客户端与 overload

每个 session 有独立 bounded byte queue。先执行 filter，再做 session sampling，最后 enqueue。queue 满时不能反压
tenant invocation 或全局 collector：

1. 丢弃该 session 的 event，其他 session 不受影响；
2. queue 首次超限时插入一次合法 `trace-v1` overload info event；
3. 恢复到 low watermark 后插入一次 `overload-stop` event；
4. 连续超限或达到 drop/time hard ceiling 时以 retryable close code 断开；
5. `tail_events_dropped_total{reason}` 只使用 bounded labels，不含 script/user/token。

overload info event 的 exact message 和 omitted/null 字段必须来自固定 Wrangler/Cloudflare fixture，因为 pretty printer
显式识别 `event.type="overload"` 与 `"overload-stop"`。

## 8. Workers Logs Telemetry API

Day 1 注册：

```text
POST /client/v4/accounts/{account_id}/workers/observability/telemetry/keys
POST /client/v4/accounts/{account_id}/workers/observability/telemetry/values
POST /client/v4/accounts/{account_id}/workers/observability/telemetry/query
```

仅支持 dataset `cloudflare-workers`。dataset omitted 表示该 account 的所有已支持 dataset，目前仍只返回这一项；未知
dataset 返回 unsupported，不静默当空结果。

### 8.1 `keys` 与 `values`

`keys` 返回当前 retention window 内可查询字段的 `key`、`type` 和 `lastSeenAt`。固定 metadata keys 总是按真实数据
存在性返回，structured keys 来自 `observability_fields`。同一路径出现多种类型时按官方 differential 结果处理，
不能任意 coercion。

`values` 必须指定一个已知 key，返回 bounded distinct value、type 和 dataset。该固定官方请求没有 cursor 字段，高
cardinality key 因此只支持显式 limit；实现使用 SQL `DISTINCT ... LIMIT`，不能把所有用户 ID 加载到内存。
Secret-like header value 已被 redaction，因此原始值不出现在 values。

### 8.2 `query`

Day 1 支持 inline ad-hoc query，required fields 是固定 OpenAPI 要求的 `queryId` 与 `timeframe`。saved `queryId`
lookup 不支持；当请求省略 inline parameters 试图加载 saved query 时返回 unsupported。

支持的 response view：

| `view` | Day 1 |
| --- | --- |
| `events` | 支持；返回 individual persisted events、fields、count 和 opaque offset |
| `invocations` | 支持；按 `$metadata.requestId` 分组 invocation summary/custom/error events |
| `calculations` | 不支持；聚合、chart、distribution、group by 后置 |
| `traces` | 不支持 |
| `agents` | 不支持 |
| `requests` | 在 M0 证明为 `invocations` alias 前不支持 |

`timeframe.from < timeframe.to`，范围不得超过本地 retention/query hard ceiling。`limit` 最大 2000；`offset` 是 server
生成、绑定 account/query-order/timeframe 的 opaque cursor，客户端不能构造内部 row ID。排序固定为 timestamp + event
ID 的稳定倒序，翻页不能重复或跳过同一 snapshot 内的数据；retention 并发删除导致 cursor 过期时返回明确错误。

filter AST 沿用官方 shape：顶层 `filterCombination`，leaf 为 `key`/`operation`/`type`/`value`，group
`kind="group"`，最大嵌套 4。实现同一个 typed evaluator 供 query 和 Telemetry Live Tail 使用，支持官方当前列出的：

```text
includes / not_includes / starts_with / ends_with / regex
exists / is_null
in / not_in
eq / neq / gt / gte / lt / lte
= / != / > / >= / < / <=
以及对应 uppercase aliases
```

实现要求：

- type 必须与 key 的实际 `string|number|boolean` 一致，不做字符串数字 coercion；
- string comparison 大小写敏感；
- regex 使用线性时间、RE2-compatible 语义，拒绝 lookaround/backreference 和超长 pattern；
- metadata fixed columns 尽量下推 SQLite；dynamic leaf 通过 bounded field index；
- candidate rows、CPU deadline、result bytes 和 discovered fields 都有 hard ceiling；
- 超限返回 retryable/limit error，不能返回看似完整的部分结果；
- query response 只填本地真实存在的 optional metadata，不伪造 Cloudflare region/ray/cost。

Day 1 不实现 `needle`、chart、calculations、groupBys、orderBy、compare、distribution 或 saved query。它们出现时在执行
SQL 前返回精确 unsupported pointer。

## 9. Telemetry Live Tail

Dashboard/官方 SDK 的新入口是：

```text
POST /client/v4/accounts/{account_id}/workers/observability/telemetry/live-tail
POST /client/v4/accounts/{account_id}/workers/observability/telemetry/live-tail/heartbeat
```

prepare body 支持 official `scriptId`、`filterCombination` 和同第 8 节的 filter AST，result 返回 `{wsUrl}`。
该 session 与 Script Tail 共用每 Script 10 client limit、canonical feed、redaction 和 slow-client queue，但拥有不同的
protocol adapter。Day 1 要求 `scriptId`；省略它的 account-wide Live Tail 明确 unsupported，不能默认为全账号日志。

以下信息在公开 API 页面不足以定义 wire contract，必须由固定 Cloudflare Dashboard/SDK trace 冻结：

- `wsUrl` 使用的 subprotocol、auth ticket 和首帧；
- live event frame 与 keepalive frame；
- heartbeat 如何识别 session，是否延长 eligibility 或 TTL；
- normal expiry、filter error、slow consumer 和 revocation 的 close code；
- `scriptId` 是 script name 还是另一 external ID 的具体映射。

因此不能把 Script Tail 的 `trace-v1` URL 原样返回给 Telemetry Live Tail。L4 完成 trace fixture、adapter 和真实
Dashboard/SDK Gate 后，该 endpoint 才从 capability 的 `unsupported` 变为关联
`OC-OBSERVABILITY-001` 的 `supported_with_deviation`。

heartbeat 只更新 process-local session eligibility，不写 logs DB。无 heartbeat、ticket expiry、配置 token 变更触发的
daemon restart、Script tombstone、超过 client limit 或 `ocd` restart 都回收 session。realtime tail 永不 replay SQLite
历史数据。

## 10. 认证与权限

沿用主设计三类 token，不复制 Cloudflare 完整 IAM：

| 操作 | admin | deployer | read-only |
| --- | --- | --- | --- |
| 修改 observability settings | 允许 | 允许 | 拒绝 |
| query Workers Logs | 允许 | 允许 | 允许 |
| 创建/list/delete tail session | 允许 | 允许 | 允许 |
| observability maintenance/clear | 允许（vendor endpoint） | 拒绝 | 拒绝 |

对外 token verify 返回稳定的 `workers_observability:read`、`workers_observability:write`、
`workers_tail:read` 等 scope 名称；内部可映射到三类角色。Cloudflare OpenAPI 某些 Telemetry read 操作要求
Observability Write，实际 accepted permission 以固定 schema 为准；open-compute 不能因为内部 role 简化而改变公开
permission/error contract。

WebSocket ticket 不是 API token，不能调用 REST、查询历史 logs 或创建新 session。tail URL 泄漏的影响被限制到单一
account/script/session/expiry；ticket 在 URL 中出现是客户端合同所需，因此所有 HTTP/access log 必须在解析前清洗。

## 11. 故障、恢复与生命周期

| 故障 | 预期行为 |
| --- | --- |
| collector backend unavailable | producer invocation 不受影响；live/persisted event 丢弃并计数 |
| ingest queue full | 只丢 persisted copy；live fan-out仍按各自 queue 决定 |
| `observability.sqlite` busy | bounded retry；超过 deadline 后丢弃，不阻塞 workerd |
| DB corrupt/migration failed | Logs API degraded；tenant ingress 与 control API 继续可用 |
| disk hard guard | 先停 persisted logs，再保护 control/DO/D1 等 authority |
| realtime client slow | 只影响该 session，overload 后必要时关闭 |
| `ocd` restart | ephemeral sessions 消失；已提交 logs 保留；未提交 queue 允许丢失，重启后的 queue depth 从零开始 |
| workerd child restart | generation ticket 失效；新 child 用同一 Script settings 重新挂 collector |
| Script tombstone | 拒绝新 tail，关闭旧 session；历史 logs 保留至 retention |
| Version/Deployment 删除 | 历史 logs 保留 external IDs；不得因 FK cascade 提前删除 |
| clock rollback | 接受窗口内的 event timestamp 原样保留；同一 retained row set 仍按 timestamp + opaque event ID 稳定翻页，但不承诺 reception-order，可能出现跨页期间新写入事件的时间重排 |

平台不承诺 exactly-once log delivery。目标是：同一 ingest envelope 在本进程 retry 时通过 collector event ID 幂等，
正常路径不重复；child/process crash 窗口允许 event loss。该 deviation 必须在 capability 和运维文档中可见。

## 12. Metrics、状态与审计

新增 bounded metrics，label 不含 account、script、session、event、URL 或用户输入：

```text
open_compute_observability_ingest_total{result}
open_compute_observability_events_total{kind,result}
open_compute_observability_ingest_queue_depth
open_compute_observability_db_bytes
open_compute_observability_oldest_event_age_seconds
open_compute_observability_truncated_total{stage}
open_compute_observability_tail_sessions
open_compute_observability_tail_events_total{result}
open_compute_observability_tail_dropped_total{reason}
open_compute_observability_query_total{view,result}
open_compute_observability_query_duration_seconds
```

P5 已有的 `vectorize_*`、`ai_search_*`、`ai_provider_*` 和 indexing stage metrics 继续属于 operator product
metrics，不转换为 `cf-worker-log`、exception 或 synthetic Worker invocation。同步 Vectorize/AI Search/Markdown
Conversion binding 调用只影响真实 caller invocation 的 wall time、subrequest outcome 和未捕获异常；后台 mutation、
parse/embed/index/GC 没有 tenant Worker invocation，不写入 Workers Logs。产品 metrics 只保留 operation/stage/outcome
等现有低基数 label，不能增加 query、model endpoint、instance、item、document、vector 或 tenant identity label。

`/client/v4/open-compute/system/status` 和 capabilities 只返回 health、容量、retention、oldest event、session count 和
drop counters，不返回 log contents 或 tail URLs。

审计只记录管理操作：setting change、tail create/delete、query metadata、maintenance。query audit 记录 timeframe、view、
result count 和内容无关的字段命名空间（dataset、timestamp、source、metadata、workers 或 other），不记录具体 filter key、
filter value、search text 或 event source。realtime event 本身不写 audit log。

## 13. Capability manifest

`share/cloudflare-capabilities.json` 增加一个 authority section，不建立第二份 manifest。至少表达：

```text
workers.observability.settings.fields
workers.observability.logs.persistence
workers.observability.telemetry.keys
workers.observability.telemetry.values
workers.observability.telemetry.query.views
workers.observability.telemetry.query.operators
workers.observability.telemetry.liveTail
workers.scripts.tails
workers.scripts.tailProtocol = trace-v1
workers.tailConsumers
workers.streamingTailConsumers
workers.logpush
workers.traces
limits.retentionMs / eventBytes / databaseBytes / tailClients / queryEvents
deviations.delivery / topology / indexing / retention
```

field、endpoint、view、operator 和 CLI flag 各自有 `supported`、`supported_with_deviation` 或 `unsupported`。不能只写
`workersLogs:true` 掩盖 traces、destinations、debug、query calculations 等缺口。

## 15. 回归所有权

原生事件、Wrangler tail、SDK Telemetry、Dashboard、权限／审计及重启恢复由对应产品测试覆盖；
精确 case 见 [`test/gate_cases.py`](../../test/gate_cases.py)。实际最终验收保留在第 17 节，
hosted 长尾、性能与跨平台资格见[扩展验收计划](../acceptance/p7-observability-extended-acceptance.md)。

## 16. 已接受 deviation

| deviation | 决策 |
| --- | --- |
| 单机而非全球日志管道 | API/wire 尽量一致，不承诺全球顺序、region/colo 或 hosted availability |
| delivery 非 exactly-once | bounded best-effort；crash/overload 可丢，正常进程内 retry 幂等 |
| 独立本地 SQLite | 部署方显式 hard quota、最多 7 天；不把 LynxOS 安装默认值伪装成 CF plan/pricing |
| dynamic field index 有界 | depth/leaf/value/cardinality 有平台 ceiling，capability 公开 |
| retention 是最大值 | hard quota/disk guard 可缩短；公开 oldest event 和 drop metrics |
| no replay realtime tail | realtime 只发送 session active 后事件；历史查询走 Workers Logs |
| process restart 终止 tail | session ephemeral；Wrangler 可重建，历史 committed logs 保留 |
| nested target 不聚合到 caller tail | 每个执行 target 独立采集、归属和 tail；caller 必须单独 tail target Script；hosted attribution 待扩展 differential |
| optional Cloudflare metadata 缺失 | region/ray/cost/provider 等省略，不填假值 |
| query 只支持 events/invocations | calculations/traces/agents/saved queries 明确 unsupported |
| Tail Workers/exports 后置 | 平台 collector 是内部机制，不冒充用户配置的 `tail_consumers` |

任何新增 deviation 必须写明官方来源、可观察差异、影响的 CLI/SDK/Dashboard、错误行为和回归 case。

## 17. 实际完成与验收结果

结论为 **Implementation GO**。固定客户端和本地 Day 1 核心已经进入唯一 production path；没有旧 observability
schema、双写、fallback、历史协议选择或半套成功响应。仓库级静态检查、90% coverage 与最终单轮 Gate 已完成。剩余
hosted 长尾、跨平台和性能资格由独立 active acceptance 跟踪，不扩大当前 capability。

完成证据：

- `wrangler@4.127.1 tail` 的声明 flags、`trace-v1`、10-client admission、删除/重启、JSON/pretty 输出通过真实
  Wrangler subprocess；`cloudflare@7.1.0` Telemetry 与 Dashboard Live Tail/heartbeat 通过真实 `ocd + stock workerd`；
- 生产 WorkerLoader assembly point 使用统一 target-own collector；validation/system path 不挂 collector，nested target
  不错误聚合到 caller，generation token、API token、tail ticket、Version secret 和敏感 header/URL 经过失败路径验证；
- 独立 `observability.sqlite` 的 migration/checksum、采样、批量持久化、retention/quota、query、进程内 session、重启恢复、
  audit/status/metrics 和 failure isolation 已覆盖；
- capability、OpenAPI、SDK types、Dashboard、default config、runbook、support bundle 与 compatibility/deviation authority 已同步；
- Tail Workers、Streaming Tail Workers、traces、非空 destinations、Logpush、calculations、agents/requests 与 saved queries
  继续 fail closed；
- `bun run build`、`bun run check:generated`、214/214 JavaScript tests、14/14 conformance cases、Rustfmt、
  `--no-default-features -D warnings`、Rust 1.98 all-targets、metadata、dependency boundaries 和 `git diff --check` 通过；
- canonical Clippy 以 `--workspace --all-targets --all-features --keep-going -- -D warnings` 通过；不存在此前记录的
  repo-wide lint 阻断；
- `./test/coverage.sh` 的完整 49-target Gate 与 **1,107/1,107 cases** 通过，production Rust line coverage 为
  **106,499 / 118,313 = 90.0146%**；没有降低 90.00% 门槛、扩大排除规则或把生产逻辑移入测试路径。报告为
  `.temp/gate-run/20260904T183339-2530b7a2/report.json`，HTML/LCOV/JSON 位于 `target/llvm-cov/`；
- 最终非插桩 `./test/gate.py --workspace` 单轮通过 **49/49 targets、1,107/1,107 cases**，792.76 秒；报告为
  `.temp/gate-run/20260904T184550-21c28bfc/report.json`，冻结 Gate source SHA-256 为
  `5080cd1f3bc00154f8d90c10d8c9ab166df1bd0f549fb9a471cf52a0666c6040`；
- `cf-compatibility-check` 依据固定 workerd/Workers types/Wrangler、正式 capability authority 与 Cloudflare Workers
  Logs、Real-time Logs、Telemetry、Tail Workers 官方合同完成复核，无阻断项且无需改变 workerd pin。单机
  persistence/session、nested target attribution、Script Tail GET list shape 与 unsupported Tail Workers/traces 等差异继续由
  `OC-OBSERVABILITY-001` 精确登记。

仍未完成的 hosted Script Tail 长尾、nested Service/DO/Workflow/Queue 托管端 attribution differential、参数化性能水位和
跨平台发行资格记录在 [`P7 observability 扩展差分与发行验收`](../acceptance/p7-observability-extended-acceptance.md)。这些限制不撤销
本地 repository acceptance，也不允许把完整 hosted parity 或 release qualification 写成已通过。

## 18. 官方参考

- [Workers Logs](https://developers.cloudflare.com/workers/observability/logs/workers-logs/)
- [Real-time logs](https://developers.cloudflare.com/workers/observability/logs/real-time-logs/)
- [Tail Handler](https://developers.cloudflare.com/workers/runtime-apis/handlers/tail/)
- [Tail Workers](https://developers.cloudflare.com/workers/observability/logs/tail-workers/)
- [Wrangler `tail`](https://developers.cloudflare.com/workers/wrangler/commands/workers/#tail)
- [Workers Script Tail API](https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/tail/)
- [Workers Observability Telemetry API](https://developers.cloudflare.com/api/resources/workers/subresources/observability/subresources/telemetry/)
- [Prepare Telemetry Live Tail](https://developers.cloudflare.com/api/resources/workers/subresources/observability/subresources/telemetry/methods/live_tail/)
- [Run a Telemetry query](https://developers.cloudflare.com/api/resources/workers/subresources/observability/subresources/telemetry/methods/query/)
- [Cloudflare workers-sdk](https://github.com/cloudflare/workers-sdk)
- [Cloudflare API schemas](https://github.com/cloudflare/api-schemas)

实施时必须把这些动态来源收敛成固定 revision/hash 和仓库内的无敏感 golden fixtures；链接本身不证明 conformance。
