# P3.4：Cloudflare 能力对齐、隔离与恢复 Day1 方案

状态（2026-09-01）：**Day1 全量 stable tenant API 实现与本地 conformance 已完成；外部资格为
Conditional Go**。固定 baseline/catalog、typed runner、portable fixtures、P3.1–P3.3 产品证据、隔离与
crash/recovery 已进入当前实现，inventory 为 2,097 个成员、`blocked=0`。Workers、Cache API、KV、D1、
R2、Durable Objects、Queues 的真实 Cloudflare differential 已通过并精确清理；Workflow 托管端运行因
Wrangler OAuth `10000` 拆分到[剩余验收](../acceptance/cloudflare-runtime-compatibility-acceptance.md)。最终 workspace
运行一个完整 round，802/802 cases 通过。2026-09-03 起仓库最终 Gate 统一为一轮，因此该证据满足当前
Gate 轮数政策；它不替代尚未执行的外部资格或正式发行操作。
P1 长时 soak 与正式发行演练继续由独立的[P1 剩余验收计划](../acceptance/p1-release-acceptance.md)跟踪。

P3.4 的目标不是实现 Next.js，也不是让某个 vinext revision 的全部 API 或测试无条件变绿；目标由
[Cloudflare Worker Runtime 全量兼容目标](cloudflare-runtime-compatibility.md)定义：让 open-compute
对固定 baseline 的 latest stable Workers runtime，以及 Workers、Durable Objects、Queues、
Workflows、R2、D1、KV 的完整 tenant API 具有可追溯契约、真实 stock workerd 证据、与 Cloudflare
的差异记录，以及单节点 self-host 场景的隔离和恢复保证。

vinext 是第三方应用检验手段之一。它能同时覆盖多环境构建、SSR/RSC、Static Assets、Service
Binding、KV、Workers Cache、Version Metadata 和 Images，但它自己的 Next.js 兼容缺口不属于
平台实现，vinext 未使用的 Cloudflare API 也不能因此从平台契约中消失。

本文细化[总方案](open-compute-workerd-platform.md)的 P3.4。进入最终验收前，P3.1、P3.2 与
[P3.3](p3-3-workers-cache-images.md)必须达到各自声明的 Go；它们的 Conditional Go/未运行项
中属于平台 contract 的部分不能在本阶段改名为“上游限制”。未选择的应用 qualification 保持
“未评估”，不混入 Platform verdict。

## 1. 对齐原则

### 1.1 真值优先级

出现冲突时按以下顺序判断：

1. 固定版本 `@cloudflare/workers-types` stable 入口，以及匹配 pin 的 upstream workerd generated types；
2. Cloudflare 官方 runtime/binding 文档和正式 pin 的源码、WPT/单元测试、真实 binary 行为；
3. 同一 portable fixture 在真实 Cloudflare Workers 上的冻结观察结果；
4. open-compute 已声明的单节点 deviation 与安全/资源边界；
5. workers-sdk、Miniflare、Wrangler、Vite plugin 等工具的集成行为；
6. vinext、Hono 或其他应用的使用方式和测试。

第三方项目不能覆盖前四层。若 vinext 为兼容 Next.js 做了额外转换，那是 vinext 行为；若它在
Cloudflare 上也失败或明确标成 partial，open-compute 不补一个框架专用平台分支。若同一能力在
Cloudflare 通过、open-compute 失败，才是平台差距。

### 1.2 “贴近 Cloudflare”的含义

本项目不声称 Cloudflare 全产品、管理 API 或全球 edge parity。目标范围内每项能力只能处于：

| 状态 | 含义 |
| --- | --- |
| `supported` | API shape、成功/失败、可见副作用与固定 single-latest runtime contract 行为通过 |
| `supported_with_deviation` | upstream stable API 完整可用，仅单节点拓扑差异有稳定 ID、文档和负向测试 |
| `unsupported` | 只用于全量目标文档明确列出的非目标，不得用于七项产品内的 stable API |
| `blocked` | 目标支持但缺正式输入、runtime primitive 或验收证据；不能发布成 supported |

不得使用“部分支持”而不列方法，也不得用产品名相同推导兼容。完整 upstream 类型可以包含未绑定
产品的 type name；实际 `Env`、runtime availability、`platformd capabilities --json`、文档和
conformance catalog 必须一致。目标 stable API 未实现时是 `blocked`，不能从类型中删除。

单节点差异可以接受：没有 edge placement、跨地域复制、全球 cache、D1 replica、DO 全球迁移、
Queue exactly-once 或 Cloudflare 管理/计费面。安全边界、事务原子性、不可变 deployment、明确错误、
本地 crash recovery 和租户隔离不能因 self-host 而降级。

