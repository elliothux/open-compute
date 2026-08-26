# P0.7：Durable Objects 详细设计

> 状态：已实现并验证（2026-08-25）
>
> 前置依赖：P0.1 至 P0.6 已按当前 checkout 和用户确认跑通。
>
> 直接依赖：[P0.2：Workers Runtime](./p0-2-workers-runtime.md)、
> [P0.3：Resource 与 Binding Framework](./p0-3-resource-binding-framework.md)、
> [P0.5：R2](./p0-5-r2.md)、[P0.6：D1](./p0-6-d1.md)
>
> 后续消费者：[P0.8：Scheduler kernel 与 DO alarms](./p0-8-scheduler-do-alarms.md)

P0.7 在单进程、单 workerd、单本地磁盘的边界内实现 Cloudflare Durable Objects 最常用的
namespace、stub、fetch/RPC、私有 SQLite/KV storage 和 basic WebSocket。tenant Worker 仍由
P0.2 的 `workerLoader` 动态加载；DO class 也由同一份 immutable deployment 通过
`getDurableObjectClass()` 动态取得。

本阶段不复制 Cloudflare 的全球 placement、跨节点迁移和 multi-region consistency。它只保证：
同一个 object identity 的调用落到同一个 workerd actor 并由 actor 串行化，不同 object 可以并行，
持久数据由 workerd native Durable Object SQLite 管理，deployment promotion/rollback 采用明确的
restart policy。

## 0. 已验证基线与本阶段决策

[G0 结果](./g0-results.md)在 pinned stock workerd `v1.20260826.1` 上已经验证：

- `WorkerLoader.getDurableObjectClass(className)` 可以取得动态 class；
- dynamic class 可以挂到 native facet，并执行 fetch 与 RPC；
- 不同 facet 的 SQLite storage 隔离；
- transaction rollback、SIGKILL 后恢复、promotion、rollback 和显式 delete 均可工作；
- `localDisk` 可持久化 native DO/facet SQLite，但仍是 experimental workerd config。

这些结果证明“stock workerd + WorkerLoader + native facet”可行，却没有替 P0.7 解决 resource
authority、public ID、deployment fence、删除恢复和 tenant-facing API。P0.7 固定以下决策：

1. 只有一个静态、平台自有的 `DoHost` Durable Object class/namespace；
2. 每个 tenant Durable Object 映射为一个独立 `DoHost` actor，而不是所有对象共享固定 shard；
3. 每个 `DoHost` 只创建一个 tenant facet，facet class 由当前 deployment 动态加载；
4. public namespace/ID/stub 是 tenant isolate 内的 project-owned facade；
5. tenant 数据只写 workerd native facet SQLite，`control.sqlite` 不保存 tenant bytes；
6. workerd `localDisk` 目录由平台挂载和备份边界管理，但平台不读取内部文件名或 SQLite schema；
7. promotion/rollback 保留 P0.2 active pointer 的线性化点，并以单调
   `route_generation` 阻止旧代码回流；
8. P0.7 只实现 basic WebSocket；hibernation、全球重连和连接跨版本保留不在范围内。

### 0.1 为什么不直接复制 WDL 的固定 supervisor shard

WDL 为多副本 placement 和 Redis owner lease 把对象路由到固定数量的 supervisor，再用 facet 承载
tenant object。open-compute 的目标是单机 SMB self-deploy，不需要 owner election 或跨进程迁移。

P0.7 使用“一 tenant object 一 host actor”：

```text
public namespace + object ID
    └── physical host actor ID = H(do_storage_id, namespace_id, object_id, generation)
            └── DoHost native actor
                    ├── trusted host metadata
                    └── facet "tenant"
                            ├── dynamically loaded tenant class
                            └── private native SQLite storage
```

这样保留 native actor 的同对象串行化和不同对象并行，避免一个 supervisor actor 成为全局调度
瓶颈。它仍然参考 WDL 的动态 class/facet 路径，但不复制为多节点服务准备的 Redis lease 拓扑。

### 0.2 Miniflare 的使用边界

Miniflare 的 DO plugin 用于参考 workerd config 生成和本地持久化：

- namespace `uniqueKey` 必须稳定；
- `localDisk` 对应的 writable directory 必须在 workerd 启动前创建；
- `enableSql`、`preventEviction` 和 ephemeral mode 是 namespace config，而不是 tenant API。

Miniflare 静态枚举 worker/class；本平台的 tenant deployment 是动态的，因此不能照搬它的静态
class wiring。动态执行仍以 G0 验证过的 WorkerLoader/facet 为准。

### 0.3 当前实现与验证证据

P0.7 已落到 production runtime、central authority、loaded-isolate facade 和 operator surface：

- migration 007 保存 namespace/object lifecycle authority，tenant bytes 仍只由 native facet SQLite 持有；
- production `DoRouter`/`DoHost`、单一 loaded-isolate wrapper、同步 ID codec 和 namespace facade 已进入
  static workerd config；
- control API、delete/recreate generation fence、startup reconciliation、storage marker、health/metrics 和
  runtime composition 已接通；
