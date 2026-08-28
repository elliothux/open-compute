# SQLite-only workerd 平台方案

> 状态：实施设计；2026-08-28 更新交付目标。Next.js/vinext 尚未完成平台验收。
>
> 当前目标是在 open-compute 上跑通 Cloudflare 官方 Next.js 指南采用的 vinext 路径，
> 与固定版本 vinext 的行为和完整用例集对齐，包括 SSR、CSR、RSC、PPR 等已实现特性。
> 不要求完整兼容官方 Next.js，也不要求补齐 vinext 上游尚未实现的 Next.js 能力。
> 本文只约束本仓库，不改变父项目边界；已有 P0–P2 记录不构成 vinext 已通过的证据。

## 1. 结论

平台继续面向单机、SMB self-deploy 场景；下一阶段以完整 vinext 应用和测试矩阵作为交付标准，
不能仅以基础 Worker 或一个 SSR demo 能返回 HTML 判断完成。

推荐的核心组合是：

```text
workerd + workerLoader
+ Rust platform daemon
+ SQLite
+ external S3-compatible storage
```

对外尽量还原常用 Cloudflare Workers 编程模型：

- Workers；
- D1；
- Durable Objects；
- KV；
- R2；
- Queues；
- Workflows；
- Cron Triggers 和 Durable Object alarms；
- Static Assets、Service Binding，以及 vinext 用例实际依赖的缓存和图片处理能力。

方案不追求 Cloudflare 的边缘调度、跨地域复制、多副本高可用和完整管理面行为。它提供的
是单节点、强本地一致性、可恢复、常用 API 兼容的运行环境。

结构化状态只使用 SQLite。对象字节、Worker bundle 和静态产物使用同一个外部
S3-compatible provider 的隔离前缀。平台不依赖 Redis、Postgres、Kafka 或独立网关。