### 1.3 平台目标与应用目标分离

P3.4 有两个独立结论：

- **Platform verdict**：声明的 Cloudflare 能力是否全部有契约与 Gate，是否只有公开 deviation；
- **Application verdict**：某个固定应用是否能在该支持面运行，例如 vinext 的选定 workload。

应用通过不能让 Platform Go，应用失败也不一定让 Platform No-Go。只有失败被映射到一个声明
supported 的平台契约，或目标 workload 明确属于本阶段承诺，才阻塞对应结论。

## 2. 固定输入与 P3.0 迁移

当前工作树已有 `test/conformance/baseline.json` 和 catalog 草案，但它们固定的是原方法子集、多个
tenant date 和 flag 组合，且尚未成为本目标的 committed authority，不能证明新的全量 single-latest
目标。P3.4-0 直接把该 authority 更新为全量目标基线；第三方应用基线仍另行登记，不作为 Platform
Go 的隐含前置条件。迁移后的正式 platform manifest 至少包含：

```json
{
  "schemaVersion": 1,
  "openComputeRevision": "<source-tree-digest>",
  "workerdLockSha256": "<sha256>",
  "effectiveCompatibilityDate": "<pinned-latest-date>",
  "requiredCompatibilityFlags": ["<platform-internal-flag>"],
  "cloudflareDocs": { "revision": "<commit>", "treeSha256": "<sha256>" },
  "workersTypes": {
    "source": "@cloudflare/workers-types",
    "version": "<pinned>",
    "packageSha256": "<sha256>",
    "astSha256": "<sha256>"
  },
  "workersSdk": { "revision": "<commit>", "lockSha256": "<sha256>" },
  "wrangler": { "version": "<pinned>" },
  "vitePlugin": { "version": "<pinned>" }
}
```

字段是目标 schema 示意，不表示当前文件已经迁移。权威结果仍由 tracked
`test/conformance/baseline.json` 保存；所有 sha 必须来自实际准备的 immutable 输入。不能写浮动
`latest`，也不能在生产启动或 Gate 中隐式 clone、安装、下载浏览器或更新 runtime。

应用验收在 `test/conformance/applications/<name>.json` 固定 repository revision、lock、构建器、浏览器
和选定 workload。没有该文件只表示对应 Application verdict 未评估，不会把已具备完整平台证据的
contract 降为 blocked。反过来，应用文件存在也不能替代平台 baseline 或 contract case。

tenant schema 和 deployment request 不包含 `compatibility_date` 或 `compatibility_flags`。上面的 date/flags
是平台构建期输入：date 固定为 pinned workerd 在本次 coordinated update 中支持的最新 stable 行为，
flags 只允许启用该行为所需的平台内部项。基线变更必须重新发现契约/用例，审查 API 增删、默认行为
和 expected result，更新 catalog、类型、deviation 和证据；不能只改 package version 后沿用旧 PASS。

## 3. 契约目录

### 3.1 机器可读 catalog

官方文档来源优先固定 `cloudflare/cloudflare-docs` 的精确 commit/path；live URL 和 observed date 用于
发现与展示，不单独充当可复现内容。若某页没有可固定的源码路径，catalog 保存相关字段/行为的
最小事实摘要及其 digest，不 vendoring 整页内容。

扩展现有 `test/conformance/catalog.json`，使 upstream stable AST 的每个目标成员都有稳定 ID：

```json
{
  "id": "cache.api.match.if-none-match",
  "product": "cache_api",
  "surface": "caches.default.match",
  "status": "supported",
  "runtimeContract": "<baseline-digest>",
  "sources": [
    { "kind": "cloudflare-doc", "url": "https://...", "revision": "<commit>", "path": "<path>", "sha256": "<digest>" },
    { "kind": "workerd-test", "path": "references/workerd/...", "revision": "<lock revision>" }
  ],
  "cases": ["cache-api/conditional/if-none-match"],
  "deviations": []
}
```

catalog 只保存结构化事实和 source identity，不复制大段第三方文档。配套
`docs/references/cloudflare-compatibility.md` 是人类可读矩阵；已有
[`p1-deviations.md`](../references/p1-deviations.md)继续拥有稳定 deviation 文本，不建立第二份
deviation truth。

静态检查强制：

