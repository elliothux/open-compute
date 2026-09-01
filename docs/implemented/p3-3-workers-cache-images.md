# P3.3：Workers Cache、Cache API 与 Images Day1 方案

状态：**Platform Go（声明的单节点支持面）**。核心实现、完整静态检查、覆盖率与最终三轮 Gate
均已通过，2026-08-30。Cloudflare differential/portable contract 由后续 P3.4 单独验收；不据此
宣称完整 Cloudflare 兼容，第三方应用 qualification 仍为“未评估”。

本阶段补齐通用 Cloudflare Workers 平台能力，而不是给某个框架增加专用后端。交付对象是普通
Worker 可以声明和使用的 Workers Cache、Cache API、Images binding 与 Version Metadata
binding；vinext 只是其中一组真实应用验收，不进入生产 schema、协议、缓存 key 或错误分支。
该应用 qualification 可选，不是 P3.3 Platform Go 的前置条件。

本文细化[总方案](../open-compute-workerd-platform.md)的 P3.3，依赖已经完成核心实现的
[Static Assets](p3-1-static-assets.md)和
[Service Binding](p3-2-service-bindings.md)。两项尚未完成的 direct Cloudflare qualification 由
[独立验收计划](../p3-assets-service-bindings-acceptance.md)负责；应用 qualification 独立出结论，
不能用本阶段的框架 smoke 代替任何平台证据。

### 当前实施证据

完成 revision 已经直接实现本文的 Day1 核心模型：per-Worker SQLite cache authority 与 S3 body、
automatic Workers Cache、显式 Cache API、`ctx.cache.purge()`、纯 Rust Images engine、Images
session/facade、Version Metadata、严格工具链配置、capability/deviation、低基数 metrics、operator
接口及 Worker 删除清理。实现不包含 vinext 命名或框架专用分支。

已实际通过的证据包括：

- `bun run typecheck`、78 个 JS/TS 测试、`bun run build` 与生成资产一致性；
- 正式 `workerd v1.20260826.1` 上的 `p3-cache-images` 产品 Gate：最新单轮报告
  `.temp/gate-run/20260830T115516-afa59b83/report.json` 为 1/1 PASS，并验证 automatic Range 命中、
  deployment 默认隔离与显式跨版本共享、cache 跨 workerd 重启仍命中、Images session generation
  隔离/清理、Version Metadata/Images 重启可用及 source/binding generation credential 轮换；
- `cargo fmt --all --check`、带 `--keep-going` 的 workspace Clippy、no-default-features、Rust 1.98
  MSRV、metadata、dependency-boundary、生成资产和 production hygiene 检查；Clippy 首轮一次收集全部
  reachable diagnostics，批量修复后只复跑一次验证；
- per-Worker cache DB 对完整 SQLite DDL/索引计算并校验 schema 指纹；Cache body 的 S3 写入/读取
  handoff 与 artifact GC 共用生命周期 reservation，相关并发回归、状态/TTL/tombstone 边界和
  Images orientation/animation/像素/输出限额回归均已通过；
- 修复后的 P0.2 真实运行时单轮回归，以及 runtime 取消/子进程清理聚焦回归；
- `./test/coverage.sh` 的 37 个目标全部通过，workspace Rust 行覆盖率为 90.10%，报告为
  `.temp/gate-run/20260830T130329-4856d74a/report.json` 与 `target/llvm-cov/summary.json`；
- 源码冻结后的 `OPEN_COMPUTE_GATE_ROUNDS=3 ./test/gate.py --workspace` 通过：第一轮完整 37 个目标，
  第二、三轮各 19 个审计登记时序目标，报告为
  `.temp/gate-run/20260830T132334-47703959/report.json`。

上述证据满足第 12 节中由 P3.3 拥有的实现与本地验收条件。失败报告仍保留在
`.temp/gate-run/failed/`。Cloudflare differential/portable contract 属于后续 P3.4 的平台资格结论；
在 P3.4 完成前，本阶段不扩大为“完整 Cloudflare 兼容”。

## 1. 结论与范围

Day1 采用一套平台级缓存存储与两种公开行为面：

```text
普通 HTTP / Service fetch / ctx.exports fetch
        │
        ▼
Workers Cache policy ───────────────┐
                                    │
tenant Worker ── caches.* ─────────┤── Cache authority ── SQLite metadata
             └─ ctx.cache.purge ───┘                  └── S3 response body

tenant Worker ── env.IMAGES ── Images facade ── bounded native transform engine
                                                   └── Response（不自动缓存）
```

三层必须分开：

- **Workers Cache** 是部署配置驱动的 HTTP response cache，在调用 Worker 前查找，在响应返回后
  按 HTTP 规则写入；覆盖 public fetch、Service Binding fetch 和 `ctx.exports` fetch，普通 RPC
  不参与。