[Cloudflare 官方 Next.js 指南](https://developers.cloudflare.com/workers/framework-guides/web-apps/nextjs/)
当前推荐 vinext。它通过 Vite 实现 Next.js API，而非消费原版 `next build` 产物；因此本阶段
以 vinext 为行为基准，OpenNext 或原版 Next.js 的结果不能代替 vinext 验收。具体基线、特性边界
与“全部用例通过”的含义见第 18 节，交付顺序和最终门槛见第 22、24 节。

## 2. 目标与非目标

### 2.1 目标

- 跑通官方指南中的 vinext Cloudflare 构建路径，将应用产物部署到 open-compute；只适配
  构建、部署和宿主连接，不以改写应用逻辑或降低断言来取得通过；
- 固定 vinext 基线的全部启用用例按原语义通过，并完整登记上游已有的 skip/fixme/todo；
- 覆盖 SSR、流式 SSR、CSR/hydration、RSC、Server Actions、SSG、ISR、PPR、Cache
  Components、路由、middleware、静态资源和绑定；每项以 vinext 已实现的范围为准；
- 一个安装包或一个容器即可部署；
- 容器外可以作为一个 systemd、launchd 或普通前台服务运行；
- 一个数据目录保存所有本地持久状态；
- 一个 S3 配置同时承载 R2、immutable Worker bundle 和静态部署产物；
- Worker 使用标准 workerd 执行，不 fork workerd；
- 参考 WDL 的 `workerLoader`、binding adapter、immutable version 和 fenced state
  思路；
- 参考 Miniflare 的 workerd 子进程管理、config/plugin 装配、embedded system Worker 和
  本地 persistence 组织方式；
- 常用 Workers API 能够运行现有中小型应用；
- 进程崩溃后 Queue、Workflow 和 alarm 能自动恢复；
- 资源可以独立创建、绑定、重命名、备份和删除。

“与 vinext 对齐”包括其正确拒绝不支持输入的行为，不意味着所有输入都应成功。上游尚未实现
的完整 Next.js PPR、Cache Components 或其他能力不自动转为本平台补齐 Next.js 的任务；
但上游已通过而本平台失败、跳过或未运行的用例，仍然阻塞交付。

### 2.2 非目标

- 原版 Next.js 全 API、全测试集和全部部署模式兼容；
- 超出所选 vinext 基线的完整 PPR/Cache Components、Vercel 专属服务、webpack/Turbopack
  或 OpenNext 专用行为；
- Cloudflare Edge 或 Anycast；
- 跨节点、跨地域同步；
- 多副本同时写入同一份 SQLite；
- Cloudflare KV 的全球 eventual consistency 和 edge cache；
- D1 read replica、bookmark 和完整 PITR；
- Durable Object 全球唯一放置与跨节点迁移；
- Queue exactly-once；
- Cloudflare Workflows 的全部限制、管理 API 和可观测性细节；
- 完整 Wrangler 管理面兼容；
- Kubernetes 和独立微服务拆分。

这些非目标不能用于豁免 vinext 已启用的用例。若实际用例暴露新的平台依赖，应补齐该依赖或
明确报告阻塞，不能缩小测试清单后仍宣称“全部通过”。Node/Vite 开发与构建工具可用于构建、
基线测试和 HMR 验证；生产请求必须由 `platformd` 与 verified stock workerd 承担，不新增
Node SSR 服务，也不以 Miniflare 代替平台运行时。

### 2.3 当前实现与目标的差距

以下为 2026-08-28 源码观察，包含当前工作树，不是本次重新执行的验收结果：

| 范围 | 当前基础 | 目标仍需完成 |
| --- | --- | --- |
| Worker 执行 | workerd、WorkerLoader、Fetch/Streams、显式 `nodejs_compat` | vinext 真实产物、所需 Node API 与资源预算验证 |
| TS 工具链 | TS7 类型检查、Rolldown 普通 Worker 打包 | vinext/Vite 的 RSC、SSR、client 多环境构建及产物导入 |
| 资源与绑定 | KV/R2/D1/DO、Queues/Cron/Workflows 的已声明子集 | 逐项验证 vinext 依赖，不能由产品名称推导完整兼容 |
| Static Assets | S3 ArtifactStore、摘要校验、不可变部署 | 资源 manifest、上传、路由、`ASSETS.fetch()` 与原子发布 |
| Service Binding | 原生 entrypoint stub、受限绑定装配基础 | 跨 Worker/自绑定、目标解析、fetch/RPC、生命周期与删除约束 |
| 框架缓存与图片 | 可复用存储基础 | vinext 实际使用的缓存接口、失效语义和图片处理绑定 |
| 验收 | 已有平台 Gate 与产品测试 | vinext 全量清单、上游对照、平台浏览器与生产构建结果 |

实现依据包括 [工具链](../packages/toolchain/src/build-worker.ts)、
[绑定类型](../crates/core/src/resource.rs)、[RuntimeSource](../crates/workers/src/runtime_source.rs)
和[能力注册表](../crates/service/src/capabilities.rs)。本次文档更新不安装 vinext、不运行其测试，
也不将这些差距标为已完成。

### 2.4 Day1 约束

遵循 [AGENTS.md](../AGENTS.md)：直接调整当前部署、schema、配置和内部协议，不保留旧开发
版本的双读写、迁移回退或旧引擎。现有阶段编号、V1/V2 名称和历史验证记录不产生兼容义务；
已识别的历史路径按 [Day1 清理清单](./day1-architecture-cleanup.md) 收敛。

只有声明支持范围内、Cloudflare 官方 API 要求的兼容日期/flag 行为可作为兼容例外，并需记录
来源、适用范围、workerd pin 和回归测试。跟踪 vinext 的版本是固定依赖与测试基线，不是保留
多套 open-compute 历史实现。任何调整仍须保留隔离、完整性、不可变部署和当前状态的崩溃恢复。

## 3. 部署单元

“单体服务”定义为一个可安装、启动和升级的部署单元，不强求只有一个 OS process。

发行形式为单个按平台构建的 `platformd`，内部包含正式 pin 对应的 workerd 和 runtime assets：

```text
platformd          唯一发行文件、Rust 主进程
└── workerd        校验并物化内嵌资源后启动的 upstream 子进程
```

`platformd` 启动并监督 `workerd` 子进程，两者通过仅监听 loopback 的内部 HTTP 通信。
外部只暴露 `platformd` 的 public/control 端口。具体发行、离线启动与资源校验契约见
[单二进制分发与部署](./references/single-binary.md)；框架构建器和 Node/Bun 不进入生产请求路径。

```text
Public HTTP / Control API
            │
            ▼
┌─────────────────────────────────────────┐
│ platformd                               │
│                                         │
│  ingress/router        control API      │
│  resource catalog      binding backend  │
│  queue scheduler       workflow engine  │
│  SQLite manager        S3 adapter        │
│  workerd supervisor                     │
└───────────────┬─────────────────────────┘
                │ authenticated loopback
                ▼
┌─────────────────────────────────────────┐
│ workerd                                 │
│                                         │
│  assets/service router workerLoader     │
│  loaded tenant worker  native DO/facets │
└─────────────────────────────────────────┘
```

这样仍然是一个服务、一个配置、一个数据目录和一个生命周期，同时避免把 workerd 作为
C++ library 嵌入 Rust 带来的升级和 ABI 维护成本。

## 4. 与 WDL / Miniflare 的关系

### 4.1 保留的思路

- Worker deployment 是不可变版本；
- workerd `workerLoader` 按不可变 ID 动态加载并缓存 Worker；
- runtime 在 loader realm 构造受限的 binding adapter；
- tenant Worker 不直接获得 SQLite 文件路径、S3 credential 或内部通用 Fetcher；
- HTTP、scheduled、queue、workflow 和 DO 统一走内部 dispatch envelope；
- Workflow、Queue 和 alarm 使用 run token、lease 和 generation 防止旧执行提交；
- Durable Object 继续使用 workerd native SQLite/facet 能力。

### 4.2 有意简化的部分

WDL 当前把 control、route、bundle、KV、Queue 和 Workflow 状态分布在不同 Redis DB，
并把 Gateway、runtime、scheduler、workflows、D1 runtime 和 DO runtime 做成独立 service
family。本方案改为：

| WDL | 本方案 |
| --- | --- |
| Redis DB 0 control state | `control.sqlite` |
| Redis DB 1 KV/Queue | KV resource files + `scheduler.sqlite` |
| Redis DB 2 Workflow state | `scheduler.sqlite` |
| 多个 runtime service | 一个受监督的 workerd |
| Gateway + Control + Scheduler | 一个 `platformd` |
| 多副本 owner lease | 单节点进程所有权 |
| S3 R2/ASSETS | S3 R2/bundle/assets |

单节点不需要 WDL 的跨 replica owner protocol，但 Queue、Workflow 和 alarm 仍然必须使用
lease token，因为进程可能在执行和提交之间崩溃。

### 4.3 Miniflare 的参考位置

Miniflare 和 WDL 不应被当作两个互斥实现。两者适合参考的层次不同：

| 来源 | 主要参考内容 | 本方案不照搬的部分 |
| --- | --- | --- |
| workerd | runtime、capability binding、`workerLoader`、DO/facet 的真实语义 | 不 fork、不依赖未验证的内部文件布局 |
| Miniflare | 生成 workerd config、启动/停止子进程、等待 socket ready、plugin 生成 binding/service、embedded system Worker、本地 persistence | Node bridge、dev registry、热重载和本地开发专用 API |
| WDL | immutable deployment、动态 runtime、tenant trust boundary、binding-scoped adapter、持久任务 fencing | Redis、多副本 owner lease、微服务部署拓扑 |

Miniflare 宿主会生成 binary Cap'n Proto config；它的 `Runtime` 通过 stdin 把 config 交给
workerd，并用 control fd 等待所需 socket 上报 ready。`Runtime` 还处理 startup stderr、
structured logs、子进程退出和 restart notification。这些是 `platformd` supervisor 和 G0
harness 的直接参考，但生产实现不引入 Miniflare/Node 作为运行时依赖。

Miniflare 的 resource plugin 也验证了一种有价值的组合方式：plugin 负责生成 tenant binding、
entry service、embedded Worker 和 disk service。例如 D1、R2 使用共享 entry service，并通过
binding `props` 传入具体 resource ID；DO persistence 则由专用 disk service 接到 workerd
`localDisk`。本方案沿用“共享 adapter + immutable resource props”的能力模型，但仍保留自己
的物理存储决策：KV/D1 一资源一 SQLite，R2 字节进入外部 S3。

Miniflare 是本地 simulator，不是 hostile multi-tenant production control plane。它的 Node
binding、直接 storage getter、remote dev proxy 和自动 config reload 不能暴露给 tenant。
尤其是其当前 Queue broker 明确使用 in-memory DO storage，因此只能参考 API/event dispatch，
不能作为本方案 Queue durability、lease 或 crash recovery 的依据。

### 4.4 vinext、Static Assets 与 Service Binding 的参考边界

- vinext 是本阶段的框架行为与用例基准，固定源码 revision，保留其 fixture 和断言来源。
- [官方 asset-worker](../references/workers-sdk/packages/workers-shared/asset-worker/src/handler.ts)
  与 [router-worker](../references/workers-sdk/packages/workers-shared/router-worker/src/worker.ts)
  提供资源响应和分流语义；参考其算法与测试，不引入 Cloudflare 内部遥测、实验分组或账单系统。
- [官方 RPC proxy](../references/workers-sdk/packages/miniflare/src/workers/assets/rpc-proxy.worker.ts)
  展示默认入口 fetch 经资源路由、RPC 直达用户 Worker 的组合方式。Miniflare 只作为参考和
  上游对照环境，不能成为 open-compute 的生产依赖或最终 Gate 替代物。
- [WDL Service Binding](../references/wdl/runtime/bindings/service.js) 可参考原生 stub 的连接，
  但不照搬部署时永久固定目标版本、禁止自绑定、透传调用方 secrets 等行为；其 `ASSETS.url()`
  也不能替代官方 `ASSETS.fetch()`。
- [workerd WorkerLoader](../references/workerd/src/workerd/api/worker-loader.h) 是原生调用能力
  的参考；最终以当前正式 pin 的真实执行结果为准。引用或复用第三方代码时固定来源并保留许可证。

## 5. Worker 加载与调度

### 5.1 不可变版本

每次部署产生新的 deployment ID：

```text
<account-id>/<worker-id>/<deployment-id>
```

这个完整 ID 是 `workerLoader` cache key。禁止使用 worker name 或 `active` 作为 cache
key。

```text
workers.active_deployment_id ──► worker_deployments.id
                                      │
                                      └── bundle_ref + immutable metadata
```

Promotion 只原子更新 `active_deployment_id` 和 route projection。Rollback 是把 active
指针改回旧的不可变 deployment。正常发布不需要 loader cache invalidation 协议。

### 5.2 Dispatch 边界

SQLite scheduler 负责“什么时候运行”，`workerLoader` 负责“加载哪个 Worker 并执行”。

```text
SQLite scheduler / ingress
        │
        │ DispatchEnvelope
        ▼
workerd runtime worker
        │
        ├── LOADER.get(immutableDeploymentId, loadBundle)
        ├── stub.getEntrypoint(name)
        └── stub.getDurableObjectClass(className)
```

建议的 envelope：

```ts
type DispatchEnvelope = {
  requestId: string
  deploymentId: string
  event:
    | { type: "fetch"; request: SerializedRequest }
    | { type: "scheduled"; scheduledTime: number; cron: string }
    | { type: "queue"; queueId: string; batch: QueueMessage[] }
    | { type: "workflow"; instanceId: string; className: string; runToken: string }
    | { type: "do"; storageId: string; className: string; objectId: string }
}
```

入口映射：

| Event | Runtime 调用 |
| --- | --- |
| HTTP | `stub.getEntrypoint().fetch(request)` |
| Scheduled | 默认 entrypoint 的 scheduled handler |
| Queue | 默认 entrypoint 的 queue handler |
| Workflow | `stub.getEntrypoint(className).run(...)` |
| Durable Object | `stub.getDurableObjectClass(className, ...)` |

### 5.3 版本冻结规则

- HTTP：每次请求解析当前 active deployment；
- Static Assets：请求使用该次解析出的 deployment manifest；`ASSETS.fetch()` 使用所属
  Worker deployment 的资源集，不读取另一个 active deployment 的资源；
- Service Binding：绑定冻结目标 Worker ID 和 entrypoint，调用时解析目标当前 deployment，
  随即固定本次执行；不能把目标 deployment 永久缓存进调用方 env。自绑定遵循同一解析规则；
- Queue：claim batch 时冻结 consumer deployment 和 consumer generation；
- Workflow：instance 创建时永久冻结 deployment 和 class；
- DO：storage identity 跨普通 deploy 保持不变，新 facet 使用当前 deployment；
- DO alarm：保存 alarm 时记录目标 deployment，promotion policy 可以选择 preserve 或
  restart；V1 默认 restart，部署时关闭已有 WebSocket 并让新请求使用新版本。

### 5.4 Service Binding 与资源路由的组合

在 `runtime/src/services/` 实现受限的原生 `WorkerEntrypoint`/Fetcher 能力，Rust authority
负责绑定验证、目标解析、pin 和删除约束，WorkerLoader 负责加载目标。不得绕公网域名或使用
自建 JSON RPC 代替 workerd 原生 RPC。支持自绑定和惰性加载，不能在组装 env 时递归展开依赖图。

| 调用 | 路径 |
| --- | --- |
| 默认入口的 `SERVICE.fetch()` | 目标部署的 Assets/Worker 路由 |
| 默认入口的 RPC 方法 | 目标用户 Worker |
| 具名 entrypoint 的 fetch/RPC | 指定入口，不经过默认 Assets 路由 |
| `ASSETS.fetch()` | 所属部署的资源服务 |

目标只获得自己的 env；私有身份和 token 不从业务请求传入，也不透传调用方 secrets。每次调用
校验绑定与目标状态，暖缓存不绕过 authority。调用预算不能每跳重置；pin 必须覆盖执行、响应流、
WebSocket 和 RPC 对象等相应生命周期，验证 `waitUntil`、异常、递归上限、删除和进程重启行为。
具体 Node API、原生 RPC 和绑定支持面由 vinext 用例与当前 pin 的真实 Gate 确认。

## 6. 数据目录与 SQLite 边界

推荐布局：

```text
data/
├── control.sqlite
├── scheduler.sqlite
├── kv/
│   └── <account-id>/<namespace-id>/data.sqlite
├── d1/
│   └── <account-id>/<database-id>/data.sqlite
├── do/
│   └── workerd localDisk managed files
├── cache/
│   ├── bundle/<sha256>
│   └── assets/<sha256>
└── backup-staging/
```

| 数据 | Authority | 文件策略 |
| --- | --- | --- |
| Worker、deployment、binding、resource catalog | `control.sqlite` | 全局一个 |
| Queue、Workflow、Cron、alarm projection | `scheduler.sqlite` | 全局一个 |
| KV | namespace SQLite | 一 namespace 一文件 |
| D1 | database SQLite | 一 database 一文件 |
| Durable Object | workerd native SQLite | 逻辑上一 object 私有，物理由 workerd 管理 |
| R2 objects | S3 | 不镜像 object catalog 到 SQLite |
| Worker bundle/assets | S3 | content-addressed，SQLite 只保存引用 |

两个系统数据库分开是为了避免高频 Queue/Workflow 写入扩大 control WAL、阻塞 deploy 和
资源管理。它们仍由同一个进程管理。

## 7. `control.sqlite`

### 7.1 核心实体

下面是逻辑 schema，不是最终 migration：

```sql
CREATE TABLE accounts (
  id              TEXT PRIMARY KEY,
  name            TEXT NOT NULL,
  created_at_ms   INTEGER NOT NULL
);

CREATE TABLE workers (
  id                      TEXT PRIMARY KEY,
  account_id              TEXT NOT NULL REFERENCES accounts(id),
  name                    TEXT NOT NULL,
  active_deployment_id    TEXT,
  do_storage_id           TEXT NOT NULL,
  created_at_ms           INTEGER NOT NULL,
  deleted_at_ms           INTEGER
);

CREATE UNIQUE INDEX workers_live_name
ON workers(account_id, name)
WHERE deleted_at_ms IS NULL;

CREATE TABLE worker_deployments (
  id                    TEXT PRIMARY KEY,
  worker_id             TEXT NOT NULL REFERENCES workers(id),
  version_number        INTEGER NOT NULL,
  bundle_sha256         BLOB NOT NULL,
  bundle_ref            TEXT NOT NULL,
  compatibility_date    TEXT NOT NULL,
  metadata_json         BLOB NOT NULL,
  state                 TEXT NOT NULL,
  created_at_ms         INTEGER NOT NULL,
  UNIQUE (worker_id, version_number)
);

CREATE TABLE deployment_bindings (
  id                       TEXT PRIMARY KEY,
  deployment_id            TEXT NOT NULL REFERENCES worker_deployments(id),
  name                     TEXT NOT NULL,
  kind                     TEXT NOT NULL,
  resource_id              TEXT NOT NULL REFERENCES resources(id),
  resource_spec_generation INTEGER NOT NULL,
  capability_version       INTEGER NOT NULL,
  permissions_json         BLOB NOT NULL,
  config_json              BLOB NOT NULL,
  descriptor_sha256        BLOB NOT NULL,
  UNIQUE (deployment_id, name)
);
```

所有 resource 使用共同生命周期：

```sql
CREATE TABLE resources (
  id                     TEXT PRIMARY KEY,
  account_id             TEXT NOT NULL REFERENCES accounts(id),
  kind                   TEXT NOT NULL,
  name                   TEXT NOT NULL,
  state                  TEXT NOT NULL,
  availability           TEXT NOT NULL,
  spec_generation        INTEGER NOT NULL DEFAULT 1,
  driver_schema_version  INTEGER NOT NULL,
  created_at_ms          INTEGER NOT NULL,
  deleted_at_ms          INTEGER
);

CREATE UNIQUE INDEX resources_live_name
ON resources(account_id, kind, name)
WHERE state != 'tombstoned';
```

这里仍是总览。实际列、trigger、referrer、descriptor canonicalization 和 lifecycle/availability
边界以 [P0.3：Resource 与 Binding Framework 详细设计](./implemented/p0-3-resource-binding-framework.md)
为准。

产品表保存各自的物理信息：

```text
kv_namespaces(resource_id, storage_key, schema_version, quota_bytes)
d1_databases(resource_id, storage_key, schema_version, quota_bytes)
r2_buckets(resource_id, physical_prefix)
queues(resource_id, retention, dlq_resource_id)
queue_consumers(queue_id, worker_id, batch_config, generation)
workflow_definitions(resource_id, worker_id, class_name, definition_key)
cron_triggers(resource_id, worker_id, cron, generation)
```

### 7.2 Scheduler projection

`control.sqlite` 与 `scheduler.sqlite` 不能依赖跨文件原子事务。Scheduler 不能在 claim
transaction 中临时 join control 数据，而是读取 `scheduler.sqlite` 内的 runtime projection：

```text
queue_consumer_projection(
  queue_id, generation, deployment_id, batch_config, updated_at_ms
)

cron_trigger_projection(
  trigger_id, generation, deployment_id, cron, next_run_at_ms, updated_at_ms
)
```

Control mutation 先提交新的权威 definition，再幂等更新 scheduler projection。projection
更新成功前，旧 deployment 必须保持 retained；旧 projection 继续接收少量工作是允许的，
但不能指向已删除版本。启动恢复器比较 generation 并修复缺失或落后的 projection。

Queue claim 和 Cron tick 只读取同一个 `scheduler.sqlite` 中的 projection，因此 claim 与
冻结 deployment/generation 是单文件原子操作。projection 是可重建索引，control definition
仍是 authority。

### 7.3 Binding 必须冻结物理 ID

部署记录保存：

```text
CACHE -> kv_01JXYZ...
DB    -> d1_01JABC...
```

不能只保存：

```text
CACHE -> "production-cache"
```

资源 rename 只改变 display name。删除后以同名重建必须产生新 ID，因此旧 deployment
不会意外绑定到新资源。

### 7.4 Secret

Secret ciphertext 存在 `control.sqlite`，使用部署时提供的 master key 做 authenticated
encryption。master key 只来自环境变量或权限受限的本地 key file，不写入数据库、日志、
S3 manifest 或命令行参数。明文只在构建指定 Worker env 时短暂存在于 host memory。

### 7.5 部署资源与服务绑定

第 7.1 节的 `deployment_bindings` 描述产品 resource 关联，不应通过伪造 `ResourceId` 把
Static Assets 和 Service Binding 塞入同一张资源表。新增关系按各自所有权定义：

```text
deployment_assets(deployment_id, manifest_ref, routing_config, optional_binding_name)
deployment_services(deployment_id, binding_name, target_worker_id, optional_entrypoint)
```

这是待实现的逻辑模型，不是已经应用的 SQL。所有 binding、vars、secrets 的名称在部署 authority
边界统一检查冲突；目标 Worker 必须属于授权账户。代码、资源引用、绑定和配置一起参与部署摘要。
Assets 归部署所有；Service Binding 的定义不可变，但目标的 active deployment 可独立变化。
同名 Worker 删除重建不复用旧 ID，不能让旧绑定自动连接到新 Worker。

## 8. KV

### 8.1 物理模型

推荐：一个 KV namespace 对应一个 SQLite 文件，而不是一个全局 `kv_entries` 表。

```text
kv/<account-id>/<namespace-id>/data.sqlite
```

每个文件只有一张 entry 核心表。这里保留显式 rowid，因为单 value 可达 25 MiB，SQLite 的
incremental BLOB I/O 不支持 `WITHOUT ROWID` table：

```sql
CREATE TABLE kv_entries (
  id             INTEGER PRIMARY KEY,
  key            BLOB NOT NULL UNIQUE,
  value          BLOB NOT NULL,
  metadata_json  BLOB,
  expires_at_ms  INTEGER,
  updated_at_ms  INTEGER NOT NULL
) STRICT;

CREATE INDEX kv_entries_expiration
ON kv_entries(expires_at_ms, id)
WHERE expires_at_ms IS NOT NULL;
```

`key` 保存用户字符串的 UTF-8 bytes。BLOB 排序可以实现按 UTF-8 bytes 的字典序 list，
避免依赖 locale collation。

### 8.2 常用操作

```sql
-- get
SELECT id, length(value), metadata_json
FROM kv_entries
WHERE key = :key
  AND (expires_at_ms IS NULL OR expires_at_ms > :now);

-- put 的实际实现：BEGIN IMMEDIATE 后 insert/update zeroblob(:size)，
-- 再按 rowid 使用 incremental BLOB I/O，全部成功后 COMMIT。

-- delete
DELETE FROM kv_entries WHERE key = :key;
```

`list({ prefix, cursor, limit })` 使用 keyset pagination：

```sql
SELECT key, metadata, expires_at_ms
FROM kv_entries
WHERE key >= :prefix
  AND (:prefix_end IS NULL OR key < :prefix_end)
  AND (:last_key IS NULL OR key > :last_key)
  AND (expires_at_ms IS NULL OR expires_at_ms > :now)
ORDER BY key
LIMIT :limit_plus_one;
```

cursor 是带版本和 HMAC 的 opaque token，绑定 namespace generation、prefix 和 last key，
防止调用者伪造跨 namespace cursor。

过期 row 在读取时视为不存在，后台 GC 使用 `expires_at_ms` index 分批物理删除。单节点
没有异地 replica，因此不需要 tombstone。

### 8.3 为什么不共享一个 KV 数据库

一个共享表也可以实现：

```sql
PRIMARY KEY (namespace_id, key)
```

但它会让所有 namespace 竞争一个 SQLite writer lock，也让 namespace 级备份、恢复、
删除和 quota 统计变重。一 namespace 一文件能把写锁和故障影响隔离到资源边界。

代价是文件和连接数量增加。`platformd` 应使用 LRU connection manager，只保持最近活跃
的文件打开。对几十到几百个 namespace 的 SMB 部署，这个取舍优于全局表。达到数万
namespace 后再引入固定数量的 KV shard files，不在 V1 提前设计。

不要在同一个 SQLite 中为每个 namespace 动态创建一张表。这既没有拆开 writer lock，
又增加动态 DDL、schema cache、迁移和 table-name 安全问题。

API 兼容面、25 MiB stream、TTL/list cursor、connection manager、backup/restore、corruption
隔离和完整 Gate 见 [P0.4：KV 详细设计](./implemented/p0-4-kv.md)。

## 9. D1

### 9.1 一数据库一文件

D1 允许用户创建自己的 table、index、view、trigger 和数据，因此一个 D1 database 必须对应独立
SQLite 文件；原子多 statement 使用 `batch()`，不允许 transaction 跨 binding call：

```text
d1/<account-id>/<database-id>/data.sqlite
```

`control.sqlite` 只保存 resource metadata、file key、binding referrer 和 lifecycle。
deployment 绑定冻结 physical database ID。

### 9.2 Runtime facade

tenant Worker 获得 Cloudflare-shaped D1 binding：

```text
prepare / bind / first / all / raw / run
batch
exec
```

`prepare()` 和 `bind()` 必须同步返回可复用的本地 statement object，因此不能把整个 D1 facade
直接做成远端 JSRPC object。P0.5.0 先建立 loaded-isolate facade framework：WorkerLoader 注入项目
自有 D1 client module 和 deterministic main wrapper，在 tenant isolate 内构造 `D1Database`/
`D1PreparedStatement`；只有 `run/all/first/raw/batch/exec` 这类 terminal operation 通过
binding-scoped JSRPC transport 回到 `platformd`。transport 的 immutable props 固定 binding、
deployment、database generation 和 permission，tenant code 不能选择其他文件。

完整 facade Gate、SQLite authorizer/limits、batch/exec/migration、backup/restore、工作包和测试矩阵见
[P0.6：D1 详细设计](./implemented/p0-6-d1.md)。

### 9.3 SQL 安全边界

平台必须拦截或禁用会突破文件边界的 SQL：

- `ATTACH` / `DETACH`；
- extension loading；
- filesystem-affecting pragma；
- 可写 schema 或其他可绕过 authorizer 的路径；
- 任意 database filename；
- 无界 result、statement count 和 request bytes。

每个请求设置 statement、row、result byte 和 execution time budget。不要把 tenant SQL 和
`control.sqlite` 连接放在同一个 SQLite connection。

## 10. Durable Objects

### 10.1 逻辑与物理边界

Cloudflare 的语义是每个 Durable Object instance 拥有私有、强一致的持久存储。本方案
保留这个逻辑边界，但不由 `platformd` 手工创建一个 OS 文件。

使用 workerd native Durable Object/facet SQLite 和 `localDisk`：

```text
doStorageId + namespaceId + objectId + objectGeneration
    -> one native DoHost actor
        -> one dynamically loaded tenant facet
```

workerd 管理物理 SQLite。动态 class 通过 `workerLoader` 加载，再由 host actor 获取
`getDurableObjectClass()`。WDL 的 fixed supervisor shard 主要服务多副本 owner/placement；本方案
是单进程单 workerd，P0.7 采用“一 tenant object 一 host actor”，让相同 object 使用 native actor
ordering、不同 object 真正并行，也避免 namespace 级 supervisor 成为瓶颈。public ID、binding、
generation fence、delete/recreate 与 workerd storage ownership 见
[P0.7：Durable Objects 详细设计](./implemented/p0-7-durable-objects.md)。

### 10.2 Storage identity

- 普通 deployment promotion 不改变 `do_storage_id`；
- Worker 删除并以同名重建时生成新 `do_storage_id`；
- class/object identity 与 immutable deployment 解耦；
- code version 可以更新，object storage 不能随 deployment 更换；
- V1 deployment 使用 restart policy：关闭旧 WebSocket，下一次请求重建新 facet，保留
  SQLite 数据。

### 10.3 Concurrency

单节点只有一个 workerd owner，不需要 Redis owner lease。仍需保持：

- 相同 object ID 的请求由 native DO 串行化；
- 不同 object 可以并行；
- DO binding 只能访问 deployment 声明的 class；
- storage deletion 按 `do_storage_id + class + object` 限定；
- Worker delete 使用 tombstone，不能立刻重用 storage identity。

### 10.4 Alarms

在动态 facet 路径中，alarm 使用两层状态：

1. object SQLite 中的 alarm row 是 authority；
2. `scheduler.sqlite` 的 due row 是可重建 projection。

每次 alarm mutation 生成新的 `row_token`。Scheduler dispatch 必须同时匹配
`object identity + row_token`，旧 due task 不能触发后来已删除或改期的 alarm。

如果 object transaction 已提交但 scheduler projection 写入失败：

- `getAlarm()` 做 read repair；
- object 首次重新激活时做 repair；
- 启动恢复任务扫描 best-effort observed-object registry；
- projection 缺失不能删除 object SQLite 中的权威 alarm row。

Alarm delivery 是 at-least-once，handler 必须支持幂等。

完整的 facet alarm Hard Gate、object-local shim table、`scheduler.sqlite` schema、claim lease、
conditional completion、六次 retry、repair、shutdown 与 crash matrix 见
[P0.8：Scheduler Kernel 与 DO Alarms 详细设计](./implemented/p0-8-scheduler-do-alarms.md)。

## 11. R2、S3 与 Static Assets

### 11.1 Virtual bucket

外部只要求一个 S3-compatible provider。平台用不透明 prefix 实现多个逻辑 R2 bucket：

```text
tenant/r2/v1/<resource-id>/objects/<user-object-key>
```

`control.sqlite` 保存：

```text
r2_buckets(resource_id, physical_prefix)
```

object bytes、etag 和 object metadata 以 S3 为 authority。V1 不在 SQLite 镜像 object
catalog，`head/get/put/delete/list` 直接调用 S3。

S3 object key 是平面字符串，不对 user object key 做 filesystem path normalization。
物理 identity 是固定 platform prefix 加原始 user key；runtime 只在 S3 HTTP 协议和签名
边界做必要编码，不能把 percent-encoded 文本或 double-encoded 文本当成新的 object key。
错误和返回值只暴露 virtual key。

### 11.2 平台内部对象

使用与 tenant R2 分离的内部 prefix：

```text
system/bundles/<sha256>
system/assets/blobs/<sha256>
system/assets/manifests/<sha256>
system/backups/<product>/...
tenant/r2/v1/<resource-id>/objects/<key>
```

Worker bundle 是 immutable content-addressed object。`control.sqlite` 保存 hash 和 ref，
本地 `cache/bundle/` 只是可删除缓存，不是 authority。

S3 credential 只存在于 `platformd`。tenant Worker 获得的是 binding-scoped adapter，不能
读取物理 bucket、credential 或 `system/` prefix。

R2 返回对象带同步 `writeHttpMetadata()` 和本地 body helper，因此与 D1 一样使用 P0.5.0 的
loaded-isolate facade；raw transport 只传 metadata DTO 和 byte stream。完整 key budget、metadata
codec、conditional/range/list、provider preflight、bucket lifecycle、工作包和测试矩阵见
[P0.5：R2 详细设计](./implemented/p0-5-r2.md)。

### 11.3 Static Assets

以上 assets 路径是目标逻辑布局，实际引用和校验复用现有 ArtifactStore。构建工具扫描声明的
资源目录，验证路径、大小、重复项与忽略规则，生成 `URL path -> digest/size/content-type`
manifest；资源字节独立上传，不塞进 Worker JS bundle。部署 ready/promote 前必须确认全部引用
有效，代码和 manifest 一起冻结。上传中断不影响当前部署，损坏或缺失对象不能伪装成普通 404。

`platformd` 解析 public route 并取得 deployment pin，受信任的资源路由再决定进入 assets 服务
还是 tenant Worker。`env.ASSETS.fetch()` 只进入该部署的 assets 服务，不能回到租户入口造成递归。
两条路径复用同一套路径匹配、GET/HEAD、Content-Type、ETag/304 和响应逻辑；S3 凭据与内部读取
能力留在平台，不向 tenant 提供按任意物理 key/digest 读取对象的接口。

遵循声明支持的 [Static Assets 配置](https://developers.cloudflare.com/workers/static-assets/binding/)：
默认资源优先，`run_worker_first` 控制 Worker 优先及路径规则；Worker 已执行后不因其返回 404
自动补一次资源响应。HTML handling、404/SPA、`_headers`、`_redirects` 和路径编码按上游契约逐项
验收，未实现的配置明确拒绝。需要 Worker 鉴权的资源通过 Worker 优先路径访问。

发布与回滚不混用 manifest；缓存按不可变身份隔离。跨请求的 HTML/JS 版本一致性仍须单独测试，
不能把逐请求部署 pin 当成整个浏览器会话的版本亲和保证，也不能扫描历史部署来掩盖资源缺失。

## 12. Queues

### 12.1 为什么不能一 Queue 一 SQLite

Scheduler 需要按 `available_at_ms` 全局寻找可运行消息，并在一个短事务里完成 claim。
如果每个 Queue 都是独立文件，scheduler 必须轮询和打开大量数据库，也无法方便地实现
公平调度。因此所有 Queue runtime state 放在 `scheduler.sqlite`。

Queue definition 和 consumer binding 仍放在 `control.sqlite`。

### 12.2 核心 schema

```sql
CREATE TABLE queue_messages (
  id                       TEXT PRIMARY KEY,
  queue_id                 TEXT NOT NULL,
  body                     BLOB NOT NULL,
  content_type             TEXT NOT NULL,
  created_at_ms            INTEGER NOT NULL,
  available_at_ms          INTEGER NOT NULL,
  status                   TEXT NOT NULL,
  attempt_count            INTEGER NOT NULL DEFAULT 0,
  max_attempts             INTEGER NOT NULL,
  lease_token              TEXT,
  lease_expires_at_ms      INTEGER,
  consumer_generation      INTEGER,
  consumer_deployment_id   TEXT,
  last_error_json          BLOB
);

CREATE INDEX queue_messages_ready
ON queue_messages(available_at_ms, queue_id, id)
WHERE status = 'ready';

CREATE INDEX queue_messages_expired_lease
ON queue_messages(lease_expires_at_ms, id)
WHERE status = 'running';
```

### 12.3 Claim 与提交

Claim 在 `BEGIN IMMEDIATE` 事务内执行：

1. 选择一个 queue 的 due messages；
2. 读取同库 `queue_consumer_projection` 的 consumer generation 和 deployment；
3. 写入随机 `lease_token`、lease expiry 和冻结的 deployment；
4. 返回 batch；
5. 提交事务后 dispatch 到 workerd。

Handler 返回后，每条消息独立 ack 或 retry。提交必须带条件：

```sql
WHERE id = :id
  AND status = 'running'
  AND lease_token = :lease_token
```

旧 handler、超时 handler 或进程崩溃前的 handler 无法提交新 claim 的结果。

### 12.4 Delivery 语义

- at-least-once；
- 不保证严格顺序；
- 支持 delay、batch、per-message ack/retry 和 retry-all；
- lease 过期后重新进入 ready；
- 达到 max attempts 后，在同一个 scheduler transaction 中移动到 DLQ 或标记 dead；
- producer `sendBatch()` 在一个 SQLite transaction 中写入；
- handler 外部副作用需要使用 message ID 作为 idempotency key。

## 13. Workflows

### 13.1 执行模型

Workflow instance 创建时冻结：

```text
workflow definition
worker deployment ID
class name
input
creation generation
```

每次 activation 从 `run()` 开始 replay。`step.do()`、`step.sleep()`、
`step.sleepUntil()` 和 `step.waitForEvent()` 通过持久化 step record 跳过已完成工作或挂起
执行。

### 13.2 核心 schema

```sql
CREATE TABLE workflow_instances (
  id                    TEXT PRIMARY KEY,
  workflow_id           TEXT NOT NULL,
  worker_deployment_id  TEXT NOT NULL,
  class_name            TEXT NOT NULL,
  status                TEXT NOT NULL,
  input                 BLOB NOT NULL,
  output                BLOB,
  error_json            BLOB,
  generation            INTEGER NOT NULL,
  run_token             TEXT,
  run_lease_until_ms    INTEGER,
  next_wake_at_ms       INTEGER,
  waiting_event_type    TEXT,
  created_at_ms         INTEGER NOT NULL,
  updated_at_ms         INTEGER NOT NULL,
  terminal_at_ms        INTEGER
);

CREATE INDEX workflow_instances_due
ON workflow_instances(next_wake_at_ms, id)
WHERE status IN ('queued', 'sleeping', 'retrying', 'waiting');

CREATE TABLE workflow_steps (
  instance_id       TEXT NOT NULL REFERENCES workflow_instances(id),
  step_key          TEXT NOT NULL,
  step_type         TEXT NOT NULL,
  status            TEXT NOT NULL,
  attempt           INTEGER NOT NULL,
  result            BLOB,
  error_json        BLOB,
  retry_at_ms       INTEGER,
  updated_at_ms     INTEGER NOT NULL,
  PRIMARY KEY (instance_id, step_key)
);

CREATE TABLE workflow_events (
  id                TEXT PRIMARY KEY,
  instance_id       TEXT NOT NULL REFERENCES workflow_instances(id),
  event_type        TEXT NOT NULL,
  payload           BLOB NOT NULL,
  created_at_ms     INTEGER NOT NULL,
  consumed_at_ms    INTEGER
);

CREATE INDEX workflow_events_waiting
ON workflow_events(instance_id, event_type, created_at_ms)
WHERE consumed_at_ms IS NULL;
```

### 13.3 Fencing

每次 activation claim 新的 `run_token` 和 lease。所有 step commit、terminal commit、event
consume 和 retry schedule 都必须匹配：

```text
instance id
generation
run token
unexpired lease
compatible current status
```

实例 restart 会增加 generation。旧 execution 即使晚到，也不能写入新 generation。

### 13.4 语义边界

- 已成功持久化的 `step.do()` result 在 replay 时直接返回；
- callback 已产生外部副作用、但 result commit 前进程崩溃时，callback 可能再次执行；
- 因此 Workflow 不能承诺任意外部 side effect exactly-once；
- `sleep` 和 retry 只保存 due timestamp，不占用 isolate；
- `waitForEvent` 按 instance 和 event type 匹配最早未消费事件；
- terminal instance 按 retention policy 清理；
- 只要实例仍存活，就阻止其冻结的 Worker deployment 被物理删除。

## 14. Cron 与 Scheduler

Cron definition 在 `control.sqlite`；带 generation、deployment 和 `next_run_at_ms` 的
runtime projection 以及运行记录在 `scheduler.sqlite`：

```sql
CREATE TABLE cron_runs (
  trigger_id          TEXT NOT NULL,
  scheduled_for_ms    INTEGER NOT NULL,
  status              TEXT NOT NULL,
  lease_token         TEXT,
  lease_expires_at_ms INTEGER,
  PRIMARY KEY (trigger_id, scheduled_for_ms)
);
```

`PRIMARY KEY` 防止同一 slot 重复创建。V1 使用 best-effort cron：进程停机期间的所有历史
slot 不回放，恢复后只计算下一个未来 slot。Queue、Workflow 和 DO alarm 则必须通过 due
row 和 lease 恢复，不得因重启丢失。

Scheduler 主循环按较小批次交错处理：

```text
expired leases
→ due queues
→ due workflows
→ due DO alarms
→ due cron
→ retention / GC
```

每次 tick 只负责 claim 和 admission，不等待 tenant handler 完成。执行受独立的 Queue、
Workflow、alarm concurrency semaphore 限制，避免一种工作负载饿死其他工作负载。

## 15. SQLite 运行规则

### 15.1 默认 pragma

所有平台管理的 SQLite 文件默认使用：

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = <bounded value>;
```

`synchronous = FULL` 优先保证 self-deploy 数据在断电后的 durability。未来可以允许管理员
显式切换为 `NORMAL`，但不能静默改变。

WAL 适合单机多连接，reader 不阻塞 writer；每个文件仍只有一个并发 writer。这正是 KV
和 D1 按资源拆文件、control 与 scheduler 分文件的原因。

WAL 文件必须位于同一主机的本地或可靠 block volume。不要把活动 SQLite WAL 数据目录
放在 NFS、SMB 或普通对象存储挂载上。S3 只用于对象和备份快照。

### 15.2 Connection manager

- `control.sqlite` 和 `scheduler.sqlite` 使用固定 connection pool；
- KV/D1 使用按 resource ID 索引的 LRU pool；
- 同一个文件限制写 connection 数；
- 文件关闭前执行 bounded checkpoint；
- 不允许 tenant 请求长期持有 transaction；
- 所有写 transaction 设置时间和 payload 上限。

### 15.3 后台维护

- WAL checkpoint；
- KV expiration GC；
- Queue dead/retention cleanup；
- Workflow terminal retention；
- orphan resource file reconciliation；
- deleted resource trash cleanup；
- bundle cache eviction；
- SQLite integrity sampling 和容量 metrics。

## 16. Resource 生命周期

SQLite catalog 和独立文件不能构成一个跨文件原子 transaction，因此采用显式状态机：

```text
creating -> ready -> deleting -> tombstoned
```

### 16.1 创建 KV/D1

1. 在 `control.sqlite` 创建 `creating` resource 和 immutable ID；
2. 在目标目录创建临时 SQLite；
3. 应用 schema、fsync 并 atomic rename 到 `<resource-id>.sqlite`；
4. 把 resource 改为 `ready`；
5. 启动恢复器清理或完成遗留 `creating` resource。

### 16.2 删除

1. 检查 active deployment referrer；
2. 将 resource 标记为 `deleting`；
3. 阻止新 binding 和新连接；
4. drain 并关闭现有连接；
5. 将文件移动到同 volume 的 trash 目录；
6. 标记 tombstone；
7. 后台执行不可恢复的物理删除。

同名重建必须生成新 ID 和新文件，不能复用 tombstone 的物理身份。

## 17. 备份与恢复

### 17.1 Resource backup

单个 KV 或 D1 使用 SQLite Online Backup API 或 `VACUUM INTO` 生成一致 snapshot，再上传
到 S3。不能在数据库打开且 WAL 未处理时只复制主 `.sqlite` 文件。

### 17.2 全平台 snapshot

跨多个 SQLite 文件无法获得无协调的全局一致 snapshot。完整灾备流程应：

1. 进入 maintenance mode；
2. 停止新 public/control mutation；
3. 暂停 scheduler claim；
4. drain workerd 中的 DO 和正在执行的 dispatch；
5. checkpoint 并 snapshot control、scheduler、KV、D1 和 DO localDisk；
6. 生成带 schema version、checksum 和文件清单的 manifest；
7. 上传 S3；
8. 恢复服务。

R2 object、bundle 和 assets 已在 S3，不重复复制。manifest 保存 provider identity 和 object
prefix，但不保存 credential。

恢复默认是整套恢复。部分资源 restore 必须生成新 physical resource ID，避免旧 binding
和新数据意外混用。

## 18. API 范围与 vinext 对齐基线

### 18.1 平台能力范围

下表是产品边界，不是“全部已经实现”的声明。Next.js/vinext 的完成标准由第 18.2–18.4 节定义，
不能仅根据产品 registry 中存在同名 capability 判定通过。

| 产品 | 目标能力 | 不扩展为 |
| --- | --- | --- |
| Workers | modules、fetch、scheduled、bindings，以及 vinext 实际需要的 Node API | Edge placement、与 vinext 无关的完整 Node compatibility matrix |
| Static Assets | 不可变资源集、`ASSETS.fetch()`、路由、HTTP 响应与发布一致性 | Cloudflare 全球 CDN 基础设施 |
| Service Binding | 默认/具名入口、跨 Worker 与自绑定、原生 fetch/RPC | 完整 Cloudflare 部署管理面 |
| 框架缓存 | vinext 缓存适配器使用的 API、缓存命中/失效与隔离语义 | 全球缓存复制、Cloudflare 运维和计费系统 |
| 图片与字体 | vinext 已支持、用例实际调用的静态产物与图片处理路径 | vinext 尚未实现的完整 Next.js 构建时优化 |
| KV | get、getWithMetadata、put、delete、list、batch get | global cache/eventual consistency |
| D1 | prepare、bind、run、first、all、raw、batch、exec | replicas、bookmark、完整 admin API |
| DO | fetch/RPC、SQLite/KV storage、transaction、alarm | 跨节点迁移、完整 PITR；WebSocket hibernation 后置 |
| R2 | head、get、put、delete、list、range、条件请求 | multipart、完整 checksum/SSE-C |
| Queues | send、sendBatch、batch consume、ack/retry、delay、DLQ | pull consumer、严格顺序、exactly-once |
| Workflows | create/status、step.do、sleep、sleepUntil、waitForEvent、sendEvent、retry | 全部管理 API 和 Cloudflare limits parity |

兼容层必须维护独立 conformance suite。测试针对 API shape 和本文承诺的行为，不把“能运行
一个 demo”当作兼容完成。

### 18.2 固定 vinext 基线，不追求完整 Next.js 兼容

2026-08-28 本次文档勘察基线为 vinext repository commit
[`5d0b53088c689b75d63672eab6ff66434afa5b3b`](https://github.com/cloudflare/vinext/tree/5d0b53088c689b75d63672eab6ff66434afa5b3b)，
该 revision 的 package manifest 标记 `vinext` 为 `1.0.0-beta.8`、`@vinext/cloudflare` 为
`1.0.0-beta.6`。这是已核对的源码基线，**不是已安装的依赖或已通过的测试结果**。

P3.0 必须固化完整验收元组：vinext commit、包版本与锁文件摘要、React/Vite/RSC 插件版本、
构建工具版本、浏览器版本、open-compute revision、workerd lock 摘要和 compatibility date/flags。
不使用浮动 `latest`；变更基线时重新审查新增/删除/改写的用例与 upstream exclusions，再重新验收，
不能通过换版本、删测试或扩大排除列表来隐藏平台失败。本仓库依赖仍由根 Bun workspace 管理。

验收对象是 **vinext 的实现和已声明行为**。官方 Next.js 文档可以帮助解释术语，但不是一份
额外的全兼容清单；不引入原版 Next.js 全套测试，也不要求修复所有 vinext 与 Next.js 的差异。
OpenNext 不是本阶段的替代基准。

上游 [Project status](https://github.com/cloudflare/vinext/blob/5d0b53088c689b75d63672eab6ff66434afa5b3b/README.md#project-status)
明确说明 Cache Components/PPR 尚有缺口。我们需要通过该基线已有的 PPR/缓存用例，包括
`ppr-impact-demo` 对应行为；不能把它写成“完整 Next.js PPR 已实现”，也不要求在此阶段补齐
上游没有实现的 partial shell、resume、prefetch 或其他 Cache Components 语义。

### 18.3 特性验收矩阵

以下是必须覆盖的分类，不是用来替代或筛选上游完整用例集的白名单。每类保留上游已实现范围内
的原始断言，并补充平台边界测试；上游尚未实现的行为按第 18.4 节单独记录。

| 特性 | 在 open-compute 上需要证明的行为 |
| --- | --- |
| SSR / Streaming SSR | App/Pages Router 服务端渲染、状态码/headers、流式到达顺序、背压和错误处理；不能缓冲完整响应伪装 streaming |
| CSR / Hydration | JS/CSS 加载、交互、客户端导航、刷新、浏览器历史与 hydration；不能只检查 HTML 或 HTTP 200 |
| RSC | server/client 边界、Flight 请求与响应、导航/prefetch、服务端信息不进入 client bundle；HTML 与 RSC 缓存不串用 |
| Server Actions | 表单与调用、参数/结果、redirect、错误和缓存失效，保持上游的请求校验与安全断言 |
| SSG / Static export | 构建产物、动态路由预生成、资源引用、basePath/trailingSlash、404 与静态站点交付 |
| ISR / Data Cache | 上游采用的 HIT/STALE/更新路径、时间与 tag/path 失效、并发重新生成、租户和部署隔离 |
| PPR / Cache Components | vinext 已实现的 `use cache`、预渲染与动态内容组合及已有 PPR 用例；边界明确标为 upstream partial，不扩大为完整 Next.js PPR |
| Routing / Middleware | App/Pages Router、Route Handlers、middleware/proxy、动态参数、重写/重定向、编码路径、cookies/headers 与错误页 |
| 资源 / Metadata | `public/` 和客户端 chunks、CSS、fonts、metadata、vinext 已支持的图片优化及相关响应 |
| Bindings / Context | 服务端组件、Route Handlers、Actions 使用 `cloudflare:workers` 的 env；Static Assets、Service Binding 与实际依赖的产品绑定 |
| 开发与构建 | vinext/Vite 多环境编译、开发服务/HMR、CLI 与配置行为；与生产请求执行分开取证 |

### 18.4 “vinext 用例全部通过”的定义

**完整清单。** 从固定 revision 的
[package scripts](https://github.com/cloudflare/vinext/blob/5d0b53088c689b75d63672eab6ff66434afa5b3b/package.json)、
[Vitest 配置](https://github.com/cloudflare/vinext/blob/5d0b53088c689b75d63672eab6ff66434afa5b3b/vite.config.ts)、
[Playwright 配置](https://github.com/cloudflare/vinext/blob/5d0b53088c689b75d63672eab6ff66434afa5b3b/playwright.config.ts)
与 [CI matrix](https://github.com/cloudflare/vinext/blob/5d0b53088c689b75d63672eab6ff66434afa5b3b/.github/workflows/ci.yml)
枚举 unit、integration、browser E2E、相关 package/scripts 测试，以及它们的 fixtures/examples。
记录 project、mode、browser、case identity 和来源；不能只运行 `cloudflare-workers` 一个 project，
也不能因为默认启动器选择了部分 project，就遗漏 Pages Router、生产构建、静态导出或 PPR。
未被自动化覆盖的目标示例另补真实运行 smoke，不能代替已有断言。

**分层执行，不缩小范围。** 上游工具链、开发服务、CLI 和 Node standalone 打包测试留在其
对应构建/对照环境；这不要求生产引入 Node 服务。所有应用运行行为另映射到经过生产构建、
正常部署 API、真实 `platformd -> stock workerd` 路径的测试，包括原上游以 Node `start`
运行的 SSR/CSR/RSC/PPR 应用场景。不能把它们全部标成“Node 专属”来跳过平台验证。
原本使用 Miniflare/Wrangler 的用例，平台验收时由测试启动器连接实际 open-compute。

适配只允许改变构建/启动、部署、资源准备、base URL 等宿主连接点，并保留可审查的映射；不能
改应用输出、fixture 业务逻辑、预期结果、断言强度或安全策略。上游 `VINEXT_E2E_BASE_URL`
只在其已实现的 project 使用，其他 project 必须显式接入测试启动器，不能假设该变量全局生效。
默认端口、Worker 名称、用例 ID 和结果只存在于测试夹具，不能进入生产分支。

**通过与上游限制分开。** 固定基线全部启用、预期通过的用例必须执行并通过，平台新增失败、
skip、fixme、todo、未运行或无法准备环境均阻塞交付。上游已有的 skip/fixme/todo/expected-failure
逐项记录来源、原因及基线对照状态，不能计入 PASS，也不能用“上游存在缺口”豁免本平台新增问题。
正确拒绝不支持输入的负向用例按原断言验收。未由上游声明、但对照运行实际失败的项先报告阻塞，
不能临时加入上游限制表。只有明确属于 vinext 未实现范围的能力，才不要求额外补成官方 Next.js 行为。

**原始证据。** 报告同时列出 discovered、executed、passed、failed、upstream-excluded、
blocked/not-run，以及每个 project/mode/browser 的结果和日志位置；分片合并检查重复与遗漏，
总量来自测试发现，不硬编码预期数量。记录原始失败与重试，不能只保留一次成功重跑。
上游对照结果和平台结果分列，不能把 Node/Miniflare 的绿灯写成平台 PASS。

**当前状态。** 尚未生成完整用例清单，未运行上游对照或平台 vinext suite，因此总数与通过率
均为未测，不能引用上游宣传比例或已有 P0–P2 结果代替。

## 19. 安全边界

- workerd tenant outbound 只允许 public network；
- internal Unix socket 和 control endpoint 不进入 tenant env；
- 每个 binding adapter 使用 immutable scoped props；
- tenant code 不接触 SQLite path、S3 credential 或 generic internal Fetcher；
- D1 使用 SQLite authorizer 和 statement/result budget；
- Worker bundle 在 deploy 时校验大小、module graph 和 metadata；
- secret 加密后存储，日志统一 redact；
- resource ID 和文件路径只能由平台生成；
- public gateway 移除所有平台私有 request/response header；
- control API 需要独立认证，不能依赖 tenant Worker；
- S3 system prefix 与 virtual R2 prefix 必须逻辑隔离。

## 20. 故障语义

| 故障 | 恢复行为 |
| --- | --- |
| `platformd` crash | SQLite WAL recovery；过期 Queue/Workflow/alarm lease 重新 claim |
| workerd crash | supervisor 重启；下一次请求按 immutable version cold load |
| dispatch response 丢失 | 非幂等 HTTP 不自动 replay；Queue/Workflow 等 lease 到期后 at-least-once replay |
| S3 暂时不可用 | 已加载 Worker 可继续；cold load、R2 和新 bundle deploy 失败并返回可重试错误 |
| KV/D1 文件损坏 | 隔离到单个 resource；平台继续服务其他文件，resource 标记 degraded |
| `scheduler.sqlite` 损坏 | Queue/Workflow/alarm 不可用；不能从 control metadata 完整重建 |
| `control.sqlite` 损坏 | 平台不能安全解析 route/binding；整体 fail closed |

单节点意味着主机和本地 volume 仍是故障域。S3 snapshot 是灾备，不是 active-active replication。

## 21. 可观测性与运维

最低要求：

- `/health/live`：主进程存活；
- `/health/ready`：control/scheduler SQLite、workerd 和 S3 preflight 状态；
- request/dispatch ID；
- Worker execution count、duration、CPU/error；
- workerLoader cache hit/miss/cold-load time；
- SQLite busy、transaction duration、WAL size、checkpoint result；
- Queue depth、oldest message age、retry/DLQ；
- Workflow ready/due/running/failed、lease recovery；
- DO active facets、alarm lag；
- S3 latency/error；
- resource bytes 和 data volume free space。

Operator 命令至少包括：

```text
platform doctor
platform start
platform deploy
platform resources list
platform backup create
platform backup restore
platform gc
```

## 22. 按依赖顺序实施

当前新增工作的交付主线为 P3；P0–P2 是平台基础和回归约束，不是 vinext 的完成证明：

```text
P0：Workers + KV + R2 + D1 + Durable Objects
P1：P0 兼容性、可靠性和运维加固
P2：Queues + Cron + Workflows
P3：vinext 全量对齐与真实平台验收
```

以下 P0–P2 分解与具名验证记录保留其历史用途，不要求重新实现已有能力，也不要求保留旧格式、
旧升级链或 V1/V2 双引擎。当前架构按 Day1 直接收敛，尚未清理的源码不因文档中的阶段描述获得豁免。

产品优先级不等于源码目录顺序。基础能力依赖图为：

```text
workerd pin + platformd supervisor
        │
        ├── control.sqlite + resource lifecycle
        ├── S3 preflight + immutable artifact store
        └── internal transport
                    │
                    ▼
          workerLoader runtime
                    │
          deploy/version/route/fetch
                    │
                    ▼
        scoped binding adapter framework
          │         │         │         │
          ▼         ▼         ▼         ▼
         KV        R2        D1      DO core
                                        │
                                        ▼
                              generic scheduler kernel
                                        │
                         ┌──────────────┼──────────────┐
                         ▼              ▼              ▼
                     DO alarms       Queue/Cron     Workflows
```

`DO core` 不依赖 Queue 或 Workflow；只有 DO alarm 依赖 scheduler。P0 只实现 alarm 所需的
最小通用 scheduler kernel，P2 再扩展公平调度、批量 claim、Queue retry/DLQ 和 Workflow
durable execution。

### G0：架构可行性 Gate

G0 不交付产品能力，只用真实 workerd 做最小验证：

1. 固定 workerd 版本；
2. `workerLoader` 加载 immutable module Worker；
3. deployment A、B 同时存在，promotion/rollback 不依赖 cache invalidation；
4. loaded Worker 通过 JSRPC 调用一个 binding-scoped fake adapter；
5. 动态 DO class 可以通过 `getDurableObjectClass()` 作为 native facet 运行；
6. facet SQLite 写入在 workerd restart 后仍然存在。

G0 已于 2026-08-23 得到 **Conditional Go**：所有 hard gate 在三轮 fresh-process matrix 中
通过；唯一未通过项是 client disconnect 不会可靠触发 loaded Worker 的
`request.signal.aborted`。该限制不阻塞 P0，但 P0 不能依赖断连完成 tenant execution
cancellation，仍需使用 CPU、memory、subrequest 和 wall deadline 做资源边界。

任意 hard gate 回归失败都应先调整 runtime 架构，不能继续 P0 control plane 实现。

详细的工作包、黑盒测试、故障注入与 Go/No-Go 标准见
[G0：workerd 动态运行时可行性验证](./implemented/g0-workerd-runtime-validation.md)；实际 pin、三轮矩阵、
已接受限制与最终 verdict 见 [G0 results](./implemented/g0-results.md)。

### P0.1：Platform foundation

最先实现所有后续能力共同依赖的宿主层：

- `platformd` 启动、监督和重启 workerd；
- Unix socket 或 loopback internal transport；
- `control.sqlite`、schema migration 和 resource ID；
- 数据目录、文件权限和 master key 加载；
- S3 preflight；
- immutable bundle/assets object store 和 local cache；
- `/health/live`、`/health/ready` 和基础 metrics。

完成门槛：空目录首次启动、重复 migration、S3 故障、workerd crash/restart 和损坏配置
均有确定行为。

详细的进程边界、数据目录、migration、master key、S3/artifact、supervisor、health、工作包与
测试 Gate 见 [P0.1：Platform Foundation 详细设计](./implemented/p0-1-platform-foundation.md)。

### P0.2：Workers runtime

在 foundation 上实现第一条完整请求路径：

1. Worker create/deploy；
2. bundle validation 和 immutable deployment；
3. active route；
4. `workerLoader` cold/warm load；
5. HTTP fetch dispatch；
6. vars 和 encrypted secrets；
7. promotion、rollback、retention 和 delete。

这一阶段不接真实产品 binding，只验证无 binding Worker 和 fake adapter。完成门槛是
deployment A -> B -> rollback A、process restart 和 invalid deploy 不影响 active version。

详细的 schema、bundle 格式、部署状态机、RuntimeSource、loader callback、route/stream、
public-only egress、promotion/delete、工作包与测试 Gate 见
[P0.2：Workers Runtime 详细设计](./implemented/p0-2-workers-runtime.md)。

### P0.3：Resource 与 binding framework

所有产品 binding 共用这一层：

- `creating -> ready -> deleting -> tombstoned` resource lifecycle；
- deployment binding 冻结 physical resource ID；
- binding-scoped immutable props；
- JSRPC host adapter factory；
- request/result byte budget；
- tenant-safe error mapping；
- connection/cache lifecycle；
- resource referrer 和 deletion fence。

只有这层稳定后才开始分别实现 KV、R2、D1 和 DO，避免四套 binding 各自发明 transport、
鉴权和错误协议。

详细的 typed schema、descriptor、deploy integration、BindingBackend、resource lifecycle、
work packages 与真实 workerd Gate 见
[P0.3：Resource 与 Binding Framework 详细设计](./implemented/p0-3-resource-binding-framework.md)。

### P0.4：KV

KV 是最简单的持久化 adapter，用来验证 resource、JSRPC 和动态 SQLite 文件的组合：

- 一 namespace 一 SQLite；
- `get`、`getWithMetadata`、`put`、`delete`、batch get 和 `list`；
- TTL、opaque cursor 和 expiration GC；
- LRU connection manager；
- namespace backup/restore。

完成门槛包括 namespace 隔离、UTF-8 list 顺序、并发写、restart、同名重建和单文件损坏
隔离。

详细的 Workers KV compatibility matrix、rowid/incremental BLOB schema、streaming、TTL、cursor、
LRU/WAL、backup/restore、工作包与测试 Gate 见 [P0.4：KV 详细设计](./implemented/p0-4-kv.md)。

### P0.5：R2

R2 先补一层 R2/D1 共用的 loaded-isolate facade framework，再复用 foundation 中已经验证过的
S3 client 增加 tenant-facing virtual bucket：

- injected local facade + deterministic main wrapper Gate；
- logical bucket lifecycle；
- physical prefix isolation；
- `head/get/put/delete/list`；
- streaming、range、metadata 和 conditional request；
- S3 错误分类和物理信息隐藏。

完成门槛包括 logical bucket 隔离、`system/` prefix 不可达、stream cancel 和 S3 timeout/
5xx。Multipart 不阻塞 P0。

详细的 loaded-isolate facade、S3 typed store、provider capability preflight、control schema、object
metadata/key budget、读写路径、工作包和测试 Gate 见 [P0.5：R2 详细设计](./implemented/p0-5-r2.md)。

### P0.6：D1

D1 复用 resource lifecycle、SQLite manager 和 binding framework：

- 一 database 一 SQLite；
- loaded-isolate D1 facade + flat scoped transport；
- `prepare/bind/run/first/all/raw`、`batch` 和 `exec`；
- migrations；
- SQLite authorizer；
- statement、row、result byte 和 execution time limits；
- database backup/restore。

完成门槛包括跨 database 隔离、`ATTACH`/extension/filesystem pragma 拒绝、transaction
rollback、WAL recovery 和 commit 后 response 丢失的 `result-unknown` 行为。

详细的 facade architecture decision、SQLite schema/authorizer/limits、statement/batch/exec、migration、
backup/restore、工作包和测试 Gate 见 [P0.6：D1 详细设计](./implemented/p0-6-d1.md)。

### P0.7：Durable Object core

DO 依赖 Workers runtime、immutable version、binding framework 和 workerd localDisk：

1. namespace/class binding 和 object ID；
2. dynamic class/facet dispatch；
3. fetch 和 RPC；
4. native SQLite/KV storage；
5. transaction、input/output gate 和 `deleteAll()`；
6. `do_storage_id` 生命周期；
7. deploy restart policy；
8. basic WebSocket；
9. delete/recreate storage isolation。

先完成普通 fetch/RPC 和 storage，再实现 deployment lifecycle 和 WebSocket。完成门槛是
相同 object 串行化、不同 object 并行、workerd restart、promotion/rollback 和
delete/recreate。

详细的 production native-facet Gate、一 object 一 host actor、64-hex ID、loaded-isolate facade、
deployment generation fence、localDisk ownership、delete reconciliation、basic WebSocket 和测试矩阵见
[P0.7：Durable Objects 详细设计](./implemented/p0-7-durable-objects.md)。

### P0.8：Scheduler kernel 与 DO alarms

DO alarm 引入后续 Queue/Workflow 共同需要的最小 scheduler primitive：

- deterministic clock；
- due index；
- claim lease；
- random run token；
- conditional commit；
- expired lease recovery；
- bounded dispatch concurrency；
- shutdown drain。

在此之上实现 `getAlarm()`、`setAlarm()`、`deleteAlarm()`、object SQLite authority、
`scheduler.sqlite` projection、row token、read repair 和 at-least-once dispatch。

P0 不在这里预建 Queue/Workflow 通用抽象；只提取已经被 alarm 实际使用的 scheduler
primitive。

详细的 facet alarm shim Gate、object SQLite authority、`scheduler.sqlite` due projection、claim
lease、row/claim token、repair、at-least-once delivery、六次 retry 和 failure matrix 见
[P0.8：Scheduler Kernel 与 DO Alarms 详细设计](./implemented/p0-8-scheduler-do-alarms.md)。

### P0 Exit Gate

> 验证状态（2026-08-26）：已由 `crates/service/tests/p0_exit_gate.rs` 的单一真实
> pinned-workerd fixture 覆盖，并通过 `test/test-p0-exit.sh` 三轮 fresh-process 综合矩阵；
> 该入口随后递归执行 P0.8-P0.2 与 P0.1 全部 regression Gate。

一个 fixture Worker 必须能同时使用：

```text
HTTP Worker
├── KV
├── R2
├── D1
└── Durable Object
    ├── fetch/RPC
    ├── SQLite/KV storage
    ├── alarm
    └── basic WebSocket
```

整套测试覆盖 deploy A -> B -> rollback A、platform/workerd restart、资源隔离、单资源
损坏、S3 故障以及 P0 resource backup/restore。

综合 fixture 通过 control API 创建两组隔离 KV/R2/DO、三组 D1，使用同一 immutable deployment
同时执行 KV typed get/put/list/metadata/stream、R2 put/head/get/range/list/metadata/delete、D1
prepare/bind/run/all/first/raw/batch/session/migration、DO fetch/RPC/sync+async storage/transaction/alarm/
WebSocket。KV 与 D1 online backup restore-as-new 后由 deployment B 显式重绑，rollback A 再验证原
resource 未被原地覆盖；测试还直接 SIGKILL workerd、重启完整 platform owner composition，并保证
重启后的第一个 tenant event 可以是 cold DO alarm，而不依赖普通 fetch 预热。

### P1：P0 加固

P1 不增加新的 Cloudflare 产品能力。它把已通过 aggregate Gate 的 P0 收敛成具备灾备与运维能力的
单机发行版，按依赖顺序完成：

1. capability/format freeze 与 P0 API conformance；
2. resource quota、统一磁盘 admission 和 offline data-dir ownership；
3. 短维护窗口下的 platform snapshot 与 fresh-host restore；
4. 当前 schema/snapshot 完整性、离线恢复与协调的 workerd pin 更新；不保留旧开发版本升级路径；
5. security fuzzing、恶意 Worker 和跨 account/resource isolation；
6. soak、load、crash-point/fault matrix 和 capacity envelope；
7. production health、metrics、doctor、support bundle 和 runbook；
8. advanced WebSocket hibernation 的 pinned stock-workerd 条件性 Gate。

P1.0 至 P1.7 是进入 P2 的必过 Gate；P1.8 hibernation 可以是 Go、Conditional Go 或 No-Go，不能
阻塞核心稳定性发布。整机 snapshot 不重复复制 R2/bundle 所在的外部 S3，不包含 master key，恢复要求
同一 S3 authority、同一外部 master key 和当前实现支持的 schema；它恢复本地 authority，但不是
R2 point-in-time backup，R2 使用 restore 时外部 provider 中的当前状态。

详细的离线 snapshot/restore format、升级/回滚协议、磁盘 admission、安全与长稳矩阵、运维合同、
工作包和 Exit Gate 见 [P1：P0 平台加固详细设计](./implemented/p1-platform-hardening.md)。

### P2.1：Scheduler hardening

先把 P0.8 已由 alarm 验证的单 workload loop 收敛成多 workload 内核，但 production 仍只注册
Alarm。内核使用 global budget + Alarm/Queue/Cron/Workflow 独立 pool、work-conserving weighted
fairness、pool-local batch claim、generation/token/lease fence、event wake + earliest deadline、完整 virtual
clock、基础设施 backoff/jitter 和只存在于 test-support binary 的 crash point。业务 authority 仍归每个
产品，禁止建立一张 nullable-column 的通用 jobs 表；P2.1 也不创建 Queue/Cron/Workflow 业务 row。

详细的 workload contract、fairness、wake/lost-wake 协议、migration registry、故障隔离、工作包、
测试矩阵与 Exit Gate 见
[P2.1：Scheduler 多 Workload 内核详细设计](./implemented/p2-1-scheduler-hardening.md)。

### P2.2：Queue producer

实现 Queue lifecycle 与 immutable producer binding，向普通 Worker 暴露 `send`、`sendBatch` 和
`metrics`。Queue catalog/producer binding 使用 control migration 009 的独立表，不关闭 FK 重建 P0 已
冻结的 `resources` 引用图；durable message authority 使用 scheduler migration 002 的
`queue_state/queue_messages`。一个 batch 在一笔 SQLite transaction 中全成或全败，支持 JSON/text/bytes、
Queue/batch/message delay、128,000-byte 单消息、100 条/256,000-byte batch、restart/snapshot persistence、
retention、quota 与跨 account isolation；V8 明确不支持。

实现前必须在 pinned stock workerd 上验证动态 facade 是否继承 Durable Object output gate。若普通
Worker 可用但 DO output gate 不成立，则以 Conditional Go 发布：普通 Worker producer 开放，DO 内
Queue producer 稳定 fail closed，不能静默提早 enqueue。

详细的 control/scheduler schema、跨库 lifecycle、facade/transport、serialization、durability、
output-gate Hard Gate、工作包、测试矩阵与 Exit Gate 见
[P2.2：Queue Producer 详细设计](./implemented/p2-2-queue-producer.md)。

### P2.3：Queue consumer 与 Cron

按依赖拆成两个可单独验收的交付单元。先实现每 Queue 一个 active push consumer：immutable
deployment declaration、live attachment、batch size/timeout、frozen consumer deployment/generation、
per-message/batch ack/retry、Known/Unknown dispatch 分类、max retries、原子 DLQ intake、pause/update
drain 和三层 concurrency。再实现 UTC-only 五字段 Cron：deployment `inherit/replace` 语义、activation
handoff、next-run projection、logical slot dedup、bounded misfire 与 native scheduled custom-event dispatch。

Control 使用 migration 010/011，scheduler 使用 migration 003/004。Queue crash matrix 必须覆盖
insert、claim、dispatch、handler、ack 和 DLQ move 每个事务边界；Cron 必须覆盖 schedule advance、
slot insert、promotion handoff 和 wall-clock jump。两者都承诺 at-least-once，不承诺 exactly-once。

详细的 Hard Gate、API/config、control/scheduler schema、claim/completion transaction、DLQ backpressure、
Cron parser/slot/misfire、reconciler、工作包、测试矩阵与 Exit Gate 见
[P2.3：Queue Consumer 与 Cron 详细设计](./implemented/p2-3-queue-consumer-cron.md)。

### P2.4：Workflow core

Workflow 在 Queue 验证 scheduler lease 和 crash recovery 后实现。Pinned stock workerd 继续通过
dynamic `workerLoader` 提供隔离、immutable deployment 和 bindings，但完整 Workflow engine 由
platformd trusted facade + `scheduler.sqlite` 实现，不假设 workerd 内建 Cloudflare control plane。

P2.4 只实现 logical definition/immutable version、caller binding、instance `create/get/status`、冻结
Worker deployment/class、`WorkflowEntrypoint.run()`、顺序 `step.do()`、bounded canonical JSON、step
result persistence/replay、generation/run-token/step-token fence、terminal success/error 和 live instance
deployment referrer。Callback side effect 是 at-least-once；completed step replay 不再执行 callback。

Control 使用 migration 012，scheduler 使用 migration 005。Retry、sleep、event、modifier、retention、
parallel step 和完整 RpcSerializable 明确留给后续阶段。

详细的 Runtime/DO output-gate Hard Gate、schema、跨库 create saga、step identity/replay、JSON quota、
terminal/referrer、crash matrix、工作包与 Exit Gate 见
[P2.4：Workflow Core 详细设计](./implemented/p2-4-workflow-core.md)。

### P2.5：Workflow durable waiting

这一阶段的实现记录使用 Workflow capability V2 扩展 durable waiting。当前 Day1 目标是将这些
能力收敛为唯一 Workflow 实现，不保留旧 instance/version/binding 的平台引擎兼容分支；现有双路径
清理见第 2.4 节，不能将“目标唯一”误记为已经清理完成。继续使用一个 platformd、一个 pinned
workerd 子进程，以及 control/scheduler 两个 SQLite authority，不新增 Redis 或独立 Workflow 服务。

按依赖逐项增加：

1. Runtime Hard Gate：suspension 资源释放、可信 timeout、system-isolate token 隔离；
2. 当前 Workflow schema、规范 descriptor 与 replay identity；013/014、006/007/008 等编号仅记录开发阶段实现，不要求历史数据库升级兼容；
3. 公共 durable yield/wake/recovery 与 activation budget；
4. step retry/backoff、per-attempt timeout、NonRetryableError；
5. `step.sleep` / `step.sleepUntil`；
6. `step.waitForEvent` / `sendEvent`、event-before-wait、FIFO 与 timeout 原子裁决；
7. pause/resume/terminate、保持 frozen target 的 restart saga；
8. retention、typed referrer 保留/释放、跨库清理和 instance ID 重用；
9. 有界同步 fan-out 的 parallel `step.do`；
10. Aggregate Gate 与完整 P2 黑盒链路。

Waiting/paused 不占 run lease 或执行槽位；V2 terminal 在 retention 内继续保留 artifact 引用，支持
原版本 restart。Parallel 最后实现，限整体 join 的 do batch，不开放并行 sleep/event 或任意 Promise DAG。
P2.4 已验证的 DO output-gate 限制继续保留：DO 内 Workflow mutation fail closed，只读 get/status 可用。

详细的 capability/API、SQLite migration、wait/retry/event 状态机、restart/retention saga、parallel
边界、工作包、crash matrix 与 Exit Gate 见
[P2.5：Workflow Durable Waiting 详细设计](./implemented/p2-5-workflow-durable-waiting.md)。

2026-08-28 已完成 P2.5 Conditional Go 与 P2 Exit 三轮验收；Rust 行覆盖率为 90.16%。
实际支持面、逐轮结果和保留限制见 [P2.5 / P2 Exit 验证记录](./implemented/p2-5-gate-results.md)。

### P2 Exit Gate

最终黑盒链路：

```text
HTTP -> Queue -> Consumer -> Workflow
                         ├── KV/D1/R2
                         └── DO RPC/alarm
```

在每个 transaction/dispatch 边界注入 process crash，要求 Queue 不丢消息、Workflow
拒绝 stale commit、冻结版本正确、所有 due work 在 restart 后恢复。

### P3.0：vinext 基线与用例清单

- 固化第 18.2 节的验收元组，发现全部上游测试、project/browser/mode 与关联示例，生成逐项映射；
- 在独立对照环境运行上游基线，记录实际结果与上游已有排除项；不新增本平台豁免；
- 分清工具链测试与应用运行测试，为后者接入真实 platformd 的测试启动器；
- 盘点每个 fixture 需要的模块类型、Node API、bindings、缓存、图片、配置与资源限额；
- 测试依赖、浏览器及 runtime 必须预置或显式授权准备，不由生产启动或缺依赖的 Gate 隐式下载。

交付物是可复现基线、完整清单和差距报告，不是“vinext 已支持”。上游有测试文件不等于该用例在
所选模式已经启用或通过；上游暂缺的完整 Next.js 特性不加入本平台新增需求。

### P3.1：框架构建产物与 Static Assets

沿用 vinext/Vite 的 RSC、SSR、client 多环境构建，不用普通 Worker 单入口打包替代框架编译。
TS7 检查本项目维护的 TypeScript；构建、模块/资源校验成功后才允许部署。产物导入必须保存
入口、模块图、静态资源、路由配置与所需 bindings，保持 server-only 与 client 输出隔离。

按第 7.5、11.3 节实现资源上传、manifest、路由与 `ASSETS.fetch()`，支持仅有静态产物的部署，
不为 static export 注入伪造的租户 Worker。代码与资源一起 ready/promote/rollback；缓存与
保留策略覆盖资源，不通过历史 manifest 搜索或把全部资源内嵌 JS 绕过产物模型。

阶段验证覆盖 App/Pages Router 构建、静态导出、真实 JS/CSS 加载和 hydration，以及路径编码、
上传中断、摘要损坏、缺失资源、部署切换和跨账户读取拒绝。这只是中间里程碑，不是全量验收。

### P3.2：Service Binding、Node API 与流式执行

实现默认/具名入口、跨 Worker 和自绑定的原生 fetch/RPC，目标按 authority 解析并固定单次调用。
与 assets 路由复用正确入口；不为 `WORKER_SELF_REFERENCE` 或特定框架名称写专用生产分支。

根据完整用例清单验证 Node compatibility、`cloudflare:workers` env、模块格式、异步上下文、
Request/Response/Streams、Server Actions 与 RPC。当前 CPU、子请求、module/artifact 限额通过
真实产物和负载测试调整为明确的产品预算；不能取消预算或只给测试 fixture 放宽限制。

验证 SSR/Flight 分块输出、取消与背压、错误页、递归调用、`waitUntil`、pin 释放、warm/cold
一致性和 restart。既有 G0 `D-abort` 限制只按原精确边界记录，不自动豁免任何 vinext 用例失败。

### P3.3：vinext 缓存与图片依赖

使用基线实际采用的接口盘点，而非假设“有 R2/Queue 就支持 ISR”。本次源码基线包含
`kvDataAdapter()`、基于 Workers Cache 的 `cdnAdapter()` 和 `imagesOptimizer()`；需要验证
KV 数据缓存、`ctx.cache`/tag purge 与响应缓存的协作，以及实际使用的 Images 能力。
普通 Cache API 与 Workers Cache 不能仅因名称相似就视为同一接口。

平台实现单节点的相应语义，持久状态归 SQLite/S3，租户只获得声明的能力。缓存 key、HTML/Flight
变体、tag/path 失效、并发重新生成及部署边界必须隔离；缺少已配置的后端应报错，不能悄悄退回
进程内缓存、关闭 ISR/PPR 或设为图片不优化来让用例通过。上游只在测试中使用的 mock 保留在
测试/对照层，不能移入生产后端代替持久化或安全边界。

PPR/Cache Components 按 vinext 的既有实现验收，不补齐其 README 已列明的 Next.js 缺口。
发现其他基线用例依赖的 binding 时纳入同一盘点与实现，不将本节列出的三个适配器作为范围上限。

### P3.4：全量对齐、隔离与恢复

完成所有启用用例的逐项映射，覆盖上游 unit/integration 与全部浏览器 project/mode。
应用行为在真实平台生产产物上复验；开发/HMR 与宿主工具行为单独取证，不能互相替代。
除原始断言外，补充两个账户/部署并发、secret 与 client bundle 隔离、资源/缓存交叉读取拒绝、
异常清理、部署回滚、S3 故障、workerd/platformd crash 与恢复测试。

能力按领域组织：构建接入在 `packages/toolchain`，部署/目标解析在 `crates/workers`，schema
在 `crates/storage`，字节与校验在 `crates/artifacts`，入口组合在 `crates/service`，资源和服务
调用分别进入 `runtime/src/assets/`、`runtime/src/services/`。缓存/图片按实际所有权划分，不能
把框架业务分支或测试条件堆入 loader host；生成 JS 由构建更新，不手改。

### P3 Exit Gate

按 [测试节奏](./references/testing.md)，开发期间每次迭代只跑相关单轮。源码冻结后才执行完整上游基线、
全量平台矩阵与相关最终三轮真实运行时 Gate；平台 P3 三轮使用新进程和隔离数据目录，覆盖该轮
正常运行与重启恢复。新增 runner 尚待实现，本节不是已有可执行命令或已通过报告。

执行完整 Rust/TS 检查、依赖边界和既有 coverage 要求，Rust 行覆盖率不得低于 90.00%。相关
G0/P0/P2 回归按影响范围执行，不在每个中间步骤递归重跑所有历史 aggregate。
最终报告按第 18.4 节列出基线、全部 case 映射、各轮原始结果、上游限制、失败与清理证据。
中间 SSR smoke 通过、上游 CI 绿灯或某个 browser project 通过都不能替代 P3 Exit。

## 23. 主要风险

### 23.1 workerd internal API 稳定性

`workerLoader` 和 native facet 能力是方案的核心。必须固定 workerd version，通过升级测试后
才能变更，不能自动追随 latest。

### 23.2 Durable Object 动态执行

DO 的 class loading、facet identity、SQLite、alarm、restart 和 WebSocket 相互耦合，是
整套系统最复杂的部分。应复用 WDL 已验证的结构，而不是重新发明一个普通 actor framework。

### 23.3 Workflow replay

Workflow 不是“带 retry 的 Queue”。step identity、并行 step、sleep/event、外部副作用、
lease expiry 和 version retention 都需要独立状态机与大量 crash-point 测试。

### 23.4 SQLite 单 writer

`scheduler.sqlite` 是最早出现的整体吞吐瓶颈。对于 SMB 足够，但若目标变成高吞吐 Queue
平台，需要 scheduler sharding 或 Postgres；这不应偷偷加入 V1。

### 23.5 全局备份

多个 SQLite 加 S3 无法天然获得单一原子 snapshot。V1 必须接受短 maintenance window，
不能声称在线全局一致备份。

### 23.6 vinext 基线与上游缺口

vinext 仍在快速变化，PPR/Cache Components 等范围不能根据功能名称推断。以固定 revision 的
实现、测试和已知限制为准，升级时审查差异；不承诺完整 Next.js，不把上游限制记成本平台 PASS，
也不能借上游 beta 状态接受平台新增回归。

### 23.7 框架产物与执行预算

普通 Worker 已通过不代表能承载 vinext 的模块图、client assets、RSC 流和缓存请求。
需要实测包大小、冷启动、CPU、内存、子请求、并发生成、缓存命中/失效和浏览器行为。
若当前 pin 或预算无法支持基线，应报告平台差距并协调修复，不能以 Node sidecar、mock binding
或关闭框架特性代替目标执行路径。

## 24. 验收门槛

### 24.1 平台基础约束

以下基础能力继续作为回归约束，但单独满足它们不能宣布新目标完成：

1. 同一 Worker 可以声明并使用 KV、D1、R2、DO、Queue 和 Workflow binding；
2. promotion 和 rollback 始终加载正确的 immutable Worker version；
3. KV namespace、D1 database 和 logical R2 bucket 可以创建多个并独立绑定；
4. 一个 KV/D1 的高写入或损坏不会阻断其他 resource；
5. Queue handler 在任意 claim、dispatch、ack crash point 后不丢消息；
6. Workflow 在任意 step commit crash point 后能够 replay，并拒绝 stale run commit；
7. DO storage 在普通 Worker deploy 后保持，delete/recreate 后不复用旧 storage；
8. DO alarm 在改期、删除、进程 crash 后不会由旧 token 错误触发；
9. D1 tenant SQL 无法 ATTACH 或读取其他数据库和平台文件；
10. S3 credential 和物理 prefix 不会暴露给 tenant Worker；
11. 完整 backup/restore 在全新主机上通过；
12. 一条命令或一次安装操作可以启动并通过 doctor/smoke。

### 24.2 当前交付目标：与 vinext 全量对齐

只有同时满足以下条件，才能宣布本阶段完成：

1. 固定且可复现的 vinext/工具链/浏览器/workerd 基线，以及无遗漏的上游用例与平台映射清单；
2. 全部上游启用、预期通过的用例按其原语义执行通过，平台新增失败、跳过、未运行和环境阻塞均为零；
3. SSR、CSR/hydration、RSC、Server Actions、SSG/ISR、PPR、路由、静态资源与绑定，分别具有
   vinext 已实现范围内的实际证据；PPR 不以完整 Next.js 语义追加范围，也不以普通 SSR 代替；
4. 所有应用运行场景经过正常构建、部署和真实 platformd/workerd 路径，浏览器断言实际执行；
5. Static Assets、Service Binding、缓存和图片依赖不是 mock，不以关闭特性或降级后端取得通过；
6. 多租户隔离、secret 边界、不可变部署、发布回滚和故障恢复通过，未弱化任何既有安全 Gate；
7. 上游既有 exclusions 与 Next.js 差异逐项披露，不计入通过数，不扩大为本平台新增豁免；
8. P3 最终三轮、相关平台回归与完整检查完成，报告包含原始结果、重试及未遗留进程/资源的证据。

结论应写为“与指定 vinext revision 的启用用例全部对齐，上游限制另列”，不能写成“完整兼容
Cloudflare Workers / 官方 Next.js”。当前未执行上述 vinext 验收，状态仍为未完成。

## 25. 参考资料

- [Cloudflare Next.js 指南（vinext 路径）](https://developers.cloudflare.com/workers/framework-guides/web-apps/nextjs/)
- [vinext 本次勘察基线](https://github.com/cloudflare/vinext/tree/5d0b53088c689b75d63672eab6ff66434afa5b3b)
- [vinext Playwright projects 与 PPR 用例入口](https://github.com/cloudflare/vinext/blob/5d0b53088c689b75d63672eab6ff66434afa5b3b/playwright.config.ts)
- [vinext Workers Cache 示例](https://github.com/cloudflare/vinext/tree/5d0b53088c689b75d63672eab6ff66434afa5b3b/examples/workers-cache)
- [Cloudflare Static Assets 配置与绑定](https://developers.cloudflare.com/workers/static-assets/binding/)
- [Cloudflare Service bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/)
- [WDL repository](https://github.com/wdl-dev/wdl)
- [WDL runtime loader](https://github.com/wdl-dev/wdl/blob/wdl.20260817.1/docs/modules/runtime.md)
- [WDL Queues and Cron](https://github.com/wdl-dev/wdl/blob/wdl.20260817.1/docs/modules/queues-cron.md)
- [WDL Workflows](https://github.com/wdl-dev/wdl/blob/wdl.20260817.1/docs/modules/workflows.md)
- [WDL Durable Objects](https://github.com/wdl-dev/wdl/blob/wdl.20260817.1/docs/modules/durable-objects.md)
- [Miniflare repository](https://github.com/cloudflare/workers-sdk/tree/main/packages/miniflare)
- [Miniflare workerd runtime supervisor](https://github.com/cloudflare/workers-sdk/blob/main/packages/miniflare/src/runtime/index.ts)
- [Miniflare worker-loader plugin](https://github.com/cloudflare/workers-sdk/blob/main/packages/miniflare/src/plugins/worker-loader/index.ts)
- [Miniflare D1 plugin](https://github.com/cloudflare/workers-sdk/blob/main/packages/miniflare/src/plugins/d1/index.ts)
- [WDL loaded-isolate R2 facade](https://github.com/wdl-dev/wdl/blob/main/runtime/r2-client.js)
- [WDL loaded-isolate D1 facade](https://github.com/wdl-dev/wdl/blob/main/runtime/d1-client.js)
- [WDL host-binding wrapper generator](https://github.com/wdl-dev/wdl/blob/main/runtime/load/wrapper-generate.js)
- [Miniflare Durable Objects plugin](https://github.com/cloudflare/workers-sdk/blob/main/packages/miniflare/src/plugins/do/index.ts)
- [workerd WorkerLoader](https://github.com/cloudflare/workerd/blob/main/src/workerd/api/worker-loader.h)
- [Cloudflare KV namespaces](https://developers.cloudflare.com/kv/concepts/kv-namespaces/)
- [Cloudflare KV list keys](https://developers.cloudflare.com/kv/api/list-keys/)
- [Cloudflare D1](https://developers.cloudflare.com/d1/)
- [Cloudflare SQLite-backed Durable Object storage](https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/)
- [Cloudflare Queues delivery guarantees](https://developers.cloudflare.com/queues/reference/delivery-guarantees/)
- [Cloudflare Workflows Workers API](https://developers.cloudflare.com/workflows/build/workers-api/)
- [SQLite WAL](https://www.sqlite.org/wal.html)
- [SQLite Online Backup API](https://www.sqlite.org/backup.html)
