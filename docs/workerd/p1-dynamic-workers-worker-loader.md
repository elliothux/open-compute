# P1：Dynamic Workers / Worker Loader 设计

状态：**原生 fork 路线待实施，未完成验收**。2026-09-05 用户确认在 `third_party/workerd/` 的用户 fork
上实现并重新编译，见[原生方案](native-limits-loader.md)与[源码基线](README.md)。此前 stock pin 的 Loader
转移失败、custom limits 不执行仍是有效历史证据，详见 [DW-G0 复核记录](../implemented/p10-worker-loader-feasibility.md)。
DW1–DW5 未实施、未验收；选择 fork 不代表公开 binding 已可启用。

本文细化 [P6 Cloudflare v4 API 与 Wrangler 子集兼容设计](../implemented/p6-cloudflare-v4-wrangler-compatibility.md)
中的 `worker_loaders` binding，以及 open-compute 已有的内部 WorkerLoader 调度。limits 的统一定义见
[Workers Standard limits 专项设计](p2-workers-standard-limits.md)，日志与 tails 见
[Workers Logs / realtime tail 专项设计](../implemented/p7-workers-logs-realtime-tail.md)。


2026-09-06 调整交付顺序：先完成 P1 原生 Loader，再实现 P2 Standard limits。P1 的范围不包含默认
CPU/内存/subrequest enforcement 或 custom limits；显式 limits 必须由原生 API 拒绝，不能静默忽略。
P1 仍须完成 namespace/权限隔离、结构大小限制、in-flight 计数、缓存与生命周期及正式 pin 验收。
该子集不宣称完整 Cloudflare 资源限制兼容，也不保证失控代码不会影响同进程邻居；P2 完成后消除此偏差。

## 1. 范围与结论

Day 1 目标是 Cloudflare 当前公开的 **Dynamic Workers**：

- Wrangler `worker_loaders = [{ binding }]`；
- `env.LOADER.load(code)`；
- `env.LOADER.get(id, getCodeCallback)`；
- `WorkerStub.getEntrypoint()` / `getDurableObjectClass()`；
- `WorkerCode` 的 modules、env、network、tails 与 resource limits。

明确不在范围内：

- Workers for Platforms；
- dispatch namespaces、dynamic dispatch Worker、user Worker namespace、WfP outbound Worker；
- WfP 管理 API、tags、pricing 或 tenant script lifecycle；
- 用 Dynamic Worker 取代普通 Scripts / Versions / Deployments。

普通应用仍是标准 Worker Script/Version/Deployment。Dynamic Worker 是一个普通 Worker 在 invocation 内按需创建的
child isolate，不创建 Script、Version 或 Deployment 记录。

当前最终结论：

1. open-compute 内部已经正确使用 stock workerd `WorkerLoader.get()` 加载普通 tenant Worker，但这不等于
   tenant Worker 已获得公开 `env.LOADER` binding。
2. open-compute tenant Worker 本身是 Dynamic Worker。固定 stock workerd 的 `DynamicWorkerSource.env` 只能转移
   structured-clone value 与 service/actor/RPC capability，不能转移另一个原生 WorkerLoader channel。因此原生
   **nested Worker Loader** 当前不可用。
3. JavaScript facade 可以同步返回本地 handle，但这不等于原生 capability、facets、错误和生命周期全合同兼容。
   本方案选择原生实现；不将普通 Worker 改成部署时需全 runtime restart 的 static config。
4. 正式路径是在 `third_party/workerd/` 用户 fork 中实现 native delegation/namespace capability、in-flight 与生命周期，
   完成验证并协调更新 formal pin；upstream 合并不再是本地实施前提。
5. G0 未通过前，`worker_loaders` upload 必须 fail closed，capability 保持 unsupported。不能把内部 LOADER 暴露
   给 tenant，也不能让 `limits` 静默 no-op。

## 2. Authority

实施和 qualification 固定：

