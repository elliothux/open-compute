# SQLite-only workerd 平台方案

> 状态：Proposal
>
> 本文描述一个未来可替代 WDL 部署栈的自托管 Workers 平台。它不是当前 Lynx OS MVP
> 的已实现架构，也不修改 `GOAL.md` 中“使用未修改的上游 WDL、MVP 不依赖 Queues 和
> Workflows”的现有边界。正式实施前需要单独做产品和工程决策。

## 1. 结论

该方案在单机、SMB self-deploy 场景下可行。

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
- Cron Triggers 和 Durable Object alarms。

方案不追求 Cloudflare 的边缘调度、跨地域复制、多副本高可用和完整管理面行为。它提供的
是单节点、强本地一致性、可恢复、常用 API 兼容的运行环境。

结构化状态只使用 SQLite。对象字节、Worker bundle 和静态产物使用同一个外部
S3-compatible provider 的隔离前缀。平台不依赖 Redis、Postgres、Kafka 或独立网关。

## 2. 目标与非目标

### 2.1 目标

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

### 2.2 非目标

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

## 3. 部署单元

“单体服务”定义为一个可安装、启动和升级的部署单元，不强求只有一个 OS process。

推荐发布一个包含两个二进制的 bundle：

```text
platformd          Rust 主进程
workerd            固定版本的 upstream workerd
```

`platformd` 启动并监督 `workerd` 子进程。两者通过 Unix domain socket 或仅监听
loopback 的内部 HTTP 通信。外部只暴露 `platformd` 的 public/control 端口。

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
                │ Unix socket / loopback
                ▼
┌─────────────────────────────────────────┐
│ workerd                                 │
│                                         │
│  runtime worker        workerLoader     │
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
- Queue：claim batch 时冻结 consumer deployment 和 consumer generation；
- Workflow：instance 创建时永久冻结 deployment 和 class；
- DO：storage identity 跨普通 deploy 保持不变，新 facet 使用当前 deployment；
- DO alarm：保存 alarm 时记录目标 deployment，promotion policy 可以选择 preserve 或
  restart；V1 默认 restart，部署时关闭已有 WebSocket 并让新请求使用新版本。

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
│   └── bundle/<sha256>
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
边界以 [P0.3：Resource 与 Binding Framework 详细设计](./p0-3-resource-binding-framework.md)
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
隔离和完整 Gate 见 [P0.4：KV 详细设计](./p0-4-kv.md)。

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
[P0.6：D1 详细设计](./p0-6-d1.md)。

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
[P0.7：Durable Objects 详细设计](./p0-7-durable-objects.md)。

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
[P0.8：Scheduler Kernel 与 DO Alarms 详细设计](./p0-8-scheduler-do-alarms.md)。

## 11. R2 与 S3

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
system/assets/<deployment-id>/<path>
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
[P0.5：R2 详细设计](./p0-5-r2.md)。

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

## 18. API 兼容范围

| 产品 | V1 常用能力 | 暂不承诺 |
| --- | --- | --- |
| Workers | modules、fetch、scheduled、bindings、service RPC 基础 | Edge placement、完整 Node compatibility matrix |
| KV | get、getWithMetadata、put、delete、list、batch get | global cache/eventual consistency |
| D1 | prepare、bind、run、first、all、raw、batch、exec | replicas、bookmark、完整 admin API |
| DO | fetch/RPC、SQLite/KV storage、transaction、alarm | 跨节点迁移、完整 PITR；WebSocket hibernation 后置 |
| R2 | head、get、put、delete、list、range、条件请求 | multipart、完整 checksum/SSE-C |
| Queues | send、sendBatch、batch consume、ack/retry、delay、DLQ | pull consumer、严格顺序、exactly-once |
| Workflows | create/status、step.do、sleep、sleepUntil、waitForEvent、sendEvent、retry | 全部管理 API 和 Cloudflare limits parity |

兼容层必须维护独立 conformance suite。测试针对 API shape 和本文承诺的行为，不把“能运行
一个 demo”当作兼容完成。

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

交付优先级调整为：

```text
P0：Workers + KV + R2 + D1 + Durable Objects
P1：P0 兼容性、可靠性和运维加固
P2：Queues + Cron + Workflows
```

