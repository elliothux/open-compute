# P8：Workers Standard limits 设计

状态：设计完成，待实施与验收。

本文细化 [P6 Cloudflare v4 API 与 Wrangler 子集兼容设计](p6-cloudflare-v4-wrangler-compatibility.md)
中的 limits 合同。它只定义 open-compute 平台本身，不包含 LynxOS 的团队规模、单机容量默认值、部署拓扑
或运维策略。

本文所说的 **Standard** 是 Cloudflare 当前 `usage_model: "standard"` 与 Wrangler `limits` 字段的合同，
不是为 open-compute 引入 Free/Paid 计费套餐。open-compute Day 1 只有一个 Standard runtime profile；计费、
每日请求额度和商业 plan 不在兼容范围内。

## 1. 结论

1. limits 必须拆成四类，不能用一张“本地限额表”混在一起：
   - Cloudflare 可观察的 Worker runtime 合同；
   - upload、metadata、ingress、logging 等 open-compute 能独立执行的确定性边界；
   - 必须由 stock workerd 执行的 isolate/request limits；
   - 部署方容量保护，它不是 Cloudflare API 合同。
2. Day 1 目标 profile 对齐 Cloudflare Workers Standard/Paid 的 runtime 数值；不实现 Free plan，也不伪造
   Cloudflare account plan。
3. 固定的 stock workerd standalone `LimitEnforcer` 当前明确不执行 CPU、subrequest、memory 或连接配额。
   因此这些字段在真实执行器可用之前必须保持 capability `unsupported`，不能接受后静默忽略。
4. `limits.cpu_ms` 与 `limits.subrequests` 是 Version 配置。Wrangler 上传、v4 settings、持久化、runtime
   snapshot、Workers Logs outcome 和 Dynamic Worker lower-limit 语义必须端到端一致。
5. 部署方可以配置 admission、队列、磁盘、全局并发和进程回收阈值，但这些字段只能出现在 vendor
   capability/config 中，不能改变官方 `/client/v4` 的 limits 含义，也不能被命名为 Cloudflare plan。

## 2. Authority 与版本固定

实施时固定以下输入及 SHA-256：