- 每个 `supported` 方法至少一个正向、一个关键负向或拒绝 case；
- 每个 deviation ID 在 registry、文档和 case 中均被引用，且不存在孤儿；
- 每个 capability method 能回指 catalog，catalog 的 product/method 也存在 capability；
- upstream stable 类型 package 未被裁剪或重写；目标 API 缺实现时登记 `blocked`，非目标 binding 不进入
  generated `Env`；
- 配置 parser 不接受非目标产品字段，也不接受 tenant `compatibility_date`/`compatibility_flags`；
- 唯一 `effectiveCompatibilityDate`、内部 required flags、workerd、workers-types 与实际测试基线一致；
- case/source ID 唯一，删除或改名需要显式 baseline diff，不能用数量变化掩盖遗漏。

### 3.2 全量目标清单

P3.4 以 upstream stable 类型 AST 发现目标成员，不以当前 registry 的方法子集反推范围：

| 领域 | 必须覆盖的 tenant contract |
| --- | --- |
| Workers runtime | stable globals、Web APIs、modules/handlers、fetch、Request/Response/Streams、WebSocket、Crypto、HTMLRewriter、Cache API、scheduled、TCP sockets、RPC、ExecutionContext，以及匹配最新 date 的默认 Node.js surface |
| KV | 完整 `KVNamespace` interface、全部 overload、metadata、stream、list 与 cache-status shape |
| R2 | 完整 Worker R2 binding/object/body/multipart/options/checksum/SSE-C/storage-class surface |
| D1 | 完整 database/session/prepared-statement/result/meta surface，包括 bookmark 和单机 session 顺序语义 |
| Durable Objects | 完整 namespace/ID/stub/RPC/state/storage/SQL/sync KV/transaction/alarm/hibernatable WebSocket surface |
| Queues | 完整 producer/push-consumer/message/batch/ack/retry/metrics/delay/content-type surface，包括 `v8` |
| Workflows | 完整 binding/instance/batch/delete/lifecycle/step/context/parallel/event/restart-from-step/rollback surface |

Static Assets、Service Binding、Cache API、scheduled handler 和 Version Metadata 中属于 stable Workers
tenant API 的部分随 Workers runtime 验收。open-compute 自己的 deployment/promote/rollback/route 是
本地管理面，不以 Cloudflare 管理 API parity 验收。Images 等现有相邻能力继续按自身 capability 回归，
但不因已经实现而扩大本文七项产品范围，也不能替代目标 contract。

Wrangler 的账号/资源管理命令、Cloudflare `/client/v4`、zone/DNS/WAF/CDN rules，以及 Analytics
Engine、AI、Browser、Vectorize、Hyperdrive、mTLS、Rate Limiting、Workers for Platforms 等非目标
产品不因 workers-types 中出现 type name 就进入 Day1。它们不能进入 generated `Env`，对应配置必须
fail closed；但不得裁剪上游类型 package 来伪造边界。

### 3.3 single-latest runtime contract

open-compute 不向 tenant 提供 compatibility date 或 flag 选择。所有 deployment 使用 baseline 中唯一的
`effectiveCompatibilityDate` 和平台内部 required flags；loader、facade 和各产品不得再按 tenant date
维护行为分支。

每次更新先选择当时最新 stable `@cloudflare/workers-types`，再固定能提供同一 surface 的 workerd
release、最新非未来 stable date、必要内部 flags、workers-sdk 和文档 revision。该元组必须经过：

1. upstream types digest、AST 和 compile fixture；
2. workerd config 编译、native runtime probe 与 WorkerLoader 传递；
3. 受影响 facade、产品、failure/restart 和 differential case；
4. capability/catalog/generated `Env` 双向完整性检查；
5. tenant date/flag 输入的明确拒绝测试。

生产启动不联网计算 latest。完成新的 coordinated update 后直接替换旧 contract，不保留旧 date range、
历史 flag 组合、双路径或 open-compute 私有兼容分支。

## 4. Portable conformance fixture

### 4.1 一份 Worker 源码、两个目标

高风险契约使用同一 fixture 部署到 open-compute 和真实 Cloudflare Workers：

```text
test/conformance/fixtures/<product>/<case>/
├── src/                 # 完全相同的 Worker 业务源码
├── contract.json        # 输入、可观察输出、normalization、资源声明
└── assets/              # 固定字节/图片/模块 fixture

               ┌── open-compute adapter ── public deploy API ── platformd/workerd
fixture build ─┤
               └── Cloudflare adapter ───── pinned Wrangler ─── Cloudflare Workers
```

两个 adapter 只能改变 account/resource ID、endpoint、hostname、构建/部署连接和清理流程；不能改变
Worker 源码、handler 分支、Cache-Control、断言或资源内容。binding 名相同，资源由 manifest 显式
映射。若某项只能通过 `if (OPEN_COMPUTE)` 运行，它不是有效 differential case。