产品优先级不等于源码目录顺序。实际开发必须沿依赖图推进：

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
[G0：workerd 动态运行时可行性验证](./g0-workerd-runtime-validation.md)；实际 pin、三轮矩阵、
已接受限制与最终 verdict 见 [G0 results](./g0-results.md)。

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
测试 Gate 见 [P0.1：Platform Foundation 详细设计](./p0-1-platform-foundation.md)。

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
[P0.2：Workers Runtime 详细设计](./p0-2-workers-runtime.md)。

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
[P0.3：Resource 与 Binding Framework 详细设计](./p0-3-resource-binding-framework.md)。

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
LRU/WAL、backup/restore、工作包与测试 Gate 见 [P0.4：KV 详细设计](./p0-4-kv.md)。

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
metadata/key budget、读写路径、工作包和测试 Gate 见 [P0.5：R2 详细设计](./p0-5-r2.md)。

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
backup/restore、工作包和测试 Gate 见 [P0.6：D1 详细设计](./p0-6-d1.md)。

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
[P0.7：Durable Objects 详细设计](./p0-7-durable-objects.md)。

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
[P0.8：Scheduler Kernel 与 DO Alarms 详细设计](./p0-8-scheduler-do-alarms.md)。

### P0 Exit Gate

> 验证状态（2026-08-26）：已由 `crates/service/tests/p0_exit_gate.rs` 的单一真实
> pinned-workerd fixture 覆盖，并通过 `scripts/test-p0-exit.sh` 三轮 fresh-process 综合矩阵；
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

P1 不增加新的 Cloudflare 产品能力。它把已通过 aggregate Gate 的 P0 收敛成可安全升级、灾备和长期
运行的单机发行版，按依赖顺序完成：

1. capability/format freeze 与 P0 API conformance；
2. resource quota、统一磁盘 admission 和 offline data-dir ownership；
3. 短维护窗口下的 platform snapshot 与 fresh-host restore；
4. 以已验证 snapshot 为 rollback anchor 的 forward-only schema/workerd upgrade；
5. security fuzzing、恶意 Worker 和跨 account/resource isolation；
6. soak、load、crash-point/fault matrix 和 capacity envelope；
7. production health、metrics、doctor、support bundle 和 runbook；
8. advanced WebSocket hibernation 的 pinned stock-workerd 条件性 Gate。

P1.0 至 P1.7 是进入 P2 的必过 Gate；P1.8 hibernation 可以是 Go、Conditional Go 或 No-Go，不能
阻塞核心稳定性发布。整机 snapshot 不重复复制 R2/bundle 所在的外部 S3，不包含 master key，恢复要求
同一 S3 authority、同一外部 master key 和 source-compatible release；它恢复本地 authority，但不是
R2 point-in-time backup，R2 使用 restore 时外部 provider 中的当前状态。

详细的离线 snapshot/restore format、升级/回滚协议、磁盘 admission、安全与长稳矩阵、运维合同、
工作包和 Exit Gate 见 [P1：P0 平台加固详细设计](./p1-platform-hardening.md)。

### P2.1：Scheduler hardening

先扩展 P0 alarm 已验证的 scheduler kernel：

- Queue、Cron、Workflow 和 Alarm 独立 admission pool；
- ready/due fairness；
- batch claim；
- backoff/jitter；
- scheduler projection generation；
- virtual-clock test harness；
- test-only crash injection point。

### P2.2：Queue producer

- Queue lifecycle；
- `send`、`sendBatch` 和 delay；
- message ID、payload limits 和 producer transaction；
- restart persistence 和 Queue 隔离。

### P2.3：Queue consumer 与 Cron

- 一个 active consumer；
- batch claim 和 frozen consumer deployment/generation；
- per-message/batch ack/retry；
- max attempts 和 DLQ；
- consumer concurrency；
- Cron next-run projection、slot dedup 和 scheduled dispatch。

Queue crash matrix 必须覆盖 insert、claim、dispatch、handler、ack 和 DLQ move 每个事务
边界，承诺 at-least-once，不承诺 exactly-once。

### P2.4：Workflow core

Workflow 在 Queue 验证 scheduler lease 和 crash recovery 后实现：

- definition 和 instance create/status；
- frozen Worker deployment/class；
- `run()`；
- 顺序 `step.do()`；
- step result persistence 和 replay；
- generation/run-token fence；
- terminal success/error；
- live instance version referrer。

### P2.5：Workflow durable waiting

按依赖逐项增加：

1. step retry/backoff；
2. `step.sleep`；
3. `step.sleepUntil`；
4. `step.waitForEvent`；
5. `sendEvent` 和 timeout；
6. pause/resume/terminate/restart；
7. retention；
8. parallel step。

parallel step 最后实现，避免在顺序 replay 和 fencing 尚未稳定前扩大状态空间。

### P2 Exit Gate

最终黑盒链路：

```text
HTTP -> Queue -> Consumer -> Workflow
                         ├── KV/D1/R2
                         └── DO RPC/alarm
```

在每个 transaction/dispatch 边界注入 process crash，要求 Queue 不丢消息、Workflow
拒绝 stale commit、冻结版本正确、所有 due work 在 restart 后恢复。

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

## 24. 验收门槛

只有满足以下条件，才能称为可部署平台：

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

## 25. 参考资料

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