- `wrangler@4.127.1/config-schema.json`；
- `@cloudflare/workers-types@5.20260830.1`；
- open-compute formal runtime pin `workerd v1.20260830.1` 及仓库 `references/workerd` 对应 revision；
- Cloudflare [Workers limits](https://developers.cloudflare.com/workers/platform/limits/)；
- Cloudflare [Workers Scripts API](https://developers.cloudflare.com/api/resources/workers/subresources/scripts/)；
- Cloudflare [Dynamic Workers limits](https://developers.cloudflare.com/dynamic-workers/platform/limits/)；
- Cloudflare [Dynamic Workers custom resource limits](https://developers.cloudflare.com/dynamic-workers/usage/limits/)；
- Cloudflare [Vectorize limits](https://developers.cloudflare.com/vectorize/platform/limits/)；
- Cloudflare [AI Search limits](https://developers.cloudflare.com/ai-search/platform/limits-pricing/)；
- 已归档的 [P5 Vectorize 与 AI Search 实现合同](implemented/p5-vectorize-ai-search.md)及
  [完成记录](implemented/p5-vectorize-ai-search-results.md)；
- [Workers Logs 与 realtime tail 专项设计](p7-workers-logs-realtime-tail.md)。

网页只用于发现最新合同。release qualification 使用固定的 schema、types、workerd source snapshot 和
Cloudflare differential fixture；网页变化不能在未 review 时自动改变运行参数。

## 3. limits 分类

### 3.1 `cf_standard`

对 Worker 作者可见、与 Cloudflare Standard runtime 对齐的合同。只有满足以下任一条件才能标记
`supported`：

- open-compute 在管理面或 ingress 精确执行；
- 固定 stock workerd 有源代码和 conformance case 证明执行；
- Cloudflare 本身明确没有 hard limit，open-compute 也没有增加可观察的 Worker 级限制。

### 3.2 `platform_hard`

为了协议完整性、安全或有界资源使用而执行的硬边界，例如 multipart 总大小、JSON 深度、内部 frame
大小。这类限制如果不属于 Cloudflare 公开合同，必须：

- 在 vendor capabilities 中用独立名称报告；
- 不复用 `limits.cpu_ms` 等官方字段；
- 对公开请求返回确定、可测试的错误；
- 不泄漏 filesystem path、内部 service 名或 secret。

### 3.3 `product`

KV、D1、R2、Vectorize、AI Search、Workers AI Markdown Conversion、Queues、Workflows、Cache、Images 等产品
自己的限制继续由各产品 authority 管理。本文只规定 request-scoped subrequest 计数如何覆盖这些 binding 的
调用；不复制各产品 limit matrix，也不把 P5 的模型、文件、向量或索引额度改名为 Workers Standard limits。

计数边界位于 tenant 可观察的 binding 调用，而不是产品内部 fan-out：一次 facade 到 authenticated binding backend
的逻辑请求按一个 internal-service subrequest 资格化；redirect/retry/poll 等由调用方实际再次发起的请求分别计数。
AI Search backend 内部的 embedding、rewrite、rerank、generation、S3 和 indexing coordinator 操作，以及 Vectorize
异步 mutation coordinator，不额外消耗原 Worker invocation 的 subrequest budget。这个单位必须由固定 stock
workerd/source fixture 和 Cloudflare differential 冻结；在证据完成前不能仅按方法名猜测 supported。

### 3.4 `operator_capacity`

全局并发、admission queue、数据库连接、磁盘水位、WorkerLoader cache 回收、AI provider request pool、
indexing/parser child 并发和 child process restart 属于部署方容量保护。它们：

- 没有 Day 1 固定人数或单机默认值；
- 不进入 Cloudflare settings/multipart；
- 只能通过 vendor config/capabilities/metrics 暴露；
- 触发时返回 overload/unavailable，而不是伪装成 CPU、memory 或 subrequest 超限。

## 4. Day 1 标准矩阵

当前 `WorkersConfig` 不能直接沿用为 v4/Standard authority：

- `max_bundle_bytes = 17 MiB` 限制旧 `WorkerBundleV1` 整体，不区分 Cloudflare 的 gzip code size、raw code、
  multipart metadata 与 Static Assets；
- `max_request_body_bytes = 16 MiB`，且当前 validation 将 request body 与 bundle 都封顶在 `64 MiB`，达不到
  Standard ingress 的 100 MB baseline；
- retention、delete drain、recovery batch 等字段是 control-plane/operator policy，不是 Worker runtime limits；
- 当前 capability registry 报告这些配置值，但不能把它们投影成 `usage_model` 或 `limits` response。

v4 replacement 实施时删除旧 bundle/body 值对公开合同的 authority，分别建立 CF structural contract 与
`operator_capacity`。这不是把 LynxOS 安装默认值上移到 open-compute。

### 4.1 可由管理面或边缘 listener 执行

| 合同 | Standard 目标 | 执行位置 | Day 1 判定 |
| --- | ---: | --- | --- |
| Worker gzip 后代码 | 10 MB | 按固定 Wrangler/Cloudflare code-size 算法由 server 重算 | 待实施 |
| Worker 未压缩代码 | 64 MB | module decoder 累加原始 module body | workerd Dynamic Worker 已有；普通 upload 待统一 |
| text + secret variables | 128 / Worker | metadata canonicalization | 待实施 |
| 单 variable 值 | 5 KB | metadata/secrets mutation | 待实施 |
| URL | 16 KB | public ingress 解析前 | 待实施 |
| request headers 总量 | 128 KB | public ingress | 待实施 |
| response headers 总量 | 128 KB | tenant response 出口 | 待实施 |
| request body | 100 MB baseline | public ingress 流式计数 | 当前值不合格，待迁移 |
| response body | 无 Worker hard limit | 保持 streaming，不全量 buffer | 待审计 |
| 每 request log data | 256 KB | Tail collector 在结构化编码前计量 | 见 Logs 专项 |
| Static Asset 单文件 | 25 MiB | Assets upload session | 待 v4 Assets 实施 |

这里的 100 MB 是 Cloudflare 当前 request body 表中最低的 account-plan 值，用作 open-compute 唯一
Standard ingress baseline；它不是 LynxOS 或某个安装的容量默认值。部署方 reverse proxy 若设置更低边界，
必须在 installation capabilities 中显式报告，且该安装不能声称通过该项 Cloudflare differential。

Worker size 必须按 Worker code 计算，不能把 multipart metadata、Static Assets manifest 或平台内部 canonical
descriptor 混入 10 MB gzip 数值。source map 与多 module 的具体聚合方式由固定 Wrangler trace 和 Cloudflare
boundary differential 锁定，不自行发明“逐文件 gzip”算法。为防止压缩炸弹，未压缩边界先执行，压缩数值再由
server 以固定算法重算；server 不信任客户端提供的 size。

### 4.2 必须由 runtime 执行

| 合同 | Standard 目标 | Cloudflare 行为 | 当前 stock workerd 事实 | Day 1 状态 |
| --- | ---: | --- | --- | --- |
| HTTP CPU 默认值 | 30,000 ms | 可配置到 300,000 ms | standalone `enterJs()` no-op | blocked |
| subrequests 默认值 | 10,000 / invocation | 可配置到 10,000,000 | `newSubrequest()` no-op | blocked |
| memory | 128 MB / isolate | 超限产生 `exceededMemory` | `NullIsolateLimitEnforcer` 无 heap policy | blocked |
| startup | 1 second | upload validation 可报 `10021` | standalone startup scope 无 limit | blocked |
| 同时等待 response headers 的连接 | 6 / top-level request | Service Bindings 共享计数 | standalone 无对应计数 | blocked |
| HTTP wall time | client 保持连接时无 hard limit | response/disconnect 后 `waitUntil()` 最多 30 s | 必须做 differential | 未资格化 |
| Cron / Queue / DO alarm wall time | 15 min | invocation-type limit | 现有 scheduler timeout 不是充分证据 | 未资格化 |

`blocked` 的含义不是“未来不做”，而是当前实现不得把它写成 supported。特别禁止：

- 把外层 wall-clock timeout 称为 CPU time；
- 只统计 KV/D1/R2 facade 而漏掉 global `fetch()`、redirect、Service Binding 或 TCP；
- 用整个 workerd child 的 RSS 冒充 128 MB per isolate；
- 接受 `limits` 后只写数据库或日志；
- 根据错误字符串猜测 `exceededCpu` / `exceededMemory`。

### 4.3 不属于 open-compute Standard runtime 的商业额度

以下 Cloudflare 数值依赖 Free/Paid、CDN、Zone 或商业 account，不作为 open-compute runtime 默认值：

- 100,000 requests/day；
- Workers per account；
- Cron Triggers per account；
- Routes、Custom Domains、routed zones；
- CDN cache object/body 上限；
- limit increase request 与 billing overage。

open-compute 可以有部署方 capacity ceiling，但它必须属于 `operator_capacity`，不得复用上述名称或错误码。

## 5. Wrangler 与 v4 wire contract

### 5.1 配置语法

固定 Wrangler schema 的唯一用户字段是：

```jsonc
{
  "usage_model": "standard",
  "limits": {
    "cpu_ms": 30000,
    "subrequests": 10000
  }
}
```

规则：

- `usage_model` 只接受 `standard`；deprecated `bundled` / `unbound` fail closed；
- `cpu_ms`、`subrequests` 必须是 JSON finite integer，不接受 string、float、负数或未知 key；
- `cpu_ms` 范围目标为 `1..300000`；省略时 effective value 为 `30000`；
- `subrequests` 范围目标为 `1..10000000`；省略时 effective value 为 `10000`；
- 若 runtime capability 尚未通过 Gate，出现任一 `limits` 字段时 upload 必须失败，不能降级成 warning；
- 即使用户省略字段，平台也只有在默认值真实执行时才能标记 Standard runtime limits supported。

### 5.2 Multipart metadata

Wrangler 生成的 metadata 中保留官方名称：

```json
{
  "main_module": "index.js",
  "compatibility_date": "2026-09-02",
  "usage_model": "standard",
  "limits": {
    "cpu_ms": 30000,
    "subrequests": 10000
  }
}
```

server canonicalization 必须区分：

- `omitted`：采用当时固定 Standard default；
- explicit value：持久化精确整数；
- `null`、空 object 中的未知字段、重复 multipart metadata：拒绝。

### 5.3 Version authority

Cloudflare Version 捕获 code、assets、bindings 和 compatibility configuration 的完整状态。open-compute 因此把
limits 存在 immutable Version，而不是可变 Deployment 或进程内 map：

```text
worker_versions
  usage_model             TEXT NOT NULL CHECK usage_model = 'standard'
  cpu_ms                  INTEGER NOT NULL
  subrequests             INTEGER NOT NULL
  limit_contract_revision TEXT NOT NULL
```

每次 direct upload 或 `versions upload` 都 materialize effective values。Deployment 只选择 Version，不复制或
覆盖 limits。rollback 到旧 Version 必须恢复旧 limits；secret/binding/limits 变化都创建新 Version。

`GET /accounts/{account}/workers/scripts/{script}/settings` 返回选中 Version 的 `usage_model` 与 `limits`；
`script-settings` 只返回真正的 Script-level settings，不把 limits 错放进去。

### 5.4 Runtime snapshot

经 descriptor digest 认证的 immutable runtime snapshot 必须包含：

```json
{
  "usageModel": "standard",
  "limits": {
    "cpuMs": 30000,
    "subRequests": 10000
  },
  "limitContractRevision": "cf-workers-standard-2026-07-28"
}
```

字段名在存储/API 使用 Cloudflare snake_case，在 workerd native `ResourceLimits` 使用 `cpuMs` 与
`subRequests`；转换只能存在一处并有 round-trip test。

## 6. Enforcement architecture

### 6.1 单一 effective-limit 计算

```text
Wrangler metadata
  -> strict v4 decoder
  -> Standard range validation
  -> immutable Version effective limits
  -> signed runtime snapshot
  -> request-scoped runtime enforcer
  -> outcome + logs + metrics
```

不能在 Dashboard、SDK、Rust service、loader-host 和 workerd 分别补默认值。canonicalization 后 Version 中不允许
nullable effective limits。

### 6.2 runtime enforcer Gate

符合 Day 1 的执行器必须同时覆盖：

- JavaScript、Wasm、Python 与 startup CPU；
- incoming fetch、scheduled、queue、alarm、RPC/Service Binding；
- global `fetch()`、redirect chain、KV、D1、R2、Cache、Queues、Service Binding 和 TCP connection admission；
- parent Worker 与 Dynamic Worker 各自的 request scope；
- exception/abort/stream cancel 后的计数清理；
- warm isolate、cold isolate、concurrent request 与 child process restart。

允许的交付路径只有两条：

1. pin 一个 upstream stock workerd release，其 standalone server 提供可配置且经测试的 enforcer；
2. 先向 upstream workerd 合入该能力，再 pin 正式 release。

不接受维护 open-compute 私有 workerd fork，也不接受 JavaScript wrapper 近似 CPU/heap 计量。

### 6.3 current fail-closed policy

在 runtime enforcer Gate 通过前：

- capabilities 对 request CPU/subrequest/memory/startup/connection limits 返回 `unsupported` 与
  `OC-WKR-LIMIT-001`；
- 普通 Worker upload 若显式声明 `limits`，返回 Cloudflare v4 failure envelope；
- `worker_loaders` 整体仍受 Dynamic Workers G0 Gate 阻断，因为其 runtime `limits` 不能被静默接受；
- 文档、Dashboard、SDK 不显示“已启用 30s CPU”等误导状态；
- 不把现有外层 request timeout 写入 `limits` response。

## 7. Dynamic Worker limits

Dynamic Workers 复用 parent Worker 的 Standard plan limits，并允许两层更低限制：

```ts
const worker = env.LOADER.get("content-addressed-id", async () => ({
  compatibilityDate: "2026-09-02",
  mainModule: "index.js",
  modules: { "index.js": source },
  limits: { cpuMs: 1000, subRequests: 100 },
}));

const entrypoint = worker.getEntrypoint(undefined, {
  limits: { cpuMs: 100, subRequests: 10 },
});
```

effective value 对每个维度取：

```text
min(parent Version limit, WorkerCode limit, entrypoint invocation limit)
```

省略某层不代表 unlimited。两层对象使用 camelCase `cpuMs` / `subRequests`，与 Wrangler snake_case 不同。
超限应立即让 Dynamic Worker invocation 抛异常，并在 parent/tail observability 中保留真实 outcome。

此外，Cloudflare 当前限制每个 Worker request 同时有 4 个 distinct Dynamic Workers in flight；同一 Dynamic
Worker 的多个并发 request 只计一个。Durable Object context 的 10 个数值只有实现 Dynamic Object Facets 时才进入
范围；本文不因此引入 Facets。stock workerd 当前没有可证明的 4-worker request scope enforcement，因此仍是
blocked，不得用平台全局并发阈值替代。

详细 Worker Loader 合同、stock workerd nesting blocker 与 cache identity 见
[Dynamic Workers / Worker Loader 专项设计](p9-dynamic-workers-worker-loader.md)。

## 8. Errors 与 observability

### 8.1 upload / settings

- transport body 过大在读取完整 body 前返回 HTTP `413`；
- Worker code、变量或 metadata 语义超限使用标准 v4 `{success:false, errors:[...]}` envelope；
- exact Cloudflare error code/message 由固定 credential differential 捕获，不在实现前猜测；
- vendor diagnostics 可添加稳定 `source.pointer`，但不能改变官方 error array shape。

### 8.2 runtime

完成执行器后对齐 Cloudflare：

- CPU / memory limit 对外为 Worker resource-limit failure，Cloudflare 公共错误为 `1102`；
- invocation outcome 分别是 `exceededCpu`、`exceededMemory`；
- subrequest limit 在触发调用处抛异常，不能继续发出真实下游请求；
- Dynamic Worker custom limit 在 child invocation 内立即失败；
- cleanup、tail delivery 与 metrics 不计入 tenant invocation 的可用 CPU/subrequest budget。

在 stock workerd 没有可信 outcome 前，collector 不得根据异常文本合成这些 outcome。

### 8.3 capabilities

vendor capability authority 至少报告：

```json
{
  "workers": {
    "usage_model": "standard",
    "limits": {
      "contract_revision": "cf-workers-standard-2026-07-28",
      "cpu_ms": { "status": "unsupported", "deviation": "OC-WKR-LIMIT-001" },
      "subrequests": { "status": "unsupported", "deviation": "OC-WKR-LIMIT-001" },
      "memory_bytes": { "status": "unsupported", "deviation": "OC-WKR-LIMIT-001" },
      "worker_code_uncompressed_bytes": { "status": "planned", "value": 67108864 }
    }
  }
}
```

`operator_capacity` 使用另一棵 key，不能塞进 `workers.limits`。

## 9. 实施顺序

### L0：contract inventory

- 从固定 Wrangler schema 提取 `usage_model`、`limits.cpu_ms`、`limits.subrequests`；
- 固定 Scripts settings、multipart、Workers limits 与 Dynamic Workers fixtures；
- 枚举当前全部 tenant-visible internal-service bindings，包括 Vectorize、AI Search namespace/instance 与
  Markdown Conversion，并冻结每个方法产生的 logical backend request 数；
- 为每个 limits 项标记 `management`、`ingress`、`runtime`、`product` 或 `operator`；
- 将当前 implementation evidence 与 target 分列。

Exit：不存在“配置支持”等同“运行时执行”的模糊状态。

### L1：确定性 structural limits

- 分离 multipart transport、Worker code、assets 与 internal descriptor size；
- 实现 64 MB raw / 10 MB gzip code 计量；
- 实现 variables、URL、headers、100 MB body 与 256 KB logs 边界；
- 流式读取和 streaming response 不引入无界 buffer。

Exit：边界值、边界减一/加一、chunked、伪造 Content-Length、gzip bomb 全部有 case。

### L2：Version/settings contract

- v4 decoder、immutable Version schema、settings response 与 rollback；
- capability revision 与 exact defaults 单一 authority；
- Wrangler deploy / versions upload / settings get trace fixture。

Exit：显式 limits 在执行器未就绪时 fail closed；省略值不产生虚假 supported 状态。

### L3：upstream runtime enforcer

- upstream/pin stock workerd request/isolate enforcer；
- 覆盖 CPU、subrequest、memory、startup、connection 与 invocation types；
- 实现 Dynamic Worker 三层 `min()`；
- 真实 outcome 进入 Logs/tails/metrics。

Exit：source inspection、fault injection、black-box case 与 Cloudflare differential 全部通过。

### L4：qualification

- 当前固定 Wrangler 全命令 subprocess；
- cold/warm、concurrent、streaming、cancel、restart、rollback；
- ordinary Worker 与 Dynamic Worker 同一组 adversarial cases；
- 更新 compatibility matrix、deviation registry 与 capability manifest。

## 10. 必测矩阵

| case | 预期 |
| --- | --- |
| `cpu_ms` omitted / min / max / max+1 | default materialize；合法边界通过；越界失败 |
| `subrequests` 9,999 / 10,000 / explicit 10M / 10M+1 | 精确计数；越界配置失败 |
| redirect chain | 每一跳计 subrequest |
| Service Binding chain | 与 top-level request scope 共享连接计数；各 invocation CPU 独立 |
| KV/D1/R2/Vectorize/AI Search/`env.AI` + global fetch 混合 | 每个 tenant-visible logical call 进入同一 subrequest budget，无漏计/双计；产品内部 provider/S3/coordinator fan-out 不重复计数 |
| AI Search `uploadAndPoll()` | upload 和每次显式 poll 分别计数；后台 parse/embed/index 不继续占用原 invocation budget |
| Vectorize async mutation | submit 调用计数；后台 durable apply 不产生伪 subrequest |
| six pending headers + seventh | 第七个按 CF 行为排队，不提前发出 |
| 128 MB isolate pressure | outcome 为 `exceededMemory`，新 request 不复用失效 isolate |
| 1 second startup | upload/Version validation 失败，未创建可部署 Version |
| client disconnect | waitUntil 最多延长 30 s |
| Dynamic Worker code + entrypoint limits | 每维取最小值 |
| four distinct Dynamic Workers + fifth | 第五个在调用前失败；同 ID 并发只计一个 |
| process restart / rollback | effective limits 从 immutable Version 恢复 |
| forged Content-Length / chunked overflow | 读取中止，临时文件清理，mutation budget 释放 |
| log 256 KB + 1 byte | 后续 log context 截断，invocation 本身不因 collector 崩溃 |

## 11. Definition of Done

本文只有同时满足以下条件才可归档：

- `usage_model: standard` 与 `limits` 通过固定 Wrangler upload/settings round trip；
- 64 MB raw、10 MB gzip、variables、URL、headers、body、log 边界有真实实现和测试；
- CPU、subrequest、memory、startup、connections 由固定 stock workerd 真实执行，不是 wrapper 或外层近似；
- subrequest Gate 覆盖 KV、D1、R2、Vectorize、AI Search、`env.AI`、Service Binding 与 global `fetch()`，并证明
  产品内部 provider/S3/coordinator fan-out 不被重复记到 tenant invocation；
- Dynamic Worker 的 inherited/code/entrypoint limits 和 distinct-in-flight limit 通过 Gate；
- 所有运行时超限产生真实、可追溯的 outcome，并进入 Workers Logs/realtime tail；
- capabilities 把 Standard contract、product limits 与 operator capacity 分开；
- `OC-WKR-LIMIT-001` 仅在事实已改变且 regression Gate 通过后才能关闭；
- Cloudflare differential 固定输入与结果；若因 credential 未运行，拆成 active acceptance，不能标记通过；
- docs links、`git diff --check`、focused tests、coverage 与最终单轮 workspace Gate 均通过。