结果 normalization 只处理 contract 明确允许的不稳定值，例如 request ID、时间精度、随机 UUID、
Cloudflare 专属观测 header。不能删除 status、body、错误类别、cache outcome、调用次数或可见副作用
来制造相等。

### 4.2 Remote differential 的授权边界

部署到 Cloudflare 是外部写入、可能计费并需要 credential，不属于普通开发 Gate。只有显式选择
`p3-cf-diff` 且提供预置 account 与 Wrangler OAuth/token credential 才执行；runner 在 mutation 前输出 source revision、目标
account、资源前缀、预计 fixture 数和清理计划，并遵循操作授权。

credential 只由 Wrangler 或环境/文件引用读取，不写报告、argv、源码或 artifact。每轮使用唯一、有界前缀；finally
删除 Worker、KV/R2/D1/Queue 等本轮资源并二次枚举。清理失败使 qualification 失败并输出不含
credential 的资源清单，不能把孤儿留到“以后自动处理”。

当前 Cache API fixture 只创建随机 `oc-p34-*` workers.dev Worker；KV fixture 另在两个 provider 各创建
一个同前缀唯一 namespace，D1 fixture 在两个 provider 各创建两个同前缀唯一数据库。三者都不配置
Service/R2/Queue 或其它共享资源；删除使用精确名称/ID 且禁止 `--force`，再枚举确认 absent。
open-compute deployment retention 只在本次 qualification 的专用
test-support 实例上通过显式确认的 generation 轮换释放。这项安全边界不授权 runner 修改或清理账号中的
其他服务。

Cloudflare observation 是某日、某账号、某 effective compatibility date 的证据，不是永久真值。
结果以 source/baseline digest 冻结；官方契约后来变化时通过 coordinated update 重新运行，生产
runtime 不能自动追随网络上的 latest。

### 4.3 已运行的 Cache API differential（2026-08-30）

显式运行 `p3-cf-diff` 的现有报告为
`.temp/gate-run/20260830T203754-3e933947/report.json`，冻结源码摘要为
`0ac241b80a1bf4874c00925825f69e292f9dde801ffc85b6ad0ea34bcdb6943f`，baseline 摘要为
`9cc0058c58cc085f8476d226d1b9b3dd520b86cf0aed656142615a9a1ac6ff5d`。唯一 case
`cache-api/portable/cache-hit` 在账号别名 `personal-cf-account` 上使用 Worker
`oc-p34-mtfsotc3-3600aaa9-0`，Cloudflare 与 open-compute 的三项观察完全相同：

1. `/reset`：`200 {"reset":true}`；
2. 首次 `/probe`：`200 {"cache":"MISS","body":"portable-cache-v1"}`；
3. 第二次 `/probe`：`200 {"cache":"HIT","body":"portable-cache-v1"}`。

报告中的 Cloudflare cleanup 与 open-compute cleanup 都是 `deleted=true,status=absent`；本地 cleanup
另记录 generation restart。随后用 Wrangler 对该精确名称执行独立只读 `deployments list`，返回
Cloudflare `10007`（Worker 不存在）。这证明该随机 Worker 已清理，不证明账号其他服务曾被枚举或
变更。本节记录已经完成的冻结运行，不把后续文档维护表述成重新执行。

### 4.4 已运行的 KV differential（2026-09-01）

最终一轮 `p3-cf-diff` 中，`kv/portable/namespace` 使用 Worker
`oc-p34-mthwfgbw-c7f66ce5-1` 和 KV namespace `oc-p34-mthwfgbw-c7f66ce5-1-kv-0`；fixture
SHA-256 为 `ff48e7dddb49869466d68a63a07eb092bf1804f2f773d3771773963cb9d78e7a`。同一
TypeScript source 在真实 Cloudflare 与 open-compute 上依次完成 reset、seed、bulk/metadata/list read、
25 项 error/conversion/limit 观察、混合 string/byte stream put 和 delete-missing，所有 status 与 canonical
JSON 完全一致。其中固定了 bulk metadata 不含 `cacheStatus`、single/list 的可空 `cacheStatus`、Promise
rejection 时序、TTL 优先级、未配对 surrogate 的 URL decode 错误，以及 25 MiB/1 KiB/100-key 等边界。

Cloudflare Worker/KV 与 open-compute Worker/route/KV 的 cleanup 均为 `deleted=true,status=absent`；每个
资源只按本轮精确名称/ID 处理。同轮 Cache API case 也通过，Gate 共 2/2 cases passed。该结果关闭 KV
的 remote differential 缺口，不证明其它产品已完成。