- [Cloudflare Dynamic Workers overview](https://developers.cloudflare.com/dynamic-workers/)；
- [Getting started](https://developers.cloudflare.com/dynamic-workers/getting-started/)；
- [Worker Loader API reference](https://developers.cloudflare.com/dynamic-workers/api-reference/)；
- [Bindings](https://developers.cloudflare.com/dynamic-workers/usage/bindings/)；
- [Egress control](https://developers.cloudflare.com/dynamic-workers/usage/egress-control/)；
- [Observability](https://developers.cloudflare.com/dynamic-workers/usage/observability/)；
- [Custom resource limits](https://developers.cloudflare.com/dynamic-workers/usage/limits/)；
- [Dynamic Workers platform limits](https://developers.cloudflare.com/dynamic-workers/platform/limits/)；
- `wrangler@4.127.1/config-schema.json`；
- `@cloudflare/workers-types@5.20260830.1`；
- open-compute 当前 formal pin `workerd v1.20260830.1`；用户 fork checkout 的不同 revision 见[基线](README.md)。

Wrangler、types 与 workerd source 是可复现 authority。Cloudflare 网页用于发现合同，必须转成固定 fixture 后才能
进入 Gate。

## 3. 术语与两种 Loader

| 名称 | 所有者 | 可见性 | 用途 |
| --- | --- | --- | --- |
| system loader | open-compute | internal only | loader-host 加载普通 Script Version |
| public Worker Loader binding | Worker 作者 | `env.<binding>` | Worker 在 invocation 内创建 Dynamic Worker |
| ordinary Worker | control plane | Script/Version/Deployment | 可部署、可回滚的应用 |
| Dynamic Worker | parent invocation | 无 control-plane resource | runtime 提供的临时或可复用 child isolate |

system loader 与 public loader 可以复用同一个 upstream primitive，但必须是不同 capability namespace。禁止把
`packages/runtime/config.capnp` 中 `id = "open-compute"` 的 system namespace 直接传给 tenant；否则 tenant 可猜测
内部 runtime key、命中别人的 cached isolate，或影响平台调度。

## 4. 官方配置与 upload contract

### 4.1 Wrangler

唯一配置是 binding name：

```jsonc
{
  "$schema": "./node_modules/wrangler/config-schema.json",
  "name": "dynamic-parent",
  "main": "src/index.ts",
  "compatibility_date": "2026-09-02",
  "worker_loaders": [
    { "binding": "LOADER" }
  ]
}
```

TOML 等价形态是 `[[worker_loaders]] binding = "LOADER"`。Day 1 不增加 `namespace`、`id`、`policy`、`shared`
或 open-compute 自定义 key。

规则沿用固定 Wrangler schema：

- array item 必须是 object；
- `binding` 必须是 string；
- 不允许 additional property；
- named environment 中 non-inheritable，必须显式声明；
- binding name 与其他 bindings 共用唯一性校验。

### 4.2 Multipart metadata

固定 Wrangler 生成标准 binding：

```json
{
  "bindings": [
    { "name": "LOADER", "type": "worker_loader" }
  ]
}
```

v4 decoder 必须接受且只接受 `{name,type}`。Worker Loader 不指向外部 resource，因此：

- 不做 provisioning API；
- 不接受 resource ID；
- 不创建 control-plane namespace row；
- binding descriptor 是 immutable Version state；
- download/settings response round-trip 原样返回 `type: "worker_loader"`。

G0 未通过时，包含该 binding 的 upload 使用标准 v4 failure envelope 拒绝；不得删除 binding 后继续部署。

2026-09-05 源码复核：当前 `WorkerUploadBinding` 是 closed serde enum，没有 `worker_loader` variant；
multipart 在进入 Version mutation 前解码失败。现有 system loader 属于普通 Worker 的生产执行路径，
不是待删除的 public-loader 兼容实现。此次未加入 placeholder descriptor、namespace row 或半成品 tenant binding。

## 5. JavaScript API contract

### 5.1 `load(code)`

```ts
load(code: WorkerLoaderWorkerCode): WorkerStub
```

- 每次调用创建 unnamed Dynamic Worker；
- 不按 ID 复用；
- 适合一次性 code execution；
- 返回值是同步 `WorkerStub`，第一次实际 invocation 可以等待异步 startup；
- 不能因为 JS handle 被 GC 就取消仍在进行的 startup/request。

`load()` 不是“永久禁止 cache”。官方 contract 只保证不按调用者提供的 ID cache；runtime 可以在 stub 生命周期内
重建 isolate，调用者不能依赖 global state。

### 5.2 `get(id, callback)`

```ts
get(
  id: string | null,
  getCode: () => WorkerLoaderWorkerCode | Promise<WorkerLoaderWorkerCode>,
): WorkerStub
```

- 同步返回 `WorkerStub`；
- cache miss 时 callback 可异步取 code；
- 同 ID 可以复用 warm isolate，但没有复用保证；
- runtime 可调用 callback 零次、一次或多次；
- 同一 namespace + ID 每次必须返回完全相同的 WorkerCode；任何变化都必须换 ID；
- startup/callback 失败在对 stub 的 invocation 上抛出；失败不能永久 poison 该 ID。

推荐 ID 是 content digest 或 `<logical-name>:<immutable-version>`。平台不替用户重写业务 ID，也不根据 callback
返回值偷偷生成新 ID；违反 immutability 是调用者错误。

### 5.3 `WorkerStub`

```ts
interface WorkerStub {
  getEntrypoint(name?: string, options?: WorkerStubEntrypointOptions): Fetcher;
  getDurableObjectClass(name?: string, options?: WorkerStubEntrypointOptions): DurableObjectClass;
}
```

规则：

- `undefined`、`null` 和 `"default"` 均选择 default entrypoint，按固定 workerd 行为 qualification；
- unknown named entrypoint 在 invocation 时抛 `Worker has no such entrypoint` 类错误；
- stub/entrypoint 不保证两次 invocation 命中同一 isolate；
- Dynamic entrypoint 本身不可被当作 transferable service binding 再跨任意 boundary 传递；
- `props` 与 `limits` 在 entrypoint invocation scope 生效。

### 5.4 `WorkerCode`

| 字段 | 必需 | Day 1 语义 |
| --- | --- | --- |
| `compatibilityDate` | 是 | 与 Wrangler compatibility date 同义，按固定 runtime 校验 |
| `compatibilityFlags` | 否 | child 自己的 flags，不继承 parent array |
| `allowExperimental` | 否 | parent 必须有 `experimental`；production 不宣称 experimental compatibility |
| `mainModule` | 是 | 必须引用 `modules` 中存在的 module |
| `modules` | 是 | 至少一个 module；完整类型见下节 |
| `env` | 否 | structured-clone value 与显式 capability bindings |
| `globalOutbound` | 否 | omitted 继承 parent；`null` 完全阻断；Fetcher 重定向 egress |
| `tails` | 否 | invocation 完成后发 Tail Event |
| `streamingTails` | 否 | experimental；Day 1 public subset 不开放 |
| `limits` | 否 | `cpuMs` / `subRequests` lower limits；受 limits Gate 阻断 |

WorkerCode dictionary 的未知字段沿用 native JSG 忽略语义（见[上游 #5681](../references/workerd-upstream.md)）；
已知字段的类型、必需值与权限仍须验证。此规则不改变 upload metadata 的严格校验。

## 6. Module contract

支持的值与固定 `workers-types` / workerd 一致：

```ts
type DynamicModule =
  | string
  | WebAssembly.Module
  | { js: string }
  | { cjs: string }
  | { py: string }
  | { text: string }
  | { data: ArrayBuffer }
  | { wasm: ArrayBuffer | ArrayBufferView | WebAssembly.Module }
  | { json: unknown };
```

校验规则：

- plain string 只根据 `.js` / `.py` suffix 解释；其他 plain-string name 拒绝，Python 特殊目录按固定 workerd
  行为 qualification；
- object 恰好包含一个已知 type field；零个或多个已知 type fields 拒绝，未知附加键按 native dictionary 语义忽略；
- JSON 必须可序列化，不能把 capability 藏进 JSON；
- module names 作为模块 specifier，不映射到 host filesystem；禁止 NUL、无界长度和重复 canonical name；
- `mainModule` 必须存在，空 modules 拒绝；
- 固定 stock workerd 已执行 Dynamic Worker code 未压缩总量 64 MiB；计量覆盖 JS/CJS/Python/text/data/Wasm/JSON
  的实际 body，边界 case 需与 source 实现一致；
- Dynamic Worker runtime API 接收已经组成的 modules，不自动运行 npm、shell、TypeScript compiler 或任意 bundler。

`@cloudflare/worker-bundler` 是应用层可选 library，不是 open-compute daemon 的隐式功能。若 LynxOS agent 使用它，
那属于调用方 Worker code 和 dependency policy，不改变 Worker Loader 合同。

## 7. Bindings 与 capability sandbox

### 7.1 `env`

Dynamic Worker 只拥有调用者显式放入 `env` 的 capability。标准路径是 parent export 一个
`WorkerEntrypoint`，通过 `ctx.exports.Class({props})` 创建 service stub，再传给 child：

```ts
const child = env.LOADER.get(id, async () => ({
  compatibilityDate: "2026-09-02",
  mainModule: "index.js",
  modules,
  env: {
    STORAGE: ctx.exports.ScopedStorage({ props: { prefix } }),
  },
  globalOutbound: null,
}));
```

open-compute 不把 KV/D1/R2/Vectorize/AI Search native facade、Workers AI transport、内部 token、provider
credential、physical resource ID 或 filesystem path 直接放入 child env。parent 若要给 child 资源，必须显式传递
受 scope 的 service capability；authority 仍由 parent Version 的 binding、permissions 和 runtime snapshot 决定。

P5 已实现的能力进入同一规则，不获得特例：

- Vectorize wrapper 分开 query/read 与 insert/upsert/delete 权限，并固定一个 parent 已绑定的 index；
- AI Search instance wrapper 可只开放 search/chat，namespace wrapper 的 create/update/delete/items/jobs 必须额外
  获得 parent 的 write permission，并可继续收窄 instance allowlist；
- Markdown Conversion wrapper 只转发 `toMarkdown()`/`supported()`，不因 parent 存在 `env.AI` 就暴露完整 Workers
  AI、provider endpoint 或 credential；
- wrapper 的 props 只含 opaque scope 和有界 policy，不能包含原始文档、向量、查询内容或 secret。

固定 stock workerd 对 serialized `env` 有约 1 MiB 硬边界。Day 1 保留该确定性边界并通过 types/source fixture
锁定；它不能被误写成“128 个 variables × 5 KB”的精确 Cloudflare plan 等价物。

### 7.2 network

对齐官方三态：

| `globalOutbound` | child 行为 |
| --- | --- |
| omitted | 继承 parent 的 global `fetch()` / `connect()` authority |
| `null` | global `fetch()` / `connect()` 抛异常 |
| Fetcher | 全部 global outbound 重定向到该 service capability |

open-compute 不为了“更安全”擅自把 omitted 改成 `null`。应用若执行不可信或 AI-generated code，应显式使用
`null` 或 egress proxy；平台文档和 examples 默认展示 `null`，但 runtime contract 仍保持继承语义。

### 7.3 tails

`tails` 接受可转移 Fetcher/WorkerEntrypoint stub。child invocation 完成后产生一个 Tail Event，包含 console、
exception 与 request metadata；delivery 不增加 child response latency。open-compute system tail collector 与用户
传入 tails 是两个 consumer，必须 fan-out，不能互相替代。

Dynamic Worker 的 `console.log()` 不会自动出现在 parent Worker Logs；system collector 必须在创建 child 时附加，
然后按 parent account/script/version 与 dynamic ID 做 scope-safe attribution。详细 schema、256 KB/request 边界、
live tail session 与 sampling 见 Logs 专项。

`streamingTails` 在固定类型里存在，但依赖 experimental trust。Day 1 拒绝 non-empty 值，不通过
`allowExperimental:true` 偷偷开放。

## 8. limits

Dynamic Worker 默认继承 parent Standard limits，并允许：

- `WorkerCode.limits: {cpuMs, subRequests}`；
- `getEntrypoint(..., {limits})`；
- 每个维度取 parent、code、entrypoint 中最小值。

Cloudflare 当前还限制每个 ordinary Worker request 同时最多 4 个 distinct Dynamic Workers in flight；同一 ID 的
多个 in-flight requests 计一个。本文不引入 Dynamic Object Facets，因此不把 DO context 的 10 个限额扩展为新功能。

固定 stock workerd standalone 当前：

- `DynamicWorkerSource.limits` 能接收值，但 `WorkerLoaderNamespace` 创建 service 时未使用；
- `getEntrypoint(...limits)` / `getDurableObjectClass(...limits)` 的 limits 到 resolved channel 时被丢弃；
- standalone `LimitEnforcer` 的 CPU/subrequest 方法是 no-op；
- 没有证据证明 4 distinct in-flight limit 被执行。

P1 不等待 Standard limits 执行器。以上预算继承和 lower limits 是 P2 的目标合同，不是 P1 已支持行为。
P1 必须原生拒绝 WorkerCode、entrypoint/actor-class options 中显式提供的 limits（含空对象），覆盖 load/get
及 callback 路径；省略 limits 可使用已验收的 Loader 子集。未知字段仍遵循原生 dictionary 语义。
P1 实现 distinct in-flight 计数，默认 CPU/内存/subrequest 缺口以明确 deviation 发布，不声明完整资源隔离。

## 9. 当前 open-compute 路径与 blocker

### 9.1 已有 system path

```text
public request / scheduler event
  -> Rust resolves immutable deployment snapshot
  -> static loader-host Worker
  -> env.LOADER.get(open-compute runtime key, callback)
  -> callback assembles modules + scoped bindings
  -> tenant ordinary Worker executes as a workerd Dynamic Worker
```

这条路径已经满足 system-level 动态加载，但 system `LOADER` 只存在于 static loader-host 和 do-host。tenant 的
`env` 由 `DynamicWorkerSource.env` 构造，没有原生 WorkerLoader。

### 9.2 source evidence

| 事实 | 固定 source |
| --- | --- |
| WorkerLoader 是 static workerd binding group | `third_party/workerd/src/workerd/server/workerd.capnp` |
| config 中同 ID bindings 共享 native namespace | `Server::workerLoaderNamespaces` |
| Dynamic Worker env 只 rewrite subrequest/actor/RPC caps | `WorkerLoaderNamespace::WorkerStubImpl::start()` |
| WorkerLoader JSG object 没有 transferable token/channel | `api/worker-loader.h` |
| dynamic entrypoint 明确拒绝 transfer | `SubrequestChannelImpl::requireAllowsTransfer()` |
| named cache 由 namespace + name 查找 | `WorkerLoaderNamespace::loadIsolate()` |
| aborted named isolate 才从 map 移除 | `removeIsolate()` |

因此“把 system LOADER 塞进 `env`”不是少一行配置，而是 stock runtime 缺少一种 capability delegation。

已有能力及其来源统一见[上游核验](../references/workerd-upstream.md)。#4834 的静态 ctx.exports 传递与
#6822 的 persistent RpcStub env 支持均已存在；不能将 dynamic entrypoint 的 transfer 限制概括为所有 service binding
不可传递，也不能由 RPC 支持推导 Loader 可传递。按对象来源、channel 类型和实际 date/flags 分别建立正反例。

### 9.3 不采用的方案

| 方案 | 拒绝原因 |
| --- | --- |
| RPC/JS facade 模拟 `env.LOADER` | 同步 handle 可模拟，但完整 capability/facets/limits/错误与生命周期需要额外适配，不作为当前原生路线 |
| source rewrite 注入 facade | 改写 module/entrypoint/stack/source map，named export、RPC 与 DO 行为脆弱，形成私有 runtime |
| 每个 Script 写入 static workerd config | deploy 需要全进程 restart，无法保持 Versions/Deployments 与在线请求语义 |
| 多 workerd child / 每用户 child | 违反一个受监督 workerd child 的 Day 1 部署约束，也不是 Dynamic Workers isolate 模型 |
| 暴露 system loader namespace | 可猜 key、跨 tenant cache collision、内部 capability 泄漏 |

## 10. 选定的 native fork 方案

### 10.1 native primitive

在用户 fork 中补齐可转移但不可伪造的 Loader namespace capability（接口名以实际实现和上游评审为准）：

- static WorkerLoader binding 可 delegate 一个 loader capability 到 Dynamic Worker env；
- capability 绑定一个 native namespace，不暴露 namespace ID；
- receiving Dynamic Worker 看到原生 `WorkerLoader` JSG object，不是 RPC facade；
- `load/get` 与 `WorkerStub` 保持同步 API；
- channel 可配置是否允许再 delegate，Day 1 tenant capability 禁止无限递归转移但允许 tenant 自己 load child；
- tails、globalOutbound 与 abort lifecycle 沿用 native implementation；P1 为 limits 补明确拒绝，P2 接入执行器；
- namespace 有 bounded eviction/cleanup hooks 与 observability，不要求调用者管理 native pointer。

原生源码与 regression 统一修改 `third_party/workerd/`，具体分工见[原生方案](native-limits-loader.md)。
向 upstream 提案与贡献不阻断本地交付；G0 仍要求执行真实 fork 二进制并完成正式 pin 接入，不能仅凭源码 patch 通过。

接线顺序为：受信任 binding factory 创建 namespace capability → WorkerLoader JSG 序列化/反序列化 →
Frankenvalue cap table 与 Loader channel 类型 → 动态 env rewrite / WorkerDef channel 注册 → 接收 isolate 构造原生对象。
同进程优先复用现有引用与 cap table，不为传 env 另造持久化 token 协议。确需跨 RPC token 时复用现有完整性保护，
绑定 capability 类型与进程生命周期；租户提供的 namespace 字符串永远不能生成授权。
新增类型只开放 Loader 所需能力，不全局放宽 requireAllowsTransfer()。

### 10.2 namespace derivation

每个 public binding 的平台 identity 是：

```text
sha256("oc/public-worker-loader/v1" || account_id || script_id || binding_name)
```

它是内部 namespace key，不返回给 Worker。选择 Script 而不是 Version 是为了保持同一 ordinary Worker 更新后仍可
按相同 child ID 获得 cache opportunity；官方要求调用者在 child code/config 变化时使用新 ID。Deployment rollback
不会改变 namespace，但 callback immutability 规则仍成立。

system namespace 使用独立固定 domain separator 和 capability，public key 永远不能与 system runtime key 相交。
删除 Script 后 namespace tombstone，停止新 invocation；native cache 在 drain 后回收。重新创建同名 Script 使用新的
opaque `script_id`，不能复活旧 namespace。

### 10.3 outer 与 nested identities

ordinary Worker 的 system cache key 与 public Dynamic Worker ID 是两层独立 identity：

```text
system: account/script/version/runtime-policy-revision/entrypoint-mode
public: public-loader-namespace + caller-provided-dynamic-id
```

Deployment 只路由到 Version，不进入 code identity。compatibility、bindings、secrets、limits 或 module bytes 改变都
创建新 ordinary Version，因此 system key 改变。public child 的 code identity 由 parent 作者通过 ID 保证。

## 11. Cache 与 lifecycle

官方只提供 cache opportunity，不提供 isolate persistence guarantee。open-compute 必须保持：

- 同 namespace + ID 的并发 miss 合并 startup，不重复调用真实下游 code source；
- callback 可在 eviction/restart 后再次调用；
- isolate global state 随时可能消失；
- 不能保证两个 request 命中同一 isolate；
- failed startup/aborted isolate 从 named map 移除，下一次可重试；
- parent request cancel、JS GC、tail failure 不悬挂 native reference；
- workerd child graceful restart 会丢失全部 warm cache，但不丢 control-plane state；
- operator cache/admission 阈值是 vendor capacity，不是 Cloudflare Worker limit。

固定 OSS source 当前 named map 没有通用 eviction evidence。native fork G0 必须提供 bounded eviction，或提供可观测且
可安全触发的 namespace cleanup；不能靠无限 map 再用整进程 OOM 回收。

`load()` unnamed isolate 需在 startup 期间由 namespace 持有额外 ref，完成后由 stub/in-flight request 管理 lifetime。
这个 stock workerd 细节必须保留在 regression tests 中。

## 12. Security invariants

- public loader capability 只能来自该 Version 的 `worker_loader` binding；Worker 不能按字符串发现其他 namespace；
- system token、runtime source、binding backend、physical IDs 和 system Worker entrypoints 不可到达；
- child env 是 allowlist，不继承 parent env；network 只按 `globalOutbound` 三态处理；
- child compatibility flags 重新验证，parent experimental trust 不自动下放；
- modules/env 先做 native size/type 校验，再启动 isolate；
- binding/tail/globalOutbound capability transfer 必须验证 `requireAllowsTransfer()`；
- error、log、tail、metrics 不含 module source、secret value、props secret 或内部 namespace key；
- loader ID 可以是用户数据，日志只记录 bounded/redacted form 或 digest；
- Dynamic Worker 不能调用 open-compute control plane，除非 parent 显式给它相应 service capability；
- deletion/tombstone、Version rollback 与 process restart 都 fail closed，不回退到别的 Script/Version。

## 13. Error contract

### 13.1 deploy time

- G0 未通过：`worker_loader` binding 以 v4 failure envelope 拒绝；
- duplicate/invalid binding name：标准 binding validation failure；
- 不创建部分 Version，不留下可部署 artifact；
- exact Cloudflare error code/message 由固定 differential 捕获，不预先编造。

### 13.2 runtime

- invalid modules/main/compat/env/size 在 child startup 失败；
- callback reject 原样成为 child invocation failure，但经过平台 error redaction；
- unknown entrypoint 明确失败；
- `globalOutbound:null` 的 fetch/connect 明确抛网络权限错误；
- P1 显式 custom limits 明确拒绝；P2 实现后按真实超限 outcome 终止 child invocation；
- tail delivery failure 不改变已完成的 child response；
- namespace tombstone / parent Version unavailable 不使用 stale authority。

Workers Logs outcome 必须来自真实 runtime event，不能根据 message regex 合成。

## 14. 实施顺序

### DW0：固定合同

- 固定 Wrangler schema、upload binding、workers-types 与 Dynamic Workers docs fixtures；
- 建立 `load/get/WorkerStub/WorkerCode` conformance inventory；
- 将 Workers for Platforms/dispatch namespace 明确列为 out of scope；
- 为现有 internal WorkerLoader 建立 source evidence。

Exit：公开 Dynamic Workers 与内部 system loader 不再混称。

### DW-G0：native fork capability Gate

- 用户 fork 的 native loader capability delegation；
- nested Dynamic Worker 获得真实 `WorkerLoader` JSG object；
- 所有显式 resource limits 由 native call 明确拒绝，省略时可加载；默认资源限制偏差可见；
- 4 distinct Dynamic Workers per request 被执行；
- named cache bounded eviction/abort cleanup；
- fork native regression 通过，open-compute 协调更新到同一正式固定的 fork artifact。

Exit：正式固定的原生实现成立，不依赖 facade、source rewrite 或 deployment-triggered 全 runtime restart。

若 G0 失败，Day 1 `worker_loaders` 保持 unsupported；这不会阻断普通 Scripts/Versions/Deployments，但本文不能归档。

### DW1：v4 与 Version model

- Wrangler field inventory；
- multipart `worker_loader` binding decode/encode；
- immutable Version persistence、download/settings/rollback；
- capabilities 与 unsupported-to-supported migration。

### DW2：runtime assembly

- 从 Version binding 创建 public namespace capability；
- 通过原生 Loader channel 放入 ordinary tenant Worker env；
- namespace domain separation、tombstone 与 cleanup；
- ordinary system runtime key 改为 Version authority，禁止 active/deployment alias 作为 code key。

### DW3：API 与 sandbox

- modules、compat、env、globalOutbound、tails、entrypoints；
- `load()` unnamed lifecycle；
- `get()` named cache、async callback、failure retry；
- custom bindings 与 scope-safe KV/D1/R2/Vectorize/AI Search/Markdown Conversion resource facade；
- experimental streaming tails fail closed。

### DW4：limits 与 observability

- 显式 limits 原生拒绝与默认资源限制 deviation；parent/code/entrypoint `min()` 执行归 P2；
- distinct in-flight Dynamic Worker accounting；
- system tail collector + user tails fan-out；
- logs/realtime tail attribution、sampling、redaction、metrics。

### DW5：qualification

- fixed Wrangler subprocess deploy/download/settings/delete；
- Cloudflare Dynamic Workers differential；
- cold/warm/evict/restart/concurrency/cancel；
- adversarial capability、ID、module、env、egress、tail 与 limit cases；
- 最终 compatibility/deviation/capability authority 同步。

## 15. 必测矩阵

| case | 预期 |
| --- | --- |
| Wrangler JSONC/TOML `worker_loaders` | metadata 只有 `{name,type:"worker_loader"}` |
| named env 未重复声明 | 按 Wrangler non-inheritable 规则不继承 |
| `load()` 两次同 code | 两个独立 unnamed worker identity |
| `get()` 同 namespace + ID | cache opportunity；callback 次数不能被测试写死为 1 |
| 同 ID 返回不同 code | caller 违规 fixture；平台不得暗换 ID |
| cache eviction / process restart | callback 可重跑，功能不依赖 global state |
| async callback reject then retry | 第一次 invocation 失败；后续不永久 poison |
| JS handle GC / chained temporary handle | 同步及异步 startup、actor class 等待、请求和流存续期间不 UAF；保留 #6553 强引用关系 |
| WorkerCode 未知字段 / 已知字段非法值 | 前者忽略，后者按 native 校验失败；module 零个/多个已知类型拒绝 |
| compiled WebAssembly.Module 两种输入 | 直接值与 wasm 字段均通过，复用编译结果；覆盖已有两种 module registry 回归 |
| env capability 来源与类型 | 静态 entrypoint、persistent RpcStub、受限制动态 entrypoint 分别验证，不放宽全部 transfer |
| Loader 委派越权 / 生命周期 | 跨 namespace、非法再委派与进程重启后的旧 capability 不可使用；有效在途引用保持安全 |
| JS/CJS/Python/text/data/json/Wasm | 类型与 fixed workerd 一致 |
| empty modules / missing main / unknown module object | startup fail closed |
| 64 MiB boundary ±1 | boundary 通过，+1 native rejection |
| env 1 MiB boundary ±1 | 与 fixed workerd estimate 行为一致 |
| omitted / null / redirected outbound | inherit / block / proxy 三态 |
| scoped KV/D1/R2 custom binding | child 只能访问被授予 prefix/operations |
| scoped Vectorize capability | query/read 与 mutation 权限分离；不能切换到未授权 index |
| scoped AI Search instance/namespace capability | instance allowlist 和 read/write 方法集生效；不能观察 provider credential 或内部 resource ID |
| scoped Markdown Conversion capability | 只开放已声明的 `toMarkdown()`/`supported()`，完整 Workers AI 仍不可达 |
| system namespace guessing | 永远不能命中/观察 system isolate |
| user tail + system collector | 都收到一次；child response 不等待 tail |
| streaming tails non-empty | Day 1 明确拒绝 |
| code/entrypoint/actor-class 显式 limits | P1 原生拒绝，包括空对象；省略时正常加载；P2 接管真实预算执行 |
| Worker 4 / DO 10 distinct children | 第 5 / 11 个按官方行为失败；同一 child 并发只计一个，结束后释放名额 |
| Script delete/recreate | 旧 namespace tombstone；新 script_id 不复用 |
| Version rollback | parent code/bindings/limits 回滚；public namespace identity 保持 Script scope |
| malicious ID/props/error | bounded、redacted，不泄漏 namespace/source/secret |

## 16. Definition of Done

本文只有同时满足以下条件才能归档：

- fixed Wrangler `worker_loaders` config、multipart、download/settings round-trip 通过；
- nested tenant Worker 获得正式固定的 fork workerd 原生 WorkerLoader，不是 facade；
- internal system loader 与 public loader namespace 有不可绕过的 domain separation；
- `load/get/WorkerStub/WorkerCode` 的 P1 声明子集通过 black-box 与 source-backed Gate；limits 偏差明确；
- modules/env size、compat、bindings、egress、tails 与 errors 对齐固定 Cloudflare contract；
- P5 的 Vectorize、AI Search 与 Markdown Conversion 只能通过 parent 显式传递的 scope-safe service capability
  到达 child，权限收窄、identity、redaction 与 negative reachability Gate 通过；
- Worker 4 / DO 10 distinct in-flight limit 真实执行；custom limits 原生拒绝，默认资源限制偏差已记录；
- named cache 有 bounded eviction/cleanup，cold/warm/restart 不改变功能正确性；
- Workers Logs/realtime tail 正确归属 ordinary parent 与 Dynamic child；
- `dispatch_namespaces` 及全部 Workers for Platforms surface 仍明确 unsupported；
- fork patch 范围可审查；不存在 source rewrite facade、双运行时 fallback 或 deployment-triggered 全 runtime restart；
- Cloudflare differential 已完成，或 credential gap 被拆为 active acceptance；
- compatibility matrix、deviation registry、capability manifest、docs links、focused tests、coverage 与最终单轮
  workspace Gate 全部同步通过。