- `./scripts/test-p0-7.sh` 已连续三轮 fresh process 验证 P0.7，并递归跑通 P0.6 至 P0.2；
- `./poc/g0 test all` 的三轮 aggregate verdict 为 `Conditional Go`，唯一条件仍是既有、精确 allowlist
  `loader:D-abort`；
- workspace format、Clippy、unit/integration、no-default-features、Rust 1.98 MSRV、metadata、dependency
  boundary 和 coverage 均通过；Rust line coverage 为 90.03%。

P0.7 Gate 覆盖 public ID/HMAC 与 intrinsic tamper、fetch/RPC/binary、SQLite/KV/transaction、
`deleteAll()`、`blockConcurrencyWhile()`、`waitUntil()`、同 object ordering、跨 object overlap、
WebSocket text/binary、class validation、in-flight promotion、A -> B -> A rollback、stale generation、
restart、delete/recreate 和 Worker tombstone 后显式 purge。`localDisk` 仍是 pinned workerd 的
experimental config；alarms 和 WebSocket hibernation 仍属于明确非目标。

## 1. 交付形态

```text
Control API
    └── DurableObjectNamespaceController
            ├── resources(kind=do_namespace)
            ├── do_namespaces / do_objects / control.sqlite
            └── immutable deployment binding

loaded tenant Worker isolate
    └── local DurableObjectNamespace facade
            ├── DurableObjectId
            ├── DurableObjectStub
            └── raw DoTransport terminal capability
                    └── static DoRouter
                            ├── current deployment + generation authorization
                            ├── object registration/deletion fence
                            └── native DoHost namespace
                                    └── one DoHost actor per tenant object
                                            └── facet "tenant"
                                                    ├── WorkerLoader class
                                                    └── native SQLite/KV storage
```

完成后，tenant 可以：

- 创建并绑定多个 DO namespace；
- 使用 `idFromName`、`newUniqueId`、`idFromString`、`get` 和 `getByName`；
- 通过 stub 调用 `fetch()` 和 bounded structured RPC；
- 在 DO class 中使用 `ctx.storage.sql`、同步 KV、异步 KV、transaction、`deleteAll()`、
  `blockConcurrencyWhile()` 和 `waitUntil()`；
- 在相同 object 上获得 actor ordering，在不同 object 上并行；
- 通过 DO fetch 建立 basic WebSocket；
- 在 workerd restart、deployment promotion/rollback 和 object delete/recreate 后得到确定行为。

### 1.1 完成定义

- namespace、binding、deployment、object identity 均由平台 authority 解析，不信任 tenant 传入 ID；
- public object ID 固定为 64 个小写 hex，`idFromString()` 拒绝其他 namespace 的 ID；
- object 名称不进入路径、日志 label 或 workerd namespace/class name；
- 同一 object 同一时刻只运行一个 native actor；不同 object 不经过全局串行 lane；
- tenant facet 只能取得自身 deployment 的 vars、secrets 和已授权 bindings；
- active deployment 更新与 P0.2 使用同一个 SQLite commit，不增加第二个 active pointer；
- 旧 generation 的迟到调用不能让 facet 从新版本退回旧版本；
- workerd 只使用稳定 `uniqueKey` 和持久 `localDisk`；普通 deploy 不改变 storage identity；
- delete/recreate 同名 Worker、namespace 或 object 不能看到 tombstoned generation 的数据；
- basic WebSocket 可双向收发；restart、promotion 和 delete 时允许断开并要求 client reconnect；
- stock workerd Gate 连续三轮 fresh process 通过，P0.2 至 P0.6 无回归。

### 1.2 非目标

- 全球唯一 placement、跨节点迁移、replication 或 actor owner election；
- Cloudflare jurisdiction/region hint；
- DO migrations、class rename、class transfer 或 namespace 跨 Worker 绑定；
- point-in-time recovery、按 object 在线导出或解释 workerd 内部 SQLite 文件；
- WebSocket hibernation、hibernation attachment、连接跨 restart/promotion 保留；
- 完整 `RpcTarget`/capability return、stub poisoning 的所有边缘行为；
- Cloudflare plan quota、计费、CPU time 或 storage size 的精确复制；
- alarms；`getAlarm/setAlarm/deleteAlarm` 在 P0.8 才注入；
- Queue、Workflow、Cron 或通用 scheduler。

## 2. P0.7.0：Production native-facet Hard Gate

P0.7 先把 G0 的最小动态 DO 路径移入 production config，不先写 public namespace facade。

### 2.1 静态 workerd 服务

`runtime/config.capnp` 增加：

```text
service do-host-worker
    ├── worker = project-owned DoHost module
    ├── durableObjectNamespace className = "DoHost"
    ├── stable uniqueKey = operator data-format identity
    ├── enableSql = true
    └── localDisk = data/do/workerd

service do-router
    ├── binding to do-host-worker namespace
    ├── binding to loader-host
    └── private control-plane authorization service
```