### 4.5 已运行的 D1 differential（2026-09-01）

冻结报告 `.temp/gate-run/20260901T083041-bdff6a02/report.json` 以同一源码运行 Cache API、D1、KV
三个 portable case，3/3 通过。D1 fixture `d1/portable/database` 的 SHA-256 为
`8763456ab0c8670cf48ebb778f3ca6aa823452c62d01880e79d6ad77314acedd`，Worker 为
`oc-p34-mthxlapl-90f90526-1`。两端观察逐字段一致，覆盖 result/meta、跨 binding statement 的 receiving-
database 语义、`all/run/first/raw/batch/exec`、duplicate column、bind 转换、错误类别/Promise 时序、
session bookmark、batch rollback 和非 alpha `dump()` 拒绝。

Cloudflare cleanup 按 Worker 名和两个 database ID
`28ad8100-17de-409d-8d2f-e62fc2c0e7ff`、`3ad8d7a8-1841-48fd-b7eb-e3253f031cfe`
完成并记录 `deleted=true,status=absent`；open-compute Worker/route/两个 D1 也全部 absent。Gate 后的独立
只读 Wrangler 复查得到三个精确 Worker 均为 `10007`，本轮前缀/ID 过滤后的 D1 与 KV inventory 均为
`[]`。这关闭 D1 的 remote differential 缺口，不代表 R2、DO、Queues、Workflows 或其余 Workers 面已完成。

### 4.6 本轮外部资格结论

本地/CI mandatory suite 不依赖 Cloudflare 账号，使用官方契约、正式 workerd 与真实平台产品 Gate；
本轮已给出 local contract implementation completion。Workers、Cache API、KV、D1、R2、Durable Objects
和 Queues 的同源 remote differential 已逐字段通过。Workflow fixture 已实现并通过 open-compute 本地路径，
但 Cloudflare Workflow inventory preflight 返回 OAuth `10000`，没有创建 Workflow 或 Worker。因此本文
结论为带明确未覆盖范围的 Conditional Go，不能写“全部产品均与 Cloudflare 托管端实测完全一致”。

## 5. 分层测试模型

| 层 | 目的 | 运行环境 | 能否替代下一层 |
| --- | --- | --- | --- |
| L0 catalog/static | registry、types、配置、source、case 完整性 | 无 runtime | 否 |
| L1 facade/unit | strict DTO、canonicalization、错误、状态机纯逻辑 | Bun/Rust unit | 否 |
| L2 runtime hard | WorkerLoader/native primitive、RPC/stream/output gate | verified stock workerd | 否 |
| L3 product | public API、真实 SQLite/S3/process、restart | platformd → stock workerd | 否 |
| L4 differential | 同一 fixture 与真实 Cloudflare 对比 | 两个真实目标 | 否 |
| L5 application | 第三方应用组合能力 | 正常 build/deploy/browser | 否 |
| L6 isolation/recovery | 攻击、故障、crash、cleanup、rollback | 多账户/进程/真实 store | 最终补充 |

Miniflare 只可用于 upstream 工具行为对照；它不证明 platformd 的 authority、SQLite/S3、进程监督
或隔离。一个 browser E2E 成功也不能替代 Cache API、Queue replay 或删除 fence 的 product Gate。

## 6. vinext 的正确角色

### 6.1 固定 workload，不是平台规格