- **Cache API** 是租户显式调用的 `caches.default`、`caches.open()`、`put/match/delete`。
  `caches.default` 与同一 Worker 的默认 response cache 共享逻辑存储；named cache 另有 namespace。
- **Images binding** 只对传入的原始图片字节执行声明的操作，不管理 hosted images，也不自动缓存
  输出。应用可显式使用前两种缓存能力。

Cloudflare 的全球 CDN、多数据中心局部缓存、tiered cache、计费、Cache Rules 管理面和 hosted
Images 服务不是目标。open-compute 是单节点实现：所谓 local cache 就是当前平台实例的 cache，
不伪造 colo、全球 purge 或传播延迟。

## 2. 契约基线与进入条件

### 2.1 公开契约

实现前把下列官方页面的 URL、对应 `cloudflare/cloudflare-docs` revision/path、最后更新时间和相关
事实摘要登记到 P3.4 的契约清单：

- [Workers Cache 配置](https://developers.cloudflare.com/workers/cache/configuration/)：
  `cache.enabled`、entrypoint override、`cross_version_cache`、HTTP cache policy、
  `ctx.cache.purge()` 与调用路径；
- [Cache API](https://developers.cloudflare.com/workers/runtime-apis/cache/)：
  `caches.default/open`、`put/match/delete`、条件请求、Range 和拒绝条件；
- [Cache 工作方式](https://developers.cloudflare.com/workers/reference/how-the-cache-works/)：
  default/named namespace 与单数据中心语义；
- [Images binding](https://developers.cloudflare.com/images/optimization/binding/)及
  [Images 限额](https://developers.cloudflare.com/images/get-started/limits/)：
  `.input/.info/.transform/.draw/.output/.response`、20 MiB 输入上限和格式；
- [Version Metadata binding](https://developers.cloudflare.com/workers/runtime-apis/bindings/version-metadata/)：
  `id`、`tag`、`timestamp`。

Cloudflare 文档会变化；某次网页勘察不能永久替代固定的 compatibility date、workerd pin 与回归
用例。WDL、Miniflare、workers-sdk 和 vinext 可以帮助定位实现和真实用法，但不覆盖官方契约。

### 2.2 当前代码事实

以下是方案编写时的源码观察，不是已通过的 Gate：

- 正式 runtime 仍是 `workerd v1.20260826.1`，来源由
  [`workerd.lock.json`](../../packages/runtime/workerd.lock.json)固定；
- `references/workerd/src/workerd/server/workerd.capnp` 的静态 Worker 支持
  `cacheApiOutbound`；
- 同一 pin 的 `WorkerLoader::WorkerCode` 仍有 `TODO(someday): cache API outbound?`，没有可声明的
  cache channel；
- `ExecutionContext.cache` 是 embedding hook，stock workerd 默认实现返回空或拒绝 purge；
- 当前 open-compute 没有 Cache facade、Images facade、response cache authority 或相应 capability；
- framework importer 会拒绝尚未支持的 `cache`、`images`、`version_metadata` 配置；已有 KV 可以
  承载任意应用的数据缓存，但“有 KV”不等于已有 Workers Cache。

因此第一工作包必须验证 runtime 接入，不先建表再假定全局 `caches` 能被动态 Worker 使用。

### 2.3 C0 Hard Gate

C0 使用已准备的正式 workerd binary 和最小动态 Worker，至少回答：

1. 静态 Worker 的 `cacheApiOutbound` wire contract 能否由一个普通 system Worker/loopback service
   实现，`put/match/delete/open` 的 request、response 和异常形状是什么；
2. `WorkerLoader` 加载的 Worker 是否继承任何可用 cache channel；预期失败也必须记录原始证据；
3. `globalThis.caches` 的 property descriptor 是否允许由受信任 bootstrap 安装完整 facade，且安装
   发生在所有 tenant module evaluation 前；直接引用、`globalThis.caches`、动态 import 与 warm
   isolate 必须一致；
4. object/function/class entrypoint 收到的 `ExecutionContext` 能否安全代理 `cache` 与 `exports`，
   保持 native receiver、`waitUntil`、RPC 和异步上下文；
5. public fetch、Service Binding fetch、self/跨 entrypoint 的 `ctx.exports.fetch()` 都能在 tenant
   handler 执行前进入同一 cache dispatcher，custom RPC 确实绕过；
6. stream 命中、miss、cancel、background revalidation 和 generation teardown 不遗留 capability、
   请求或 deployment pin。

可接受结论只有两种：

- **Go**：stock workerd 上存在通用、可回归的 facade/dispatcher 路径，不修改租户业务源码；
- **No-Go**：当前 pin/上游接口不能提供该行为。此时先向 upstream 补 WorkerLoader 能力或进行正式
  pin 升级评审；在此之前 P3.3 不能标成支持 Cache API/Workers Cache。

禁止用字符串替换 `caches`、只改 vinext bundle、给测试 fixture 注入全局变量、运行时 AST 改写、
Node sidecar 或进程内 Map 取得 Go。这些路径不能覆盖第三方模块、动态 import 和普通 Worker，且会
制造两套平台语义。

## 3. Day1 支持面

### 3.1 Workers Cache

首版承诺以下平台行为：

| 能力 | Day1 契约 |
| --- | --- |
| 配置 | top-level `cache.enabled`；entrypoint `cache.enabled` override；`cross_version_cache` |
| 调用面 | public/default fetch、具名 Service fetch、self/cross Worker Service fetch、`ctx.exports` fetch |
| 绕过 | custom RPC、非 HTTP 事件、非 GET/HEAD、明确不可缓存 request/response |
| policy | `Cloudflare-CDN-Cache-Control` > `CDN-Cache-Control` > `Cache-Control`；常用 freshness/SWR/SIE |
| 结果 | `CF-Cache-Status` 至少区分 `HIT/MISS/BYPASS/STALE/UPDATING/REVALIDATED/EXPIRED` |
| 变体 | 完整 URL、entrypoint、request method、响应 `Vary` 指定 header；`Vary: *` 不缓存 |
| purge | `ctx.cache.purge({tags})`、`{pathPrefixes}`、`{purgeEverything:true}` |
| version | 默认 deployment 隔离；`cross_version_cache=true` 时同 Worker/entrypoint 跨 deployment 共享 |

HTTP policy 不应由框架解释。平台实现 RFC 9111/5861 中本支持面的缓存规则，并为未支持或与
Cloudflare 有意不同的细节分配稳定 deviation ID。没有 `Cache-Control` 时 Cloudflare 的启发式
TTL、Cache Deception Armor 和完整状态码表范围较大；Day1 的安全默认是未明确可缓存就 BYPASS，
并以 `OC-CACHE-001` 披露，不猜 Cloudflare 套餐/zone 规则。若后续决定实现启发式，应先扩充
契约矩阵和 poisoning 测试，而不是悄悄改变默认值。

`Set-Cookie`、`Authorization`、`private`、`no-store`、`no-cache`、可缓存状态码、HEAD、Range、
压缩变体和 header precedence 分别有表驱动用例。`stale-while-revalidate` 与
`stale-if-error` 仅属于 Workers Cache；不能错误套到显式 Cache API，因为官方明确说明
`cache.put/match` 不支持这两个指令。

### 3.2 Cache API

Day1 支持：

```ts
const defaultCache = caches.default;
const named = await caches.open("rendered-pages");
await named.put(request, response);
const hit = await named.match(request, { ignoreMethod: false });
const deleted = await named.delete(request);
```

行为边界：

- key 接受 `Request | string`，string 按 URL 构造 Request；名称和 URL 先做长度、协议与 canonical
  校验；
- `put` 只接受 GET key，拒绝 206 和 `Vary: *`；response body 只消费一次，超限或取消不写入；
- `match` miss/expired 返回 `undefined`，不访问 origin；只支持 `ignoreMethod`，拒绝
  `ignoreSearch`/`ignoreVary`；
- `Range`、`If-None-Match`、`If-Modified-Since` 在命中副本上产生 206/304；非法或不可满足 Range
  使用明确的 416/完整响应规则并与固定契约测试一致；
- `delete` 原子返回是否删除了当时可见的 entry；同一逻辑 URL 的 variant 处理必须与契约矩阵
  一致；
- `Cache-Tag` 进入反向索引，不向最终 client response 泄露；
- 单节点上 `delete` 已是当前实例全部 local cache 的删除，不声称全球 purge。

官方上限 512 MiB/对象不适合 SMB 默认资源预算。Day1 使用 operator 可配置的较小上限，并在
capability/limits 输出与 `OC-CACHE-002` 中明确披露；不能广告官方额度。值由 C0/负载测试定，
本文不伪造尚未测量的数字。

### 3.3 Images binding

首版提供通用 `env.<binding>`，binding 名可配置，不要求叫 `IMAGES`：

| API | Day1 支持面 |
| --- | --- |
| `input(stream)` | raw `ReadableStream`；有界接收；按 magic bytes 识别，不信任 Content-Type |
| `info(stream)` | format、fileSize、width、height；损坏/炸弹输入稳定拒绝 |
| `transform(options)` | 有序多次调用；resize、fit、gravity、rotate、flip、background、blur 的固定子集 |
| `draw(image, options)` | 有界 overlay 数；位置、opacity、repeat=false、normal/over composite 子集 |
| `output(options)` | 必填 format；JPEG/PNG/WebP/AVIF；numeric quality；`anim:false` |
| `response()` | 正确 Content-Type、长度/stream、无内部 header/path/provider 信息 |

I0 必须把每个 option、输入/输出 codec、EXIF orientation、alpha、ICC/metadata、animated GIF/WebP
行为固定成 tracked matrix。未列出的 Cloudflare transform option 必须部署时/调用时明确拒绝，且不出
现在本项目类型声明中；不能忽略未知字段后返回看似成功的错误图片。

Hosted Images、direct upload、delivery URL、签名 URL、计费、视频、AI upscale、完整 animation
保留、任意 ICC 管理、`fetch(..., {cf:{image}})` 和 Cloudflare URL transform 服务不在 P3.3。
它们进入 `OC-IMAGES-001`，不由把 R2 bucket 命名成 hosted images 来伪装。普通应用仍可从 R2、
Assets、public fetch 或 request body 取得字节后交给 binding。

Image 输出**不自动缓存**，与官方 binding 一致。若部署同时开启 Workers Cache，应用返回的图片
Response 可以按通用 HTTP policy 缓存；cache key 由 HTTP 请求和响应变体决定，不由 Images 引擎
偷偷追加不可见规则。

### 3.4 Version Metadata

`version_metadata.binding` 生成只读对象：

```ts
interface WorkerVersionMetadata {
  readonly id: string;
  readonly tag: string;
  readonly timestamp: string;
}
```

`id` 直接使用 immutable deployment ID；`tag` 来自部署时有界、非 secret 的可选 tag，没有时为空
字符串；`timestamp` 是 deployment 创建时间的 canonical RFC 3339。rollback 恢复原对象，不能
生成新 ID 或时间。它是普通平台契约，不是 vinext 专用 warmup token。

## 4. 配置、descriptor 与部署冻结

工具链接受 Cloudflare 风格输入，再转换成当前唯一 deployment descriptor：

```json
{
  "cache": { "enabled": true, "cross_version_cache": false },
  "exports": {
    "default": { "type": "worker", "cache": { "enabled": true } },
    "Admin": { "type": "worker", "cache": { "enabled": false } }
  },
  "images": { "binding": "IMAGES" },
  "version_metadata": { "binding": "CF_VERSION_METADATA" }
}
```

实现规则：

1. importer 只识别已经实现的精确字段；未知字段 fail closed，不把整个 Wrangler JSON 原样存储；
2. entrypoint 名必须在真实 module exports 中存在并是可缓存的 Worker fetch entrypoint；DO、Workflow
   class、Queue/Cron handler 不可开启 response cache；
3. cache policy、Images/metadata binding 名进入 canonical descriptor 与 digest，ready 后不可变；
4. Images/metadata 与 vars、secrets、KV/D1/R2/DO/Queue/Workflow/Assets/Service 共用 binding 名空间；
5. Images 是平台内建 capability，不创建可由用户改指向的 resource ID；没有声明就不进入 env；
6. `cache.enabled=false` 或删除配置只改变新 deployment 的查写行为，不清理旧 entry；显式 purge/GC
   才删除；
7. framework importer 和普通项目 loader 调用同一 parser/descriptor；不得存在 `frameworkCache`、
   `vinextImages` 或隐式创建 `VINEXT_KV_CACHE` 的生产字段。

Data Cache 不属于新平台产品。若应用声明 KV binding（例如 vinext 的 `kvDataAdapter()`），继续走
已有通用 KV namespace/resource API，key、tag marker、TTL 与缓存值格式由应用 adapter 负责。

## 5. 缓存 identity 与状态机

### 5.1 隔离维度

cache identity 至少包含：

```text
account_id
worker_id
cache_surface       # automatic | cache-api-default | cache-api-named
entrypoint_name     # automatic only
version_scope       # deployment_id or shared
cache_name          # named Cache API only
canonical_url
method_class
vary_fingerprint
```

`version_scope` 规则：

- automatic Workers Cache 默认使用 deployment ID；promotion 后新版本 miss，rollback 回到旧
  deployment 的未过期 entry；
- `cross_version_cache=true` 使用固定 `shared` scope；所有启用 cache 的 deployment 明确接受共享；
- Cache API 是 Worker/namespace 级显式存储，不因 deployment 自动清空；若应用需要版本隔离，key
  自身加入版本，符合 Cloudflare 的显式 API 模型；
- 不把 hostname 当 tenant authority。相同 URL 在不同 account/Worker 下永远不共享。

部署、入口和 namespace 用内部 ID，租户不可从响应、key 或 purge 结果枚举。canonical URL 保留
query 顺序和重复参数，规范化 scheme/host/default port，去掉 fragment；不能排序 query 后误合并
语义不同的请求。

### 5.2 Automatic cache 状态

```text
absent ── request ──> render ── cacheable ──> fresh
  ▲                     │                        │
  │                     └─ bypass ──────────────┘
  │                                              │ TTL
  │                         ┌────────────────────┘
  │                         ▼
  └──── purge/expire ── stale ── SWR ──> refreshing ── commit fresh
                          │                    │
                          └─ SIE on error <────┘
```

每个 stale key 只有一个 refresh owner；其他请求按 policy 返回 stale 或等待同一个结果。owner 使用
随机 lease token 与 deadline，commit 要同时匹配 entry generation 和 token。platformd/workerd
crash 后 lease 到期可重取；迟到结果不能覆盖 purge、新 put 或较新 refresh。

`purge` 先在 SQLite 事务提高对应 generation/写 tombstone，再异步释放 body ref。这样 purge 返回
成功后，旧 in-flight refresh 即使完成也不能复活 entry。tag/path purge 作用于所有 variant；tag
比较按官方契约使用 case-insensitive canonical form，原始 tag 不进入日志 label。

### 5.3 Cache API 写入

显式 `put` 不提供 SWR lease：完整 body 上传并校验后，SQLite transaction 以 conditional generation
替换 metadata/ref；并发 put 最后一个成功 commit 的值可见。上传成功但 transaction 失败留下的
S3 object 是不可达 orphan，由 artifact GC grace 回收；transaction 成功后 response 断开属于
result-unknown，重试同一 put 是安全覆盖。

match 在 transaction 外流式读取 S3。读取前取得有界 runtime body pin；delete/purge 可以立即使
新读取 miss，但延迟释放已有 stream 的 object ref。stream 正常结束、cancel、client disconnect、
workerd generation 退出都必须释放 pin。

## 6. SQLite 与 S3 设计

### 6.1 control.sqlite

平台 current schema 直接增加部署冻结信息，不保留开发版旧字段或双读：

```sql
CREATE TABLE deployment_cache_policies (
  deployment_id       TEXT NOT NULL REFERENCES worker_deployments(id),
  entrypoint_name     TEXT NOT NULL,
  enabled             INTEGER NOT NULL CHECK(enabled IN (0, 1)),
  cross_version_cache INTEGER NOT NULL CHECK(cross_version_cache IN (0, 1)),
  PRIMARY KEY (deployment_id, entrypoint_name)
);

CREATE TABLE deployment_builtin_bindings (
  deployment_id TEXT NOT NULL REFERENCES worker_deployments(id),
  binding_name  TEXT NOT NULL,
  kind          TEXT NOT NULL CHECK(kind IN ('images', 'version_metadata')),
  PRIMARY KEY (deployment_id, binding_name),
  UNIQUE (deployment_id, kind)
);
```

这是逻辑 DDL，不是可直接追加的历史 migration。实现时按 AGENTS Day1 规则修改当前连续、校验和、
事务化 schema，并同步 build wiring、descriptor、capabilities、snapshot policy 和测试。不得自动重置
已有本地数据。

### 6.2 一 Worker 一 cache.sqlite

缓存不是 KV resource，不为每次 `caches.open(name)` 创建数据库。采用：

```text
.data/cache/<account-id>/<worker-id>/cache.sqlite
```

一 Worker 一库在单节点场景有三个好处：账户/Worker 删除和配额直接、一个高写入/损坏库不阻断
其他 Worker、named cache 不造成无限数据库文件。连接由有界 LRU manager 管理，WAL、busy timeout、
permissions、symlink/path containment 和 corruption fail-closed 复用 KV/D1 的成熟规则。

逻辑表：

| 表 | 权威字段与约束 |
| --- | --- |
| `cache_entries` | scope、namespace、key hash、canonical key、status/header、body ref/size、fresh/stale/error deadlines、generation、timestamps |
| `cache_variants` | entry identity、Vary header 名和值 fingerprint；唯一且有数量/字节上限 |
| `cache_tags` | canonical tag + entry ID；正反索引；不复制 response body |
| `cache_refresh_leases` | entry ID、random token、deadline、base generation；不保存进程 PID |
| `cache_tombstones` | purge scope/generation 与回收水位；有界 retention 后 compact |

header 保存 canonical binary/JSON encoding，保留重复 header 的合法语义，拒绝 hop-by-hop/internal
header。不能用 `serde_json::Value` 任意接受深层结构。body 不进 SQLite：复用 S3 ArtifactStore 的
content-addressed bytes 和本地 verified cache，另用 cache object kind/refcount 区分 immutable
deployment artifact。cache entry 是可回收引用，不把 cache body 加入部署 retention。

cache.sqlite 不进全平台权威备份：它是可丢失的 performance state，恢复后 miss 并重新填充。
完整 snapshot manifest 应记录“cache excluded”及 schema/配置身份，不能误写为已恢复 cache。
S3 中仍被 cache 引用的对象由正常 GC 管理；cache DB 丢失后 orphan grace 到期再回收。

### 6.3 存储故障语义

- metadata 损坏、digest 不符、跨 tenant ref：fail closed、entry quarantined/miss、内部告警，绝不返回
  未验证字节；
- S3 timeout/5xx：automatic cache lookup 按可配置 fail-open 进入 Worker，写入失败不改变用户
  Response；显式 Cache API 对契约允许的静默不存储与必须抛错的情况分别测试；
- SQLite busy：短有界重试；automatic 路径可 BYPASS；显式 `put/match/delete` 分别按 C1 固定的
  Cloudflare failure contract 返回，不把“未写入”“miss”“未删除”和 authority failure 混为一谈；
- 磁盘 hard watermark：停止新 cache 写，读取仍可用；不影响 control/scheduler authority；
- purge transaction 失败：返回 `success:false`/稳定错误，不先声称成功再 best effort 删除。

fail-open 只适用于 cache availability，不适用于身份、摘要、权限或协议错误。

## 7. Images 执行架构

### 7.1 facade 与传输

`env.IMAGES` 由 loaded-isolate facade 包装一个私有 `ImageTransport`。chain 在 tenant isolate 只累积
并验证有界 operation DAG；`.info()` 或 `.output()` 才执行。tenant 永远看不到 loopback URL、
session token、临时路径、S3 credential 或 image engine handle。

多个输入 stream 不能在 JS 中全部 `arrayBuffer()` 后一次发给 Rust。采用有界 session：

1. `begin` 创建 generation-scoped、短时、随机 session；
2. base/overlay 各自通过私有 loopback endpoint 流式上传到 `.temp/images/<session>/`，边收边计数、
   计算摘要和嗅探；
3. `finalize` 提交 canonical operation DAG、输入 digest/size，平台校验 session owner 与完整性；
4. native engine 在 bounded blocking pool 中执行，输出直接流给 workerd；
5. success、错误、cancel、deadline、workerd/platformd restart 都释放 session；失败现场只保留已脱敏
   manifest，不保留用户图片，除非显式诊断策略授权。

session 是在途资源，不写 control.sqlite，也不进入备份。所有 endpoint 只监听 loopback并要求
startup generation、binding token、deployment descriptor digest 和随机 session token。

### 7.2 native engine 选择

生产仍只发布一个 `platformd` 文件，不启动 libvips daemon、Node sharp 服务或远程图片 SaaS。I0
用正式 release profile 比较候选的静态/纯 Rust codec 组合，要求：

- Rust 1.98、目标平台、许可证与单文件静态链接可接受；
- required codec/operation matrix 真正可用，不靠运行时加载系统 dylib；
- decoder 能在分配前施加像素、帧、维度、metadata 和 decompression ratio 限制；
- cancellation/deadline 后不会释放并发槽而让不可取消 CPU 任务无限叠加；
- 输出确定到“像素/格式行为”级别；不同 codec 版本可能改变压缩字节时，测试不硬编码不稳定摘要。

只有一个 engine 实现进入生产。若候选不能覆盖已经承诺的 JPEG/PNG/WebP/AVIF 与 transform subset，
I0 是 No-Go；先调整明确的产品支持矩阵并登记 deviation，不能同时保留 fast/compat 两套引擎或
失败后返回原图。

### 7.3 资源预算与安全

配置项至少包括：input bytes、output bytes、像素总数、width/height、frames、overlay 数、operation
数、每 request wall deadline、platform 全局并发、account 并发和临时磁盘水位。默认值由 I0 压测
产生并进入 capability limits；除官方 20 MiB input ceiling 外，本文不把猜测数字写成已验证限制。

安全规则：

- magic bytes 与完整 decoder 双重验证；extension/MIME 只作提示；
- 先读有界 header 再分配完整 pixel buffer，拒绝 dimension/decompression bomb；
- 默认 strip EXIF/GPS/comment/ICC 等非必要 metadata，按固定 orientation 规则旋转；
- SVG 不是 raster input，拒绝而不是交给可联网 renderer；
- engine 不发起网络请求、不读任意路径；图片来源由 Worker 已有 R2/Assets/fetch 能力决定；
- 错误只暴露稳定分类：invalid input、unsupported format/option、limit、timeout、unavailable；
- 日志/metrics 不含 URL、object key、tag、图片 digest、用户 metadata 或原始 codec 错误字符串。

## 8. 代码所有权

按现有依赖方向组织，不建立 cache/images 微服务：

| 所有权 | 计划改动 |
| --- | --- |
| `crates/core` | Cache/Images budgets、稳定 error code、配置校验；不放存储实现 |
| `crates/artifacts` | cache body object kind/ref/stream/GC；复用 S3 provider 和 verified local cache |
| `crates/storage` | current control schema、per-Worker cache DB、连接 LRU、事务与 invariants |
| 新 `crates/images` 或明确现有 crate | codec/transform engine；只依赖 core，不依赖 service/workers |
| `crates/workers` | deployment cache policy、builtin binding descriptor、版本冻结 |
| `crates/service` | cache authority、purge、image session/engine composition、HTTP/control/metrics |
| `packages/runtime/src/cache/` | Cache facade、Workers Cache dispatcher、ctx proxy、transport |
| `packages/runtime/src/images/` | chain facade、strict DTO、private ImageTransport |
| `packages/toolchain` | 精确解析 cache/exports/images/version_metadata；普通与 framework output 共用 |
| `test/` | stock-workerd Hard/Product Gate、第三方应用 fixture 与故障矩阵 |

如果 image engine 能保持低层 sibling 依赖，可新建小 crate；否则放入 `artifacts` 会混淆不可变对象与
CPU transform 所有权，不建议。禁止把解析、SQLite 或 codec 逻辑堆进 loader host。

## 9. 工作包与依赖顺序

| 包 | 内容 | 退出条件 |
| --- | --- | --- |
| C0 | WorkerLoader Cache/ctx/dispatcher Hard Gate | stock workerd 通用路径 Go；否则停止 Cache 实现 |
| C1 | 固定契约矩阵、deviation 与限额字段 | capability schema/文档/test IDs 一一对应 |
| C2 | current descriptor/schema、per-Worker cache DB、S3 refs | clean init、损坏拒绝、删除/GC/重启 Gate |
| C3 | Cache API facade 与 wire transport | 普通 Worker 的 default/named put/match/delete 全矩阵 |
| C4 | automatic Workers Cache dispatcher | public/Service/ctx.exports、entrypoint override、version isolation |
| C5 | SWR/SIE、Vary、conditional/range、tag/path purge | 并发 lease、purge fence、crash matrix 通过 |
| I0 | Images engine/codec/limits spike | 单引擎、单文件、required matrix Go |
| I1 | Images facade/session/engine | input/info/transform/draw/output 与安全负向矩阵 |
| I2 | importer、Version Metadata、capabilities/operator metrics | 普通 Worker 配置 round-trip、rollback/restart 正确 |
| C6 | 官方示例与 portable contract qualification | 普通 Worker 示例、Cloudflare differential、无未解释差异 |
| A0 | 可选应用 qualification | 独立 application baseline；vinext 映射场景单独出结论；无专用生产分支 |
| X | P3.3 Aggregate Gate 与结果文档 | 相关静态检查、coverage、单轮/三轮政策完成 |

C0 与 I0 可以并行调查；C2 只能在 C0 结论确定后进入。Images 不依赖 Cache API；若执行 A0，
其图片 response caching 场景依赖 C4/C5。Version Metadata 应在 C4 前完成 descriptor，避免后补
版本身份。A0 不阻塞 P3.3 Platform Go。

## 10. 验收矩阵

### 10.1 Hard Gate

建议新增目标名，实际实现时才写入 `test/gate.py` 与 `test/gate_cases.py`；本文不是可执行命令：

- `p3-cache-hard`：静态/dynamic Worker cache channel、global facade、ctx proxy、Service/ctx.exports、
  stream/capability cleanup；
- `p3-images-hard`：codec/transform matrix、stream/session、limit/cancel/timeout、single-binary linkage；
- `p3-cache-images`：两者 aggregate，不重复登记同一个 case。

Hard Gate 使用 verified stock workerd、真实 system Worker、真实 loopback transport、真实 SQLite 和
SigV4 S3 fixture。Miniflare/mock/Node 能用于对照，不计产品 PASS。

### 10.2 Product Gate

Cache 至少覆盖：

- 两账户、两个 Worker、两个 deployment、default/named cache 同 URL 不串；
- deploy A → fill → promote B 默认 miss → rollback A hit；cross-version 则 A/B 共享且 purge 后全 miss；
- public、Service Binding、self Service、`ctx.exports` fetch 的 HIT/MISS；RPC 每次执行；
- status/header precedence、Vary 多 variant、Cache-Tag 消费、Set-Cookie/Auth/private/no-store；
- SWR 100 并发只有一个 refresh、SIE、refresh error/timeout、purge 与迟到 commit race；
- Cache API non-GET/206/Vary* 拒绝、Range/304、expired miss、delete boolean；
- S3 5xx/timeout/slow body/digest mismatch、SQLite busy/corruption/disk watermark；
- platformd/workerd SIGKILL 位于 upload、metadata commit、refresh claim/commit、stream drain 各边界；
- Worker 删除/recreate 不复用旧 cache path，跨账户不存在 side channel；
- metrics series 固定，不含 URL、tag、Worker ID、cache name。

Images 至少覆盖：

- 每种 committed input/output codec 的小图、alpha、orientation、损坏/truncated input；
- resize/fit/gravity/rotate/flip/background/blur 的像素级断言；多 step 顺序不可交换；
- overlay 位置/opacity、多个 input、重复/超限 option 拒绝；
- source 来自 request、R2、Assets 与 public fetch；engine 自身不联网；
- 20 MiB 边界、像素炸弹、帧/overlay/operation、全局/account 并发、deadline 和 cancel；
- workerd/platformd crash 后 session 清理、无临时文件/child/thread 泄漏；
- 相同图片在两账户无状态或诊断泄漏；输出经通用 Workers Cache 命中时不再执行 transform；
- binding 缺失/重名、rollback 的 Version Metadata、未声明 Worker 不获得 IMAGES。

### 10.3 可选第三方应用 qualification

vinext 固定 revision 的三个 adapter 分别映射到通用能力：

| vinext 用法 | 平台能力 | 不允许的实现捷径 |
| --- | --- | --- |
| `kvDataAdapter()` | 已有 KV binding | 隐式 vinext KV 表或 adapter 分支 |
| `cdnAdapter()` | Workers Cache + `ctx.cache.purge` + Version Metadata | vinext 路由硬编码、无效 purge |
| `imagesOptimizer()` | Images `input/transform/output/response` | 原图 passthrough、Node sharp sidecar |

vinext 场景证明 API 能承载真实框架，但不能替代普通 Worker contract suite。反过来，vinext 未使用
`caches.open()` 或 `.draw()` 也不能删除相应已承诺 API 的 Gate。

## 11. 可观测性、运维与清理

新增低基数指标：cache lookup/store/purge/refresh outcome、metadata/body bytes、active refresh、DB
connections、S3 latency bucket、image session/operation outcome、active transforms、input/output bytes、
limit rejection。label 只用固定 surface/operation/outcome/codec，不用 tenant 或内容身份。

operator 面至少提供按 account/Worker 查看 cache bytes/entries、purge Worker cache、执行 GC 和查看
Images capacity；管理调用走现有 control auth。不能提供任意 S3 key、cache body 下载或图片 session
文件浏览接口。Worker 删除 saga 先 fence 新调用、drain pins，再删除 cache DB 引用；物理 S3 bytes
按 grace GC。失败清理保留脱敏 manifest 到 `.temp`，不删除已有 Gate 证据。

## 12. P3.3 Exit Gate

只有同时满足以下条件才是 P3.3 Go：

1. C0/I0 两个 Hard Gate 对正式 pin 和单文件发行模型都是 Go；
2. 普通 Worker 的 Workers Cache、Cache API、Images、Version Metadata 公开矩阵全部通过；
3. public/Service/ctx.exports 三条 fetch 路径的缓存行为相同，RPC/事件正确绕过；
4. SQLite/S3 authority、tenant/version/entrypoint 隔离、purge fence、stream 生命周期和 crash recovery
   有真实进程证据；
5. Images required codec/operation/安全/资源矩阵通过，无原图 fallback、mock 或外部 sidecar；
6. capabilities/limits/deviations 与类型声明只广告实际支持面；
7. 官方普通 Worker 示例经正常 build/deploy/platformd/workerd 路径通过；portable Cloudflare
   differential 作为 P3.4 的资格验收，不反向阻塞本阶段已经声明和验证的单节点产品支持面；
8. 相关开发 Gate 单轮、源码冻结后的审查登记时序用例额外两轮，最终 workspace/coverage 按
   [测试规范](../references/testing.md)完成；
9. 报告保存输入摘要、正式 runtime 身份、逐 case/逐轮结果、故障点、资源清理和未通过项。

任何 Cache API 接口因 WorkerLoader 缺口未执行、任何 automatic fetch 路径被跳过、任何 Images
格式靠 passthrough，结论都是 No-Go 或明确缩小后的 Conditional Go，不能写“vinext 能跑所以
Cloudflare Cache/Images 已支持”。vinext 未运行时 Application verdict 是“未评估”，不降低满足以上
条件的 Platform verdict。