`uniqueKey` 由 platform data format 固定为 `open-compute-do-host-v1`。data format marker 另行绑定
platform ID、format version、uniqueKey 和 pinned workerd version。uniqueKey 不能包含 deployment ID、
worker name 或 class name，也不能在每次进程启动时随机生成。更换 uniqueKey 等价于创建一套不可见
的新 DO storage。

启动顺序：

1. 校验 `data/do/workerd` 是本地 writable directory，不允许 S3/NFS/SMB mount；
2. 校验 data format marker、workerd pinned version 和 localDisk capability；
3. 创建目录；
4. 生成 workerd config；
5. 启动 workerd 并运行 native probe；
6. probe 成功后才让 platform listener ready。

### 2.2 Gate

| Gate | 断言 |
| --- | --- |
| DG-01 | production DoHost 通过 WorkerLoader 取得 default 与 named DO class |
| DG-02 | facet fetch 与普通 RPC 均可执行 |
| DG-03 | 同一 facet 的 transaction rollback 正确 |
| DG-04 | host supervisor storage 与 tenant facet storage 相互不可见 |
| DG-05 | 两个 host actor/facet 的 storage 相互不可见 |
| DG-06 | SIGKILL workerd 后使用同一 uniqueKey/localDisk 恢复数据 |
| DG-07 | `abort("tenant")` 保留 facet storage，重新取得 class |
| DG-08 | `delete("tenant")` 永久删除 storage，随后新 facet 为空 |
| DG-09 | localDisk 路径缺失、只读或 version marker 不匹配时 fail closed |
| DG-10 | 三轮 fresh process 结果一致，不依赖 G0 临时 supervisor |

P0.7.0 失败时不得转向自己实现 actor storage 或 fork workerd。

## 3. Control plane schema 与 authority

新增 `007_durable_objects.sql`。

### 3.1 `do_namespaces`

```sql
CREATE TABLE do_namespaces (
  resource_id           TEXT PRIMARY KEY REFERENCES resources(id),
  owner_worker_id       TEXT NOT NULL REFERENCES workers(id),
  class_name            TEXT NOT NULL,
  do_storage_id         TEXT NOT NULL,
  namespace_storage_key TEXT NOT NULL UNIQUE,
  schema_version        INTEGER NOT NULL CHECK(schema_version >= 1),
  created_at_ms         INTEGER NOT NULL,
  CHECK(length(class_name) BETWEEN 1 AND 128),
  UNIQUE(owner_worker_id, class_name)
) STRICT;
```

规则：

- `resource_id` 同时是 public namespace identity；
- `owner_worker_id` 在 P0.7 创建后不可修改；
- `class_name` 是 owner Worker module 的 named export，P0.7 创建后不可 rename；
- `do_storage_id` 是 owner Worker 已有的稳定 storage identity，普通 deployment 不改变；
- `namespace_storage_key` 从 `do_storage_id + resource_id` 派生，因此同一 Worker 的多个 namespace
  彼此不同，但 Worker 删除/同名重建后不会复用；
- 同一 Worker 的一个 class 对应一个 namespace；
- P0.7 不允许一个 Worker 绑定另一个 Worker 拥有的 namespace；
- `resources.spec_generation` 与 product row 必须一致。

class 属于 namespace resource，而不是某个 deployment binding。`CanonicalBindingConfig` 在
capability V1 继续为空，避免相同 namespace 在不同 binding 上被解释为不同 class。

### 3.2 `do_objects`

```sql
CREATE TABLE do_objects (
  namespace_resource_id TEXT NOT NULL REFERENCES do_namespaces(resource_id),
  object_id             TEXT NOT NULL,
  generation            INTEGER NOT NULL CHECK(generation >= 1),
  state                 TEXT NOT NULL CHECK(state IN (
    'creating', 'ready', 'deleting', 'tombstoned'
  )),
  created_at_ms         INTEGER NOT NULL,
  updated_at_ms         INTEGER NOT NULL,
  deleted_at_ms         INTEGER,
  PRIMARY KEY(namespace_resource_id, object_id, generation),
  CHECK(length(object_id) = 64 AND object_id = lower(object_id))
) STRICT;

CREATE UNIQUE INDEX do_objects_live_identity
ON do_objects(namespace_resource_id, object_id)
WHERE state != 'tombstoned';

CREATE INDEX do_objects_reconcile
ON do_objects(state, updated_at_ms, namespace_resource_id, object_id)
WHERE state IN ('creating', 'deleting');
```

`do_objects` 不是 tenant data catalog。它只负责：

- 第一次 dispatch 前注册 identity；
- 给 delete/recreate 提供 generation fence；
- 为 P0.8 的 bounded alarm repair 提供可枚举对象集合；
- 让 namespace delete 判断是否 non-empty；
- crash 后收敛 creating/deleting state。

control plane 不保存 tenant DO SQLite bytes、row count、SQL schema 或 workerd 物理文件名。

### 3.3 name 不入库

`idFromName()` 在 tenant isolate 内同步完成；明文 name 和 name hash 都不写 `control.sqlite`。
明文只存在于调用方 facade 的 `DurableObjectId.name`/`DurableObjectStub.name`，且不写日志。
`do_objects` 只登记已经首次 dispatch 的 canonical object ID，不区分 named/unique。