继续使用固定源码 revision
[`5d0b53088c689b75d63672eab6ff66434afa5b3b`](https://github.com/cloudflare/vinext/tree/5d0b53088c689b75d63672eab6ff66434afa5b3b)，
但用途改为组合验收。完整输入仍需固定 lock、React/Vite/RSC plugin、浏览器和构建工具。

选取 workload 应覆盖：

- App/Pages Router 的 production build、Static Assets、SSR/streaming、CSR hydration、RSC、Actions；
- SSG/ISR 与应用自己的 KV Data Cache；
- Workers Cache 的 response HIT/STALE/revalidation/tag purge 与 Version Metadata；
- Images binding 的 width/format/quality 路径；
- Service Binding/self binding、promotion/rollback、cold/warm/restart；
- server-only secret 不进入 client asset，HTML/Flight/cache variant 不串。

这些 workload 证明组合可用，但平台只实现它们映射到的 Cloudflare API。vinext 的
`kvDataAdapter()` 值格式/tag 逻辑、CDN adapter 的 header 转换、Next.js cache key、PPR shell、
Server Action 编码都留在 vinext，不下沉到 platformd。

### 6.2 失败分类

| 对照结果 | 分类与动作 |
| --- | --- |
| Cloudflare pass，open-compute fail，且映射到 supported contract | 平台回归，P3.4 No-Go |
| 两边都 fail，vinext baseline 也 fail/标为 partial | vinext/upstream 限制；记录，不修平台 |
| Cloudflare pass，open-compute fail，但能力明确 unsupported | application 不兼容；保持 unsupported 或另立范围决策 |
| open-compute pass，Cloudflare fail | 可能是平台扩展/fixture 问题；不据此宣传更兼容，先调查 |
| 原版 Next.js 要求，但固定 vinext 未实现 | 不属于 P3 平台 Gate |
| vinext toolchain/dev/HMR 失败，production artifact 行为正常 | 工具集成结论；不冒充 runtime 失败或通过 |

可以发现/运行完整 vinext suite 作为调查证据，但“全部上游启用测试通过”不再是 Platform Go 的
定义。Gate 清单只包含已映射的目标 workload；每个 inclusion/exclusion 记录原因，不能临时删掉
平台失败的 case。

### 6.3 不允许的适配

- 在 loader、cache、assets、services、images 中判断 vinext module/path/header 名；
- importer 改写 framework 业务代码或删除 assertion；
- 为 `VINEXT_KV_CACHE`、`IMAGES`、`CF_VERSION_METADATA` 建特殊物理资源；
- 缺 binding 时回退内存、原图、Node server 或关闭 ISR/PPR；
- 用普通 HTML 200 代替 hydration/RSC/stream/cache 行为。

允许的只有宿主 adapter：固定构建命令、读取正式产物描述、创建通用资源、部署、设置 base URL、
浏览器连接和结果收集。

## 7. 平台隔离矩阵

P3.4 在各产品既有 Gate 之上补跨产品/跨部署矩阵：

| 维度 | 必须证明 |
| --- | --- |
| account | 同名 Worker/resource/binding/cache key/route 在两账户完全隔离；错误无存在性 oracle |
| Worker | Service/KV/D1/R2/Queue/Workflow/Cache/Images capability 只来自冻结 descriptor |
| deployment | vars/secrets/assets/code/cache policy/version metadata 一次请求固定；promote/rollback 原子 |
| entrypoint | default/named/DO/Workflow/event capability 不串；Cache per-entrypoint policy 正确 |
| client/server | secret、internal token、S3 prefix、SQLite path、server module 不进入 client bundle/response |
| cache | account/Worker/version/default/named/variant/tag/prefix 无交叉命中或 purge |
| lifecycle | 被 ready/inactive deployment、Service、Workflow、stream/capability 引用时不能提前删除 |
| metrics/error | label、status、timing和错误 body 不泄漏 tenant identity/存在性/内部拓扑 |

测试同时启动两个账户、每个至少两个 Worker 与两个 deployment，不用串行清空状态模拟隔离。每个
目标拥有独立 data-dir、S3 prefix、端口和 runtime generation，除非 case 本身验证共享同一平台
实例的 tenant isolation。

## 8. 故障与恢复矩阵

### 8.1 故障注入点

保留 P0–P2 已有 claim/commit/output-gate 故障，新增：

- artifact/assets/cache/image upload 的 bytes 完成前、S3 commit 后、SQLite ref commit 前后；
- deployment ready/promote/rollback 与 runtime snapshot publish 的每个持久边界；
- Service target resolve/pin 后、stream/capability/waitUntil 未 drain；
- Cache lookup、refresh claim、stale serve、new response store、purge generation、body ref release；
- Images session begin/upload/finalize、decode/transform/encode、response stream cancel；
- public listener response 已提交但 mutation result 未被 client 看见；
- workerd unexpected exit、platformd SIGKILL、S3 timeout/5xx/corrupt body、SQLite busy/corrupt/WAL
  recovery、磁盘 soft/hard watermark。

每个故障点必须定义 authority、允许的重复、result-known/result-unknown、restart 后 reconcile、迟到
token fence 和最终资源集合。不能只断言“进程重启成功”。

### 8.2 恢复不变量

1. ready deployment 与 artifact/assets/descriptor 永远匹配，active 指针不指向半成品；
2. Queue 不丢 committed message，Workflow 不重跑 committed step，未知外部副作用仍按已声明
   at-least-once；
3. DO/Workflow/Service/stream 的 stale generation 不提交或保留旧 deployment；
4. Cache purge 后旧 refresh 不复活；cache 损坏最多造成安全 miss，不能返回错租户字节；
5. Images 在途 session crash 后可回收，不生成被平台误当成功的半张图；
6. S3 object 只有在所有 deployment/resource/cache/runtime ref 和 grace 都释放后才 GC；
7. cleanup 不删除历史失败 evidence，不把生产数据目录当临时目录。

## 9. Conformance runner 与结果模型

### 9.1 一个入口

继续只有 `test/gate.py` 一个本地 Gate 调度入口。现有实现以 Cargo executable 为主；P3.4 扩展为
typed target，而不是让 shell script、Playwright 和 Rust aggregate 各自再调一套 runner：

```text
TargetKind = cargo-test | bun-test | browser | cloudflare-differential
```

每个 target 使用结构化 executable/argv/env allowlist、timeout、resource class、discovery command、
case inventory 和 cleanup owner，不接受任意 shell 字符串。`--list` 不构建、不联网、不启动 browser/
workerd、不创建 Cloudflare 资源。平台依赖按 platform baseline 显式预置；browser 与框架依赖只按
被选择的 application manifest 准备。

本地 `--workspace` 包含 mandatory L0–L3/L6；L5 application 只在显式选择对应 application
manifest/target 时运行。外部写入的 `p3-cf-diff` 也只在显式 qualification 选择中运行，不可被普通
workspace 暗中触发。它的结果摘要可作为 final report 输入，但不能伪造成本地 Gate case。

### 9.2 用例发现与映射

case 数量来自各原生 runner 的 list/discovery，不硬编码。每个 case ID 反向关联 catalog contract，
且只能有一个结果 owner。分片合并拒绝 duplicate、missing、unknown 和只执行零项。

application inventory 记录 fixture、mode、browser、build/deploy target、contract IDs。vinext 上游
unit/toolchain tests 与平台 production workload 分列；Node/Miniflare PASS 不能填入 platform result。
上游 skip/fixme/todo 可作为应用背景，但不计平台 PASS，也不会自动变成 platform exclusion。

### 9.3 report

`.temp/gate-run/<run-id>/report.json` 继续是调度总报告，并引用：

| 文件 | 内容 |
| --- | --- |
| `contract-report.json` | baseline/catalog digest、每个 contract 状态、case、deviation、L0–L3/L6 结果 |
| `diff-report.json` | Cloudflare/open-compute observation、normalization、差异、资源/清理；仅显式运行时存在 |
| `application-report.json` | 仅应用 qualification 被选择时存在；记录 workload、build/deploy/browser、失败分类与 contract 映射 |
| `artifacts/` | 逐 case 脱敏 stdout/stderr、browser trace/screenshot、crash marker、cleanup inventory |

报告列出 discovered/executed/passed/failed/unsupported/blocked，不把 unsupported 或未运行放进
pass denominator 后称 100%。记录所有原始 attempt；上游 runner 自带 retry 时保留每次结果，不能
只展示最后一次绿灯。自动 retry-to-green 禁止。

## 10. Gate 分组与重复策略

建议最终目标，名称在实现前不视为已有命令：

| target | 范围 |
| --- | --- |
| `p3-contract` | catalog/capability/types/config/source 完整性 |
| `p3-assets` | 已有 P3.1 product matrix |
| `p3-services` | 已有 P3.2 hard/product matrix及补齐事件源/crash 项 |
| `p3-cache-images` | P3.3 hard/product matrix |
| `p3-apps` | 显式选择的 third-party workload build/deploy/browser；不属于 Platform Go 默认集合 |
| `p3-isolation` | 两账户/Worker/deployment/entrypoint/secret/cache 组合矩阵 |
| `p3-recovery` | S3/SQLite/process/crash/reconcile/cleanup 矩阵 |
| `p3-cf-diff` | 显式受控真实 Cloudflare differential；不属于默认 workspace |
| `p3` | mandatory 平台本地目标并集，不包含 `p3-apps`/`p3-cf-diff`，不重复运行子目标 |

确定性 catalog、固定输入、协议拒绝和显式 fault marker case 登记为 ONCE。真实并发、stream cancel、
进程退出、deadline、cache refresh race、browser timing 与清理登记为 TIMING。最终仍是完整确定性
一轮、仅 TIMING fresh-process 补两轮；不把所有浏览器/上游测试机械跑三遍。

case 分类只由 [`test/gate_cases.py`](../../test/gate_cases.py)拥有。新增 typed runner 后，registry 也必须
覆盖非 Cargo case，新增/删除/重名未审查时在执行前失败。并行只用于已证明 data-dir/S3 prefix/
port/browser profile 隔离的目标；resource-heavy browser/images/recovery 保守独占或固定小并发。

## 11. 工作包

| 包 | 内容 | 完成判据 |
| --- | --- | --- |
| P3.4-0 | 固定平台契约与工具链 baseline | tracked manifest、全部平台 digest、无浮动输入 |
| P3.4-1 | upstream types、catalog、capability/config/deviation 审计 | L0 AST/compile 与双向完整性检查通过；目标缺口为 blocked，非目标 fail closed |
| P3.4-2 | typed runner 与 portable fixture harness | offline list、两个 deploy adapter、结果/清理模型通过 |
| P3.4-3 | 补齐 P3.1/P3.2/P3.3 残项 | 各阶段独立 Go，不靠应用 smoke |
| P3.4-4 | 全量目标 contract/product 回归 | 全部目标 stable API 有真实 workerd/platform evidence，blocked 为零 |
| P3.4-5 | vinext 等可选应用 qualification | 独立 application manifest；正常 build/deploy/browser；失败按 contract 分类 |
| P3.4-6 | 两账户隔离与故障恢复 | L6 矩阵、resource/pin/process/temp cleanup 通过 |
| P3.4-7 | Cloudflare differential qualification | 冻结 fixture、受控账号、无未解释差异/孤儿资源 |
| P3.4-8 | P3 Exit 与报告/归档 | 静态检查、coverage、最终轮次、verdict 与限制完整 |

P3.4-0/1/2/3/4/6/8 已按新的 upstream AST 和 single-latest contract 完成；P3.4-5 是可选应用
qualification。P3.4-7 的七项 hosted fixture 已完成，Workflow hosted fixture 因外部 credential 条件单独
保持 active，不重新打开已经完成的本地 contract/product 实现。

## 12. P3 Exit

### 12.1 Platform Go

只有同时满足以下条件才能宣布“open-compute 在声明范围内贴近 Cloudflare Workers”：

1. baseline/catalog 固定，upstream stable types 的目标 AST 与 workerd、唯一 effective date、内部 flags
   均有 source、digest、case 和实际结果；
2. upstream 类型、capability JSON、generated `Env`、配置 parser、descriptor、文档与测试支持面完全一致；
3. 所有 `supported` case 通过；所有 deviation 有稳定 ID 和回归；unsupported 输入明确拒绝；blocked
   为零；
4. P3.1 Static Assets、P3.2 Service Binding、P3.3 Cache/Images 各自完成未决真实平台 Gate；
5. P0–P2 相关产品回归、跨产品两账户隔离、secret/client boundary、immutable promotion/rollback、
   crash/recovery/cleanup 全部通过；
6. portable high-risk fixture 在真实 Cloudflare differential 中没有未解释的“CF pass / OC fail”；
7. 完整 Rust/TS/static/dependency/MSRV/production 检查通过，Rust line coverage 不低于 90.00%；
8. 正式 release qualification 按[测试规范](../references/testing.md)执行一个完整 round；本实现 goal 的
   单轮报告保留原始结果且无 retry-to-green，符合当前 Gate 轮数政策；
9. 没有遗留 workerd/platformd/browser、listener、Cloudflare fixture、S3 prefix、image session、temp
   file 或未释放 pin；
10. P1 的单文件/离线/跨目标 release qualification 仍按其 active 文档完成；P3 测试不能代替正式
    发行包装证据。

结论文字应类似：

> open-compute 对 baseline/catalog `<digest>` 固定的 latest stable Workers runtime，以及 Durable
> Objects、Queues、Workflows、R2、D1、KV tenant API 达到 Go；单节点差异见 `OC-*` 清单。结果不包含
> Cloudflare 管理 API、其他产品或全球 edge 基础设施。

### 12.2 Application Go

vinext 的结论单独写：

> 固定 vinext revision 的选定 Cloudflare workload 在 open-compute 正常 build/deploy/browser 路径
> 通过；覆盖的 contract IDs 见 application report。vinext/Next.js 未实现功能和未选择的上游测试
> 不构成平台能力声明。

如果 vinext workload 未完成，Platform verdict 仍按契约证据判定，但不能声称该应用已兼容。如果
Platform contract 未完成，即使 vinext workload 全绿也只能是应用 smoke，不能给 Platform Go。

### 12.3 归档

本方案已与[完成报告](cloudflare-runtime-compatibility-results.md)归档到 `docs/implemented/`。报告记录
revision、dirty source digest、workerd lock、baseline/catalog、Cloudflare observation、工具链、
target/case/round、coverage、清理与限制；本方案本身不作为完成证据。