如果未来 control API 要显示名称，需要新增 encrypted column、明确 retention 和独立 data migration，
不能把 P0.7 静默改为明文 catalog。

## 4. Public object ID

### 4.1 64-hex layout

P0.7 public ID 是 32 bytes：

```text
bytes[0..8]   = SHA-256("oc-do-ns-v1" || namespace_resource_id)[0..8]
bytes[8..32]  = payload

named payload  = HMAC-SHA-256(namespace_id_key, UTF8(name))[0..24]
unique payload = CSPRNG(24 bytes)
object_id      = lowercase_hex(bytes)
```

namespace prefix 让 `idFromString()` 可以同步拒绝跨 namespace ID。24-byte payload 为本地单体服务
提供足够碰撞余量。`newUniqueId()` 是同步 API，不能在返回前查询 control DB；它依赖 tenant isolate
的 CSPRNG，极小碰撞概率作为 capability V1 的固定风险，不伪造一次异步“预注册”。

`idFromName()`/`toString()` 同样必须同步，不能偷偷调用 JSRPC 或 `crypto.subtle`。binding factory 给
facade 一个不可枚举的 `{ namespacePrefix, namespaceNameKey, transport }` composite；name key 由
instance identity key 和 `namespace_storage_key` 通过 HKDF 派生，只对该 namespace 有效。platform-owned
`do-id-codec.js` 使用 pinned synchronous SHA-256/HMAC implementation，并在 tenant top-level code 执行前
捕获所需 intrinsics。`newUniqueId()` 使用 synchronous `crypto.getRandomValues()`。

codec source/hash 进入 loaded-isolate descriptor，并用 NIST test vector、UTF-8/Unicode boundary、
mutated-global-intrinsics 和跨 cold/warm deployment Gate 固定。不能为了实现同步 API 把 global instance
secret直接放到 enumerable env。

platform 的 physical host actor identity 不能直接使用 public name，也不能把任意 hash交给 native
`idFromString()`。DoRouter 先得到不透明 key，再让 workerd 生成合法 native ID：

```text
host_key = base64url(HMAC-SHA-256(
  instance_host_key,
  "oc-do-host-v1" || namespace_storage_key || object_id ||
  object_generation
))

host_actor_id = DO_HOST_NAMESPACE.idFromName(host_key)
```

`host_key` 和 native ID 只在 trusted DoRouter/DoHost 之间传递。它们不含 user name，即使 native
`ctx.id.name` 返回 host key，也不会泄露 tenant object name。

### 4.2 facade API

```ts
interface DurableObjectNamespace {
  idFromName(name: string): DurableObjectId
  newUniqueId(options?: { jurisdiction?: never }): DurableObjectId
  idFromString(hexId: string): DurableObjectId
  get(id: DurableObjectId, options?: DurableObjectGetOptions): DurableObjectStub
  getByName(name: string, options?: DurableObjectGetOptions): DurableObjectStub
}

interface DurableObjectId {
  toString(): string
  equals(other: DurableObjectId): boolean
  readonly name?: string
}
```

校验：

- name 必须是 string，UTF-8 最大 1024 bytes；
- ID 必须恰好 64 个 hex，canonical output 为小写；
- `get()` 只接受同一个 namespace facade 创建或验证过的 ID；
- jurisdiction/colo/location hints 在单节点 P0.7 抛
  `DO_PLACEMENT_OPTION_UNSUPPORTED`；
- facade object 使用不可伪造 private marker，不能靠普通 object 伪装 ID。

Cloudflare 当前可在部分场景从 object context 读取 `ctx.id.name`。P0.7 的 facet identity 使用
canonical 64-hex ID，不能可靠把原始 name 注入 native facet context。因此：

- caller-side `idFromName(...).name` 和 stub `name` 支持；
- tenant DO 内 `ctx.id.toString()` 支持 64-hex；
- P0.7 不承诺 DO 内 `ctx.id.name`，这是明确兼容偏差。

## 5. Binding 与 loaded-isolate facade

### 5.1 deployment validation

创建 DO binding 时：

1. 使用 P0.3 authority 校验 account/resource/kind/state；
2. 校验 namespace `owner_worker_id == deployment.worker_id`；
3. 生成 immutable `BindingDescriptorV1(kind=do_namespace, capability=1)`；
4. staging WorkerCode 使用与 runtime 相同的 loaded-isolate injection planner；
5. validation 真实调用 `getDurableObjectClass(class_name)`；
6. class 缺失或 export 不是可构造 DO class 时 deployment rejected；
7. descriptor、facade source 和 wrapper generator hash 进入 `worker_code_sha256`。

### 5.2 wrapper framework

原 `r2-wrapper-generator.js` 已移除并由产品无关的
`loaded-isolate-wrapper-generator.js` 取代；它一次生成一个 wrapper：

- 普通 HTTP default/named entrypoint 得到 local DO namespace facade；
- tenant DO class 本身也生成 class-specific wrapper；
- DO constructor、fetch、RPC method 和 class field 中的 importable env 都看到 wrapped env；
- DO class 获得同 deployment 的 vars、secrets、KV、R2、D1、DO 和 outbound binding；
- raw `DoTransport` 只存在于闭包，不能从 enumerable env、module export 或 error 取得；
- reserved module/source collision 在 deploy 时 fail closed。

facade 的 per-namespace name key 只允许生成该 namespace 中调用方本来就有权生成的 ID，不是 backend
authorization token；backend 仍从 raw transport/binding authority 推导 namespace。即使 tenant 通过
自身代码穷举 name，也不能获得其他 namespace key 或 physical host capability。

不得为 DO 再叠第二层 wrapper；R2、D1、DO 的 local facade 必须由一个 deterministic planner 注入。

### 5.3 raw transport scope

每个 raw transport 的 trusted props 至少绑定：

```text
account_id
worker_id
deployment_id
route_generation
binding_id
namespace_resource_id
capability_version
request_id
```

tenant terminal call 只提交 `object_id` 和 operation payload。DoRouter 每次调用重新解析 binding、
active deployment 和 resource state，不信任 tenant 提交 account/worker/namespace。

## 6. Dispatch、ordering 与 deployment restart policy

### 6.1 request path

```text
tenant stub.fetch/RPC
    1. local facade validates method/body/size
    2. raw DoTransport sends scoped terminal call
    3. DoRouter reauthorizes binding + current active deployment
    4. register/resolve live do_objects generation
    5. derive physical DoHost actor ID
    6. DoHost compares execution generation
    7. get/replace facet with current WorkerLoader class
    8. dispatch fetch/RPC to tenant facet
    9. map result/error without exposing host identity
```

`do_objects` 的第一次 insert 在 native storage 访问前 commit。若进程在 insert 后、facet 创建前崩溃，
reconciler 可把没有 native effect 的 creating row 重试为 ready；不得因为 native storage 尚未出现
而换 object generation。

### 6.2 ordering

- 同一个 local stub 发出的调用保持 E-order；
- 同一 object 的不同 stub 由同一个 native actor 串行接收；
- 不同 object 对应不同 DoHost actor，可并行；
- input gate、output gate、transaction 和 `blockConcurrencyWhile` 由 pinned workerd native DO
  语义提供；
- 不在 DoRouter 增加 per-namespace 或全局 mutex。

P0.7 Gate 必须使用阻塞 barrier/时间重叠证明“不同 object 真并行”，不能只证明它们拿到不同 facet。

### 6.3 promotion/rollback

P0.2 promotion transaction 仍是唯一线性化点：

```sql
UPDATE workers
SET active_deployment_id = :new_deployment,
    route_generation = route_generation + 1,
    updated_at_ms = :now
WHERE id = :worker
  AND active_deployment_id = :expected;
```

DoHost 在自身 supervisor SQLite（不在 tenant facet SQLite）保存最高 execution generation 和当前
deployment ID。这个极小的 trusted metadata row 随 host actor 持久化，避免 actor eviction/workerd
restart 后忘记已经见过的新版本；G0 已验证 host storage 与 facet storage 隔离。收到更高 generation：

1. 阻止新的 tenant dispatch；
2. 等待当前调用到 safe boundary；
3. `ctx.facets.abort("tenant")`，保留 SQLite storage；
4. 从新 deployment 取得 class 并创建 facet；
5. 在 host metadata 中持久化最高 generation/deployment；
6. 放行新调用。

收到更低 generation 时返回 `DO_DEPLOYMENT_STALE`，绝不能 abort 当前 facet 或加载旧 deployment。
rollback 同样增加 `route_generation`，所以 A -> B -> A 是 generation 1 -> 2 -> 3，不会被误判为
回到旧状态。

已经进入 facet 的调用可以完成；promotion 不保证中途抢占。调用方在 response loss 时必须按
result-unknown 处理非幂等 mutation。

### 6.4 isolate eviction

P0.7 不设置 `preventEviction=true` 作为语义依赖。workerd 可以回收内存中的 actor/facet，下一次调用
必须从 native storage 恢复。tenant class 的 process-memory cache 不是 authority。

## 7. Tenant DO storage

P0.7 直接暴露 pinned workerd SQLite-backed Durable Object 的常用能力：

| API | P0.7 |
| --- | --- |
| `ctx.storage.sql.exec()` | 支持 |
| synchronous KV `get/put/delete/list` | 支持 |
| async KV `get/put/delete/list` | 支持 |
| `transaction()/transactionSync()` | 支持，以 pinned workerd 为准 |
| `deleteAll()` | 支持 |
| `blockConcurrencyWhile()` | 支持 |
| `waitUntil()` | 支持 |
| `getAlarm/setAlarm/deleteAlarm` | P0.8 |
| PITR/bookmark | 不支持 |

### 7.1 storage ownership

`data/do/workerd` 的硬约束：

- 只允许 workerd 进程读写内部文件；
- Rust/control plane 不打开、`ATTACH`、rename、copy 或按文件名枚举 object SQLite；
- 不假设 workerd 的 namespace/class subdirectory、`.sqlite`、`-wal` 或 `-shm` 命名；
- 在线运行时不做逐 object filesystem snapshot；
- operator 只能看到总目录容量、disk watermark 和 workerd health，不暴露 tenant SQL schema；
- workerd upgrade 必须通过 fresh-copy/restart/recovery Gate 后才能更换 pinned version。

DoHost supervisor SQLite 只允许保存 execution generation、deployment ID 和 host data-format version；
tenant 业务 bytes 仍全部在 facet storage。platform Rust 同样不能直接打开这份 supervisor SQLite。

### 7.2 limits

P0.7 不宣称复制 Cloudflare plan limit。平台只增加本地 policy：

```toml
[durable_objects]
max_namespace_name_bytes = 128
max_object_name_bytes = 1024
max_rpc_request_bytes = 1048576
max_rpc_response_bytes = 1048576
max_fetch_body_bytes = 33554432
dispatch_timeout_ms = 30000
max_in_flight_dispatches = 256
disk_high_watermark_percent = 85
disk_stop_writes_percent = 95
```

workerd/native SQLite 自己的 SQL/value/row limit 仍需在 Gate 中记录。disk 达 high watermark 时
health degraded；达 stop-writes watermark 时平台拒绝创建新 object，并让 native write failure
映射为 `DO_STORAGE_LIMIT`，不能删除旧数据腾空间。

## 8. Fetch、RPC 与 error surface

### 8.1 fetch

`stub.fetch(input, init)` 支持 Request、URL/string 和常用 RequestInit。request/response body 走
streaming transport，不先 materialize 完整 body。平台保留的 internal header 在进入 tenant DO 前
剥离。

DO fetch 可以返回任意普通 Response，包括 WebSocket upgrade。4xx/5xx Response 原样返回；只有
platform/workerd exception 才映射为 stable error。

### 8.2 RPC

`stub.someMethod(...args)` 由 Proxy 生成 RPC terminal call：

- 支持 null、boolean、finite number、string、plain object/array 和 bounded binary value；
- preserving cycles、functions、Promise、stream、`RpcTarget` 和 capability return 不在 P0.7；
- method name 必须是合法 public identifier，拒绝 `constructor`、`prototype`、`__proto__`；
- request/response 各自默认 1 MiB；
- method exception 映射为 opaque `DO_RPC_EXCEPTION`，日志只记录 request ID 和低基数 class；
- platform internal object ID、deployment ID 和 stack 不返回 tenant。

RPC 能力比 Cloudflare JSRPC 窄，是明确 P0 兼容偏差。常用 plain-data method 纳入 Exit Gate。

### 8.3 stable errors

| Code | 含义 | retry |
| --- | --- | --- |
| `DO_NAMESPACE_NOT_FOUND` | binding/resource 不存在 | no |
| `DO_ID_INVALID` | 格式或 namespace prefix 错 | no |
| `DO_OBJECT_DELETING` | object delete fence 已立 | after operation |
| `DO_DEPLOYMENT_STALE` | 迟到 generation | resolve current stub |
| `DO_CLASS_NOT_FOUND` | ready deployment 不再满足 invariant | no/operator |
| `DO_STORAGE_UNAVAILABLE` | workerd/localDisk 不可用 | yes |
| `DO_STORAGE_LIMIT` | local disk policy/native quota | after capacity |
| `DO_DISPATCH_TIMEOUT` | result unknown | cautious |
| `DO_RPC_UNSUPPORTED` | value/method 超出 P0 surface | no |
| `DO_RUNTIME_EXCEPTION` | tenant class exception | application |

## 9. Basic WebSocket

P0.7 支持以下最小链路：

```js
const pair = new WebSocketPair();
const [client, server] = Object.values(pair);
server.accept();
ctx.waitUntil(handle(server));
return new Response(null, { status: 101, webSocket: client });
```

验收范围：

- Worker -> DO fetch 的 101 upgrade 原样返回 caller；
- text/binary message 双向收发；
- close code/reason 透传；
- client disconnect 释放 transport；
- object delete、facet abort、promotion 和 workerd restart 关闭 socket；
- socket 关闭不代表 storage transaction 回滚或请求 abort。

不支持/不承诺：

- `ctx.acceptWebSocket`、tag、attachment 和 hibernatable event handler；
- socket 跨 promotion/restart 保留；
- Cloudflare connection 数量和 message size 上限；
- client 断连自动中止已经进入 DO 的普通 fetch。G0 已观察到 loaded Worker request signal 不一定随
  caller disconnect abort，因此清理必须依赖 deadline/close，而不是只依赖 AbortSignal。

## 10. Object 与 namespace lifecycle

### 10.1 object delete

control API 提供显式 destructive delete；tenant Worker 不获得任意 object-admin capability：

1. transaction 把 live `do_objects` 改为 `deleting` 并增加 delete fence；
2. 新 dispatch 返回 `DO_OBJECT_DELETING`；
3. drain in-flight pin，超时则保留 deleting 供 reconciler 重试；
4. trusted DoHost 执行 `facets.delete("tenant")`；
5. 确认 native delete 后把 row 改为 tombstoned；
6. 同一 public ID 再被使用时创建 `generation + 1`，physical host actor ID 随 generation 改变。

关键 crash point：

- fence 前 crash：对象仍 ready；
- fence 后/native delete 前 crash：reconciler 继续 delete，不能重新开放；
- native delete 后/tombstone 前 crash：delete 必须幂等；
- response 丢失：GET/list object lifecycle 显示最终 state。

### 10.2 namespace delete

默认 delete 只允许没有 live object、没有 resource referrer 的 namespace。`force=true` 是显式
destructive operation：

1. 先禁止新 binding 与新 object；
2. bounded batch 标记 object deleting；
3. 按 object 执行 native delete；
4. 所有 object tombstoned 后删除 product physical marker；
5. 最后 tombstone resource。

不能通过删除整个 workerd localDisk 子目录实现 namespace delete，因为平台不拥有 upstream
内部布局。large namespace delete 可以长时间保持 deleting 并由 reconciler 续跑。

### 10.3 Worker delete

P0.2 的 Worker tombstone 不立即物理删除 DO storage。原因是 deployment retention、rollback 和
operator 误操作恢复窗口。最终 purge 必须是单独的 destructive operation，复用 namespace force
delete 流程；不能把 cascade delete 隐藏在普通 Worker delete 中。

## 11. 安全边界

- public ID、binding ID 和 tenant payload 均不能选择 physical actor ID；
- DoRouter 每次从 binding authority 推导 account/worker/namespace，不接受 tenant override；
- raw DoTransport、DoHost namespace 和 WorkerLoader capability 只在 system Worker；
- tenant module 不能 import project internal facade source 以取得 raw capability；
- namespace prefix/HMAC key 由 instance secret 派生，不出现在 config dump；
- error/log/metric 不记录 object name、64-hex ID 或 physical host ID，只记录 keyed low-cardinality hash；
- facet class 只从 ready/current deployment 取得；
- deletion fence 和 generation 在 native effect 前检查；
- workerd localDisk 必须是 local filesystem，严禁 tenant SQL 连接 control/D1/KV 文件；
- DO 对自身 storage 内部表的破坏只影响该 object，不得变成 host filesystem 或其他 tenant 能力。

## 12. Observability 与 doctor

metrics：

```text
oc_do_dispatch_total{operation,outcome}
oc_do_dispatch_duration_seconds{operation}
oc_do_active_host_actors
oc_do_facet_reload_total{reason}
oc_do_object_reconcile_total{state,outcome}
oc_do_websocket_active
oc_do_storage_bytes
oc_do_storage_watermark{state}
```

禁止以 namespace/object/class name 作为 metric label。

`doctor` 检查：

- pinned workerd version、`localDisk` experimental capability 和 data format marker；
- storage directory owner/mode/free space；
- stable uniqueKey fingerprint 与 instance identity 一致；
- static DoHost/DoRouter production probe；
- control 中 creating/deleting object 数量和 oldest age；
- live binding 的 owner Worker/class invariant；
- active deployment 能取得所有 bound DO classes。

doctor 不枚举或打开 workerd 内部 object SQLite。

## 13. Work packages

### P0.7.0：Production native-facet Gate（已完成）

- production DoHost/DoRouter static service；
- stable uniqueKey、localDisk bootstrap、version marker；
- port G0 fetch/RPC/storage/restart/delete Gate。

### P0.7.1：Namespace schema 与 public ID（已完成）

- `007_durable_objects.sql`；
- namespace CRUD、owner/class invariant；
- 64-hex ID/facade、HMAC/name rules；
- object registry 与 first-dispatch reconciliation。

### P0.7.2：Facade、binding 与 transport（已完成）

- generalize loaded-isolate wrapper；
- local namespace/ID/stub facade；
- synchronous ID codec 与 per-namespace key injection Gate；
- raw scoped DoTransport；
- staging class validation 和 descriptor hash。

### P0.7.3：DoRouter、DoHost 与 native storage（已完成）

- one host actor per object；
- dynamic class/facet dispatch；
- fetch、plain-data RPC；
- native SQLite/KV/transaction/input-output gate。

### P0.7.4：Promotion、rollback 与 restart policy（已完成）

- route generation fence；
- facet abort/reload；
- stale generation rejection；
- in-flight/result-unknown tests。

### P0.7.5：Delete/recreate 与 reconciler（已完成）

- object delete state machine；
- namespace force delete；
- Worker final purge；
- every-crash-boundary tests。

### P0.7.6：Basic WebSocket（已完成）

- 101 pass-through；
- text/binary/close；
- restart/promotion/delete disconnect semantics；
- connection and message budgets。

### P0.7.7：Conformance 与 recovery Gate（已完成）

- stock workerd 三轮 fresh process；
- P0.2-P0.6 regression；
- disk full/read-only/corruption isolation；
- metrics、doctor 和 operator runbook。

依赖顺序固定为 `0 -> 1 -> 2 -> 3 -> 4 -> 5 -> 6 -> 7`。P0.7.4 之前不能把 DO 暴露为
production-ready，因为缺少 deployment fence；P0.7.5 之前不能承诺 destructive lifecycle。

## 14. Test matrix 与 Exit Gate

### 14.1 API/identity

- idFromName deterministic、UTF-8 boundary、same/different namespace；
- newUniqueId uniqueness/collision retry；
- idFromString canonicalization、invalid hex、cross-namespace reject；
- get/getByName/stub id/name；
- fake ID/private marker tamper；
- unsupported jurisdiction。

### 14.2 execution/storage

- default/named DO class；
- fetch Request/stream/Response；
- plain-data RPC、binary、size/method/type reject；
- same object ordered writes；
- two object barrier proves overlap；
- SQL、sync KV、async KV、transaction rollback、deleteAll；
- blockConcurrencyWhile constructor failure/retry；
- host/facet/object/namespace/tenant isolation；
- workerd graceful restart 与 SIGKILL recovery。

### 14.3 deployment

- deploy A -> promote B -> rollback A；
- old generation arrives after B and cannot reload A；
- promotion while request is running；
- class missing at staging；
- R2/D1/KV/DO combined env in tenant class；
- warm/cold/evicted actor receives identical immutable descriptor。

### 14.4 lifecycle/failure

- crash around object register/create；
- delete before/after native effect and response loss；
- same ID recreation sees empty storage；
- same display-name namespace recreation sees empty storage；
- non-empty namespace delete refusal；
- force delete resume；
- Worker delete preserves until explicit purge；
- localDisk missing/readonly/full；
- one object storage failure does not mark unrelated resource unavailable。

### 14.5 WebSocket

- upgrade、text、binary、close；
- two objects concurrent sockets；
- client disconnect cleanup；
- promotion/restart/delete closes；
- message/connection budget reject。

### 14.6 P0.7 Exit Gate

P0.7 完成必须同时满足：

1. DG-01 至 DG-10 连续三轮 fresh process 通过；
2. API、deployment、lifecycle 和 WebSocket matrix 全部通过；
3. 一个 fixture Worker 同时使用 KV、R2、D1 和两个 DO object；
4. 同对象 ordering 与不同对象真实并行均有确定测试；
5. A -> B -> rollback A 和 stale-generation race 通过；
6. SIGKILL 后 storage 恢复，delete/recreate 后 storage 隔离；
7. `cargo fmt --check`、Clippy、MSRV、unit/integration 和既有 P0 Gate 通过；
8. 文档记录 pinned workerd/localDisk experimental risk 和明确兼容偏差。

## 15. 建议实现文件边界

```text
crates/core/src/durable_objects.rs
crates/storage/migrations/007_durable_objects.sql
crates/storage/src/durable_objects.rs
crates/workers/src/durable_objects.rs
crates/service/src/do_http.rs
crates/service/src/binding_backend.rs
crates/service/src/runtime_bridge.rs
crates/service/src/metrics_do.rs
runtime/system-workers/do-host.js
runtime/system-workers/do-router.js
runtime/system-workers/do-facade.js
runtime/system-workers/do-id-codec.js
runtime/system-workers/loaded-isolate-wrapper-generator.js
scripts/test-p0-7.sh
```

文件名可以随现有 crate ownership 调整，但必须维持三条边界：

- control lifecycle 不进入 system Worker；
- tenant facade 不取得 private router/loader token；
- platform Rust 不解析 workerd DO SQLite。

## 16. 参考

- [Cloudflare Durable Object namespace API](https://developers.cloudflare.com/durable-objects/api/namespace/)
- [Cloudflare Durable Object ID API](https://developers.cloudflare.com/durable-objects/api/id/)
- [Cloudflare Durable Object stub API](https://developers.cloudflare.com/durable-objects/api/stub/)
- [Cloudflare Durable Object state API](https://developers.cloudflare.com/durable-objects/api/state/)
- [Cloudflare SQLite-backed storage API](https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/)
- [Cloudflare Durable Objects limits](https://developers.cloudflare.com/durable-objects/platform/limits/)
- [Cloudflare Dynamic Worker facets](https://developers.cloudflare.com/dynamic-workers/usage/durable-object-facets/)
- [workerd WorkerLoader](https://github.com/cloudflare/workerd/blob/v1.20260826.1/src/workerd/api/worker-loader.h)
- [workerd config schema](https://github.com/cloudflare/workerd/blob/v1.20260826.1/src/workerd/server/workerd.capnp)
- [WDL Durable Objects](https://github.com/wdl-dev/wdl/blob/main/docs/modules/durable-objects.zh.md)
- [WDL DO runtime actor](https://github.com/wdl-dev/wdl/blob/main/do-runtime/actor.js)
- [Miniflare Durable Objects plugin](https://github.com/cloudflare/workers-sdk/tree/main/packages/miniflare/src/plugins/do)
