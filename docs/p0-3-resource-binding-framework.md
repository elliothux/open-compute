# P0.3：Resource 与 Binding Framework 详细设计

> 状态：已实现并通过 Exit Gate（2026-08-25）
>
> 前置依赖：[P0.1：Platform Foundation](./p0-1-platform-foundation.md) 与
> [P0.2：Workers Runtime](./p0-2-workers-runtime.md)
>
> 实现基线：当前 checkout 已实现 typed `BindingDescriptorV1`、`ResourceController`、
> `ResourcePins`、immutable deployment binding、每次 cold/warm 请求的 `RuntimeSource`
> 重校验、静态 `ctx.exports.KVNamespace` factory，以及独立 generation capability 的
> private BindingBackend。
>
> 后续消费者：[P0.4：KV](./p0-4-kv.md)、[P0.5：R2](./p0-5-r2.md)、
> [P0.6：D1](./p0-6-d1.md)、
> [P0.7：Durable Objects](./p0-7-durable-objects.md)、
> [P0.8：Scheduler 与 DO Alarms](./p0-8-scheduler-do-alarms.md)

P0.3 不交付一个新的 tenant-facing 存储产品。它交付所有持久化产品共用的资源生命周期、
deployment binding、运行时 capability、内部 transport、删除 fence 和错误协议。P0.4 的 KV
是第一个真实消费者；P0.3 使用测试专用 fake resource/driver/executor 驱动静态 KV adapter，
以跑通框架 Gate。

## 0. 实现与验证状态

已实现的 production framework 包括：UUIDv7 resource/binding identity、`003` forward-only
migration、resource/binding/referrer authority、create/delete reconciliation、typed canonical
descriptor、deployment hash 与 warm-load invariant、静态 loader factory、独立 backend token、
每次调用的 DB authorization/resource pin、固定 byte/time budget、稳定错误、低基数 metrics 和
secret-free doctor inspection。P0.3 的 production KV executor 按阶段边界保持 fail closed；真实
KV 数据引擎仍属于 P0.4。

已验证证据：

- `./test/test-p0-3.sh`：RB-01 至 RB-18 连续三轮 fresh-process 全部通过，并继续跑通三轮 P0.2
  regression Gate；
- `./test/coverage.sh`：workspace 全目标、全 feature 测试通过，Rust 行覆盖率 90.04%，不低于
  90.00% 门槛；
- format、Clippy、no-default-features、Rust 1.98 MSRV、metadata、dependency boundary 与 diff
  whitespace 检查通过。

## 1. 交付目标

P0.3 要把下面这条路径变成平台级固定协议：

```text
Control API
    └── product controller
            └── ResourceRepository / control.sqlite
                    ├── resource lifecycle
                    └── immutable deployment binding

deploy
    └── BindingResolver
            └── BindingDescriptorV1
                    └── WorkerCodeDescriptorV1.binding_descriptors

request
    └── RuntimeSource
            └── loader-host BindingFactory
                    └── ctx.exports.<typed adapter>({ props })
                            └── tenant env.<BINDING>
                                    └── JSRPC
                                            └── trusted adapter
                                                    └── private BindingBackend
                                                            └── ResourceDriver
```

完成后，新增一种资源类型只需要实现产品 schema、静态 driver、静态 adapter 和产品测试，
不再重复实现下面这些高风险能力：

- account/resource/deployment 的授权与物理 ID 冻结；
- immutable binding descriptor 与 warm-load invariant；
- tenant 看不到路径、S3 credential、内部 token 或通用内部 `Fetcher`；
- 每次调用的 binding 重新解析、resource pin、size budget 与稳定错误映射；
- `creating -> ready -> deleting -> tombstoned` 的 crash recovery；
- referrer 检查、删除 fence、drain 和同名重建隔离；
- bounded connection/cache 生命周期和按 resource 隔离的健康状态。

### 1.1 完成定义

- `resources` 是所有产品资源的 identity/lifecycle authority；
- deployment 只绑定 immutable resource ID，不绑定 display name；
- ready deployment 的 binding row、descriptor 和权限不可修改；
- binding descriptor 进入 `worker_code_sha256`，cold/warm path 都重新校验；
- tenant env 只获得产品形状的 JSRPC stub，不获得内部 service binding；
- BindingBackend 不信任 tenant 提交的 account/resource identity，只信任受控 `binding_id`；
- adapter kind、capability version 和方法集合是编译期静态 registry；
- resource delete 在存在 referrer 或 in-flight pin 时不能误删物理数据；
- crash 可以从 DB state + driver probe 收敛，不依赖进程内 callback；
- fake adapter 的真实 workerd Gate 覆盖 isolation、tamper、restart、delete 和 byte limit。

### 1.2 非目标

- 公共的“任意 resource CRUD”API；P0.4 起仍提供 KV/R2/D1/DO 专用 API；
- 动态加载第三方 driver、adapter plugin 或 tenant-supplied host code；
- Cloudflare 全量 REST API、Wrangler resource provisioning protocol；
- 多节点 owner election、跨区域 replication 或 distributed transaction；
- 在 P0.3 实现 KV/R2/D1/DO 的真实数据操作；
- 在 runtime request path 自动迁移资源 schema；
- 允许 tenant 通过一个通用 `fetch(url)` adapter 调用任意平台内部 endpoint；
- 以 process-local cache 作为 resource 或 binding 的 authority。

## 2. 现有 P0.2 基线与改动边界

P0.3 必须增量扩展当前实现，不能另起第二套 runtime/control plane。

| 当前 P0.2 能力 | P0.3 的使用方式 |
| --- | --- |
| `WorkerCodeDescriptorV1.binding_descriptors` 预留字段 | 改为 typed、canonical `BindingDescriptorV1` 列表 |
| `RuntimeSource` 每次 warm/cold 都校验 descriptor | 同一位置加载并校验 immutable binding snapshot |
| `ctx.exports.OutboundGateway({ props })` | 沿用成静态 `BindingFactory` 和产品 adapter class |
| private `runtime-source` external service | 增加同等级、独立 capability 的 `binding-backend` service |
| `DeploymentPins` | 增加同语义的 `ResourcePins`；一次调用同时持有 binding/resource pin |
| `deployment_referrers` | 保留 deployment retention；新增 resource 维度的 referrer registry |
| staging/validating/ready deployment | binding 只在 staging 创建，ready 后完全 immutable |
| public-only `globalOutbound` | tenant 仍不能通过 global `fetch()` 到达 loopback/private backend |

P0.3 不改变 `workerLoader` 的核心调度模型：loader key 仍然是 immutable deployment ID，
promotion/rollback 仍然只切 active pointer。Binding 的变化通过新 deployment 生效，不做 warm
isolate 热注入。

## 3. Authority 与信任边界

### 3.1 Authority 划分

| 数据 | Authority | 可丢失 projection/cache |
| --- | --- | --- |
| resource ID、kind、lifecycle、account | `control.sqlite.resources` | product list cache |
| product-specific physical mapping | 产品表，如 `kv_namespaces` | driver handle cache |
| deployment env name -> resource ID | `deployment_bindings` | RuntimeSource snapshot |
| referrer | `resource_referrers` | 无；它是删除安全检查的一部分 |
| binding runtime descriptor | immutable DB rows重建后的 canonical bytes | loader-host snapshot |
| resource availability | DB 中最后一次持久状态 + driver probe | in-memory health debounce |
| active operation | `ResourcePins` process memory | crash 后天然清空 |

### 3.2 Capability 边界

Tenant Worker 的 `env.CACHE` 只能看到产品方法，例如 KV 的 `get()`/`put()`。它不能看到：

- `account_id`、物理 SQLite path、S3 bucket/prefix 或 credential；
- `binding-backend` service binding 或 generation token；
- 能指定任意 `resource_id`/`binding_id` 的通用 RPC；
- adapter 的 `ctx.props`；Cloudflare Dynamic Workers 的 custom binding 模式正是由 loader
  Worker 通过 `ctx.exports.Class({ props })` 创建 stub，props 只供 loader-side class 使用；
- platformd loopback address、internal header 或 raw driver error。

参考：[Dynamic Workers custom bindings](https://developers.cloudflare.com/dynamic-workers/usage/bindings/)
与 [Workers RPC](https://developers.cloudflare.com/workers/runtime-apis/rpc/)。

### 3.3 每次调用仍要重新授权

`ctx.props` 是 immutable routing hint，不是独立 authorization authority。BindingBackend 每次
操作都通过 `binding_id` 查询并验证：

1. binding 存在且属于 immutable ready deployment；
2. binding 的 resource、kind、resource generation 与 descriptor 一致；
3. resource 与 deployment 属于同一个 account；
4. resource lifecycle 允许该操作，availability 没有被隔离；
5. method 在 capability version 和 permission set 中；
6. size/time/concurrency budget 未超限。

Tenant 不能把 `resource_id` 放进请求覆盖查询结果。内部协议即使携带 descriptor 摘要，也只
用于快速拒绝 stale/tampered caller，不能替代 DB authority。

## 4. 类型与 ID

在 `open-compute-core` 增加：

```rust
pub struct ResourceId(Uuid); // UUIDv7
pub struct BindingId(Uuid);  // UUIDv7
```

规则与现有 `WorkerId`/`DeploymentId` 一致：

- API 只接受 canonical textual form；
- 不接受大小写、percent decode、路径别名或 name 代替 ID；
- ID 是不可复用 identity；删除后同名重建必须生成新 ID；
- 日志可以记录 resource/binding ID，但不能记录 key、secret、路径和产品 payload；
- storage 层不得以裸 `String` 混用不同 ID 类型。

Resource `name` 只是 account 内、kind 内唯一的 display name。rename 不改变 ID，也不改变旧
deployment 的 binding。

## 5. `control.sqlite` migration

新增 `003_resource_bindings.sql`。实际 SQL 可以按 SQLite trigger 限制调整，但下列字段和不变量
必须存在。

### 5.1 `resources`

```sql
CREATE TABLE resources (
  id                       TEXT PRIMARY KEY,
  account_id               TEXT NOT NULL REFERENCES accounts(id),
  kind                     TEXT NOT NULL CHECK(kind IN (
                             'kv_namespace',
                             'r2_bucket',
                             'd1_database',
                             'do_namespace'
                           )),
  name                     TEXT NOT NULL,
  state                    TEXT NOT NULL CHECK(state IN (
                             'creating', 'ready', 'deleting', 'tombstoned'
                           )),
  availability             TEXT NOT NULL DEFAULT 'healthy' CHECK(availability IN (
                             'healthy', 'degraded', 'unavailable'
                           )),
  availability_code        TEXT,
  spec_generation          INTEGER NOT NULL DEFAULT 1 CHECK(spec_generation >= 1),
  driver_schema_version    INTEGER NOT NULL CHECK(driver_schema_version >= 1),
  created_at_ms            INTEGER NOT NULL,
  updated_at_ms            INTEGER NOT NULL,
  deleted_at_ms            INTEGER,
  CHECK(length(name) BETWEEN 1 AND 128),
  CHECK((state = 'tombstoned') = (deleted_at_ms IS NOT NULL))
) STRICT;

CREATE UNIQUE INDEX resources_live_name
ON resources(account_id, kind, name)
WHERE state != 'tombstoned';

CREATE INDEX resources_reconcile
ON resources(state, updated_at_ms, id)
WHERE state IN ('creating', 'deleting');
```

`state` 是 lifecycle；`availability` 是运行健康，二者不能混用。单个 SQLite 文件损坏时，KV
resource 仍是 `ready + unavailable`，而不是伪造为 `deleting`。这样 repair/restore 和 delete
有确定的不同路径。

`spec_generation` 只在显式、binding-breaking 的产品配置变更时增加。rename、backup、health
变化都不增加。P0.4 KV 没有在线 breaking mutation；需要替换物理 identity 时创建新 resource
ID，而不是修改 generation。

### 5.2 `deployment_bindings`

```sql
CREATE TABLE deployment_bindings (
  id                       TEXT PRIMARY KEY,
  deployment_id            TEXT NOT NULL REFERENCES worker_deployments(id),
  name                     TEXT NOT NULL,
  kind                     TEXT NOT NULL,
  resource_id              TEXT NOT NULL REFERENCES resources(id),
  resource_spec_generation INTEGER NOT NULL CHECK(resource_spec_generation >= 1),
  capability_version       INTEGER NOT NULL CHECK(capability_version >= 1),
  permissions_json         BLOB NOT NULL,
  config_json              BLOB NOT NULL,
  descriptor_sha256        BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms            INTEGER NOT NULL,
  UNIQUE(deployment_id, name),
  CHECK(length(name) BETWEEN 1 AND 64)
) STRICT;

CREATE INDEX deployment_bindings_resource
ON deployment_bindings(resource_id, deployment_id, id);
```

`kind` 在 binding row 中冗余保存是刻意的：descriptor 构建和 runtime authorization 可以检查
row/resource kind 一致，防止错误 migration 或代码路径把 KV adapter 指向 D1 resource。

### 5.3 `resource_referrers`

```sql
CREATE TABLE resource_referrers (
  resource_id       TEXT NOT NULL REFERENCES resources(id),
  referrer_kind     TEXT NOT NULL,
  referrer_id       TEXT NOT NULL,
  created_at_ms     INTEGER NOT NULL,
  PRIMARY KEY(resource_id, referrer_kind, referrer_id)
) STRICT, WITHOUT ROWID;
```

第一批 `referrer_kind`：

```text
deployment_binding
queue_dlq
queue_consumer
workflow_definition
do_class
```

P0.3 只实际写 `deployment_binding`，后续产品复用同一 registry。它不替代真正的 foreign key；
它提供统一的 delete reason、审计和跨产品引用检查。

### 5.4 必须由 trigger/transaction 保证的约束

- binding 只能插入 `staging` deployment；ready/invalid/deleting deployment 不可插入；
- binding row 不允许 `UPDATE`；删除只允许随 staging abort 或 deployment final delete；
- resource 与 deployment account 必须一致；
- binding kind 必须等于 resource kind；
- resource 必须为 `ready`，generation 必须等于当前值；
- binding name 必须通过 P0.2 env name grammar；
- 同一 deployment 的 var、secret、binding name 不能冲突；
- binding insert/delete 同 transaction 创建/删除 `resource_referrers`；
- 存在 referrer 时不能从 `ready` 转 `deleting`；
- tombstoned row 不可恢复为 ready，也不可把 ID 绑定到新物理资源；
- ready deployment 的 binding bytes 与 descriptor hash 永久 immutable。

SQLite 无法用普通 FK 表达的跨表 state/kind/account 条件，使用 `BEFORE INSERT/UPDATE` trigger
作为最后防线，controller 仍需返回清晰的产品错误。

## 6. `BindingDescriptorV1`

P0.2 的 `Vec<serde_json::Value>` 改成 typed descriptor：

```rust
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BindingDescriptorV1 {
    pub schema_version: u32,             // 1
    pub binding_id: BindingId,
    pub name: String,
    pub kind: BindingKind,
    pub resource_id: ResourceId,
    pub resource_spec_generation: u64,
    pub capability_version: u32,
    pub permissions: CanonicalPermissions,
    pub config: CanonicalBindingConfig,
}
```

Canonical 规则：

- 按 `name.as_bytes()` 排序；重复 name 失败；
- enum 使用固定小写字符串；未知 field/version fail closed；
- `permissions` 和 `config` 按 kind 使用 typed struct，不接受任意 JSON bag；
- JSON object key 通过 typed serialization 固定顺序；
- 不包含 display name、文件路径、S3 prefix、credential、token 或健康状态；
- descriptor canonical bytes 的 SHA-256 写入 row，并进入
  `WorkerCodeDescriptorV1.binding_descriptors`；
- 任一 binding 字段变化都产生新的 deployment/`worker_code_sha256`。

`resource_id` 出现在 loader-side props 中是路由完整性信息，但 tenant-facing method 永远不接收
resource ID。更敏感的物理 locator 只存在于 product table 和 driver 内。

## 7. Deploy 集成

### 7.1 Control input

扩展 deployment create payload：

```json
{
  "bindings": {
    "CACHE": {
      "type": "kv_namespace",
      "id": "019..."
    }
  }
}
```

Control API 接受 resource ID，不接受 name。UI/CLI 可以先 list/resolve name，但最终 mutation 必须
提交 ID，避免 rename 或同名重建竞态。

### 7.2 Staging transaction

部署 controller 在同一个 `control.sqlite` transaction 中：

1. 创建 staging deployment；
2. canonicalize vars/secrets/binding names 并检查全集冲突；
3. 对每个 binding 查询同 account、kind、ready resource；
4. 分配 `BindingId`，冻结 resource ID/generation/capability/config；
5. 插入 immutable binding rows 和 resource referrers；
6. 构造完整 descriptor，保存 `worker_code_sha256`；
7. 提交后进入现有 artifact/runtime validation 流程。

Validation scope 不注入真实 binding，防止一个尚未 ready 的 deployment 在 validation handler 中
修改生产资源。若某产品未来需要验证 named export/class，只传 schema-only fake 或走独立 Probe
scope；P0.3/P0.4 一律不执行真实数据操作。

### 7.3 Secret-only deploy 与 rollback

- secret/var/binding 任何变化都创建新 deployment；
- secret-only deployment 可以由 controller 显式复制上一 deployment 的 binding rows，但每个新
  row 使用新的 BindingId，且重新验证资源仍 ready；
- rollback 选择旧 deployment，因此也恢复它冻结的旧 binding set；
- 旧 resource 只要仍被 retained deployment 引用就不能删除；
- retention 删除旧 deployment 后，transaction 同步释放 resource referrer。

## 8. RuntimeSource 与 loader host

### 8.1 Runtime payload

`RuntimeSnapshot` 增加 canonical bindings；`RuntimePayload` 增加：

```json
{
  "bindings": [
    {
      "schemaVersion": 1,
      "bindingId": "019...",
      "name": "CACHE",
      "kind": "kv_namespace",
      "resourceId": "019...",
      "resourceSpecGeneration": 1,
      "capabilityVersion": 1,
      "permissions": { "read": true, "write": true },
      "config": {}
    }
  ]
}
```

RuntimeSource 在每次请求、包括 warm `LOADER.get()` 不触发 callback 的路径上，重新从 immutable
rows 构造 descriptor 并比较 hash。未知 kind/version、row hash mismatch、resource generation
mismatch 或 binding referrer 缺失都返回 `DEPLOYMENT_INVARIANT_VIOLATION`，不能继续使用 warm
isolate。

### 8.2 静态 BindingFactory

把 `loader-host.js` 中的 env assembly 拆成小型静态 factory；不是 plugin loader：

```js
function makeBinding(ctx, descriptor) {
  switch (`${descriptor.kind}@${descriptor.capabilityVersion}`) {
    case "kv_namespace@1":
      return ctx.exports.KVNamespace({ props: trustedProps(descriptor) });
    default:
      throw stable("BINDING_CAPABILITY_UNSUPPORTED");
  }
}
```

`trustedProps()` 只从 RuntimeSource snapshot 生成：

```text
bindingId
deploymentId
descriptorSha256
resourceSpecGeneration
```

产品 adapter class 必须在 loader-host module graph 中静态导出，使 `ctx.exports` 能建立 JSRPC
stub。Tenant 的 `env` 是：

```js
{
  ...vars,
  ...secrets,
  [binding.name]: typedStub
}
```

assembly 前再次检查所有 env name 唯一，不能依赖 JS object 后写覆盖前写。

### 8.3 为什么不用一个通用 `ResourceBinding`

通用 `call(method, args)` 会扩大攻击面、让 capability discovery 和 size validation散落在 backend，
也很难还原 Cloudflare 常用 API。每个产品使用 typed adapter：KVNamespace、R2Bucket、D1Database、
DurableObjectNamespace。共享的是鉴权、transport、pin、预算和 error envelope，不共享一个任意方法
入口。

## 9. Private BindingBackend

### 9.1 workerd 配置

在现有 config 增加一个独立 external service，并只绑定给 loader host：

```capnp
(name = "binding-backend", external = (http = ()))

bindings = [
  # existing LOADER / RUNTIME_SOURCE
  (name = "BINDING_BACKEND", service = "binding-backend"),
  (name = "BINDING_BACKEND_TOKEN", text = "__GENERATION_TOKEN__"),
]
```

`platformd` 为每个 workerd generation 启动 loopback-only listener，通过
`--external-addr binding-backend=127.0.0.1:<ephemeral>` 注入。token 随 supervisor generation
随机生成，绝不进入 tenant env、DB、日志或命令行。adapter 调用时把它放进固定 internal header；
backend constant-time 验证。

即使 tenant global `fetch()` 试图调用 loopback，P0.2 的 public-only egress 也会拒绝；即使同机
非特权进程猜到端口，没有 generation token 也会失败。RuntimeSource 和 BindingBackend 使用不同
service capability/token，互不代理任意 path。

### 9.2 产品化、版本化 endpoint

内部 endpoint 必须是静态 route，例如：

```text
POST /internal/bindings/v1/kv/{binding-id}/get
POST /internal/bindings/v1/kv/{binding-id}/put
POST /internal/bindings/v1/kv/{binding-id}/delete
POST /internal/bindings/v1/kv/{binding-id}/list
```

禁止：

```text
POST /internal/resource/{resource-id}/call
{ "method": "...", "args": ... }
```

每个 route 在读取完整 body 前先完成 token、method、content-length 上限和 binding lookup。缺失
`Content-Length` 的 stream 也使用 incremental counter 强制上限。

### 9.3 Internal envelope

请求 header 固定包含：

```text
x-open-compute-binding-token
x-open-compute-deployment-id
x-open-compute-descriptor-sha256
x-open-compute-request-id
content-type: application/vnd.open-compute.<product>.v1+...
```

backend 以 URL 中的 binding ID 查询 authority，header identity 只做一致性检查。响应只返回：

```json
{
  "ok": false,
  "error": {
    "code": "RESOURCE_UNAVAILABLE",
    "retryable": true,
    "resultUnknown": false
  }
}
```

raw SQLite/S3/workerd error 只进入 host-side structured log 的 redacted class，不进入 tenant。

## 10. Adapter 与 RPC 语义

Cloudflare Workers RPC 支持 `Request`、`Response` 和 byte-oriented `ReadableStream`/`WritableStream`
的 flow control，因此大 value/body 必须走 stream，不应 base64 塞进 JSON。普通 serialized RPC
message 有 32 MiB 上限，产品还要使用更小的业务上限。

参考：[RPC lifecycle](https://developers.cloudflare.com/workers/runtime-apis/rpc/lifecycle/) 与
[RPC reserved methods](https://developers.cloudflare.com/workers/runtime-apis/rpc/reserved-methods/)。

Adapter 统一规则：

- JS 参数错误在发 backend 请求前抛 `TypeError`；
- 所有 I/O method 都是 async；
- 只允许 byte-oriented stream，不传 object stream；
- stream cancel 必须向 backend 传播并释放 resource pin/connection；
- tenant 不能通过 prototype name、reserved RPC method 或 symbol 绕过 method allowlist；
- adapter 不在 module-global 保存无界 Map；可缓存的只是不含 payload 的小型 immutable metadata；
- backend response 超出 method budget 时 fail closed 并 abort body；
- `waitUntil()` 不延长 resource delete 到无限期，只有实际 backend call/stream 持 pin。

## 11. Resource driver

P0.3 的 driver 是 Rust 编译期静态 enum/registry，不是扩展系统：

```rust
trait ResourceDriver {
    fn kind(&self) -> BindingKind;
    fn create(&self, ctx: &CreateContext) -> Result<DriverIdentity, PlatformError>;
    fn reconcile(&self, resource: &ResourceRecord) -> Result<ReconcileOutcome, PlatformError>;
    fn begin_delete(&self, resource: &ResourceRecord) -> Result<(), PlatformError>;
    fn finalize_delete(&self, resource: &ResourceRecord) -> Result<(), PlatformError>;
    fn health(&self, resource: &ResourceRecord) -> Result<ResourceHealth, PlatformError>;
}
```

接口不直接暴露给 tenant，也不允许 resource kind 选择动态 library。产品数据操作不塞进这个
lifecycle trait；KV get/put 属于 `KvEngine`，R2 head/put 属于 `R2Engine`。

## 12. Lifecycle 与 crash recovery

### 12.1 Create

标准 create 状态机：

```text
transaction A: insert resources(state=creating) + product metadata
        ↓
driver: create staging physical object, fsync/validate, atomic rename/publish
        ↓
transaction B: verify driver identity, state=ready
```

请求使用 idempotency key。重试读到同一 resource ID/state 后继续 reconcile，不再分配新物理
identity。

| Crash point | 启动/重试行为 |
| --- | --- |
| transaction A 前 | 无资源；安全重试 |
| A 后、driver 前 | reconcile 创建物理对象 |
| staging 写到一半 | driver 清理明确属于该 ID 的 staging 后重建 |
| publish 后、B 前 | probe 验证 identity/schema 后标记 ready |
| B 后响应丢失 | idempotency readback 返回已创建 resource |

`creating` resource 不能被 deployment 绑定。

### 12.2 Rename

Rename 只修改 display name：

- 单 transaction 检查 account/kind 唯一；
- 不改 ID、generation、product physical mapping 或旧 descriptor；
- 不需要 reload Worker；
- 审计记录 old/new name，但 runtime log 仍只用 ID。

### 12.3 Delete

标准 delete：

1. transaction 检查 lifecycle、`resource_referrers` 为零；
2. `ResourcePins.begin_delete(resource_id)` 阻止新调用并等待当前调用/stream 有界 drain；
3. transaction 把 `ready` 改为 `deleting`；
4. driver 把物理对象移到同文件系统 quarantine/trash 或删除 virtual mapping；
5. verify 不再可打开；
6. transaction 删除 product live metadata、把 resource 标为 tombstoned；
7. 异步、可恢复地清理 quarantine。

存在 retained deployment binding 时第 1 步返回精确 referrer，不允许 `force=true` 绕过。P0 不提供
级联删除；调用方先删除/retire deployments。

`ResourcePins` API 与 `DeploymentPins` 对齐：

```rust
let pin = pins.try_pin(resource_id)?; // deleting fence 后失败
let result = engine.operation(...).await;
drop(pin);
```

process crash 会清空内存 pin，但 driver/SQLite/S3 自己的事务语义保证未提交操作不可见；启动
reconciler 继续 `deleting`。同名重建获得新 ID，旧 descriptor 永远不会指向新物理对象。

### 12.4 Delete crash matrix

| Crash point | 恢复动作 |
| --- | --- |
| fence 前 | state 仍 ready，正常服务 |
| state=deleting 后、driver 前 | 新调用被 DB lifecycle 拒绝，reconciler执行 driver delete |
| quarantine 后、tombstone 前 | probe 识别 live path 不存在、trash 属于同 ID，继续 tombstone |
| tombstone 后、trash 清理前 | identity 已不可访问，后台幂等清理 |
| client response 前 | idempotency readback 返回 tombstoned |

### 12.5 Availability 与 repair

Driver 发现局部损坏或 provider 故障时：

- lifecycle 保持 `ready`；
- 持久化 `availability=degraded|unavailable` 和 stable code；
- 只让该 resource 的调用失败，不使平台整体 `/health/ready` 失败；
- operator health/doctor 能列出 resource ID、kind、code，不输出物理 secret；
- health 恢复需成功 probe 后清除，不能仅靠 timer；
- destructive repair/restore 由产品方案定义，P0.3 不自动重建 tenant data。

## 13. Concurrency、connection 与 cache contract

P0.3 只定义 contract，具体 pool 在产品阶段实现：

- cache key 必须是 `ResourceId + spec_generation`；禁止 display name；
- 所有 handle 有全局上限和 per-resource 上限；
- cache miss 使用 per-key singleflight；
- handle checkout 同时持 `ResourcePin`；
- LRU eviction 先停止新 checkout，再等待有界 active count，最后 checkpoint/close；
- `deleting`/`unavailable` resource 不创建新 handle；
- driver open 后验证文件内 identity/schema，不能只信路径；
- payload、key 和 user metadata 不进入 cache key/log/metric label；
- backend task 不在 Tokio core thread 执行阻塞 SQLite I/O。

## 14. Error model

在 `ErrorCode` 增加稳定类别，产品可以继续细分：

| Code | HTTP/RPC 类别 | Retry | 说明 |
| --- | --- | --- | --- |
| `RESOURCE_NOT_FOUND` | not found | no | ID 不存在或不属于 scope；不泄露跨 account 存在性 |
| `RESOURCE_NAME_CONFLICT` | conflict | no | live name 重复 |
| `RESOURCE_NOT_READY` | conflict | maybe | creating/deleting/tombstoned |
| `RESOURCE_REFERENCED` | conflict | no | delete 被 referrer 拦截 |
| `RESOURCE_UNAVAILABLE` | unavailable | yes | provider/局部资源不可用 |
| `RESOURCE_INVARIANT_VIOLATION` | internal | no | identity/schema/catalog 不一致 |
| `BINDING_NOT_FOUND` | internal | no | runtime binding row 缺失 |
| `BINDING_TYPE_MISMATCH` | internal | no | kind/adapter 不一致 |
| `BINDING_PERMISSION_DENIED` | forbidden | no | method 不在 capability 中 |
| `BINDING_CAPABILITY_UNSUPPORTED` | internal | no | 未知 version |
| `BINDING_PROTOCOL_ERROR` | internal | maybe | malformed/truncated internal frame |
| `BINDING_LIMIT_EXCEEDED` | client error | no | 参数/body/result 超出预算 |
| `BINDING_RESULT_UNKNOWN` | unavailable | caller decides | mutation 可能已提交但响应丢失 |

对外不能返回“foreign resource exists”；authorization failure 和不存在统一为
`RESOURCE_NOT_FOUND`。Host structured log 可以有 internal cause chain，但必须走现有 redaction，
不得打印 body、key、metadata、credential、DB path 或 raw SQL。

## 15. Observability

新增低基数 metrics：

```text
resource_operations_total{kind,operation,outcome}
resource_operation_duration_seconds{kind,operation}
resource_open_handles{kind}
resource_pin_wait_seconds{kind}
resource_reconcile_total{kind,state,outcome}
binding_backend_requests_total{kind,operation,outcome}
binding_backend_bytes_total{kind,direction}
binding_protocol_errors_total{kind}
```

禁止把 account/resource/binding/deployment ID、binding name、KV key 放进 metric label。Tracing/log
可以记录 ID 作为 field，并沿用 request ID 串起 ingress -> loader -> adapter -> backend。

## 16. 工作包

### P0.3.0：Types、migration 与 repository

- `ResourceId`、`BindingId`、`BindingKind`、lifecycle/availability enum；
- `003_resource_bindings.sql`、trigger、migration rollback/重复启动测试；
- `ResourceRepository`、typed records、referrer query；
- stable errors 和 audit event。

完成条件：DB 直接写和 repository 两条路径都无法破坏 account/kind/state/immutability invariant。

### P0.3.1：Lifecycle controller 与 fake driver

- create/get/list/rename/delete 的内部 product-controller primitive；
- idempotency、creating/deleting reconciler；
- `ResourcePins`、delete fence、bounded drain；
- 编译期 fake driver，仅 test build/feature 可创建。

完成条件：create/delete 每个 crash boundary 重启都收敛；同名重建不复用 ID。

### P0.3.2：Deployment binding 与 descriptor

- create deployment payload 增加 typed bindings；
- staging transaction、env collision、referrer；
- `BindingDescriptorV1` canonical serialization/hash；
- secret-only copy、retention release；
- validation/probe 不注入真实 binding。

完成条件：任意 binding row tamper 都让 RuntimeSource fail closed，active deployment 不被替换。

### P0.3.3：RuntimeSource 与 BindingFactory

- snapshot/payload 增加 typed binding；
- loader-host 静态 factory 与 fake `WorkerEntrypoint` adapter；
- cold/warm descriptor check；
- method allowlist、props isolation、RPC stream/cancel smoke。

完成条件：tenant 只能调用 fake 产品方法，不能读取 props/env/backend 或构造任意 resource 调用。

### P0.3.4：BindingBackend

- generation-local external service/token；
- typed route、binding authorization、size/time budgets；
- resource pin 与 driver dispatch；
- stable envelope、result-unknown 分类、redacted logging。

完成条件：伪造 binding/deployment/hash/token、跨 account ID 和 oversized body 全部 fail closed。

### P0.3.5：Lifecycle/health/metrics integration

- handle cache contract、availability state、doctor；
- startup reconcile 和 supervisor generation restart；
- metrics/tracing/audit；
- deletion 与 active stream 的 race tests。

完成条件：backend/workerd/platformd restart 后没有 stale handle、token、pin 或端口泄漏。

### P0.3.6：真实 workerd Gate

使用 stock pinned workerd，不 mock RPC/WorkerLoader。Gate 连续三轮 fresh process，至少覆盖：

| ID | 场景 | 断言 |
| --- | --- | --- |
| RB-01 | no binding regression | P0.2 Worker cold/warm/rollback 不变 |
| RB-02 | fake binding cold/warm | 相同 immutable deployment 两条路径结果一致 |
| RB-03 | env collision | var/secret/binding 同名在 staging 失败 |
| RB-04 | physical ID freeze | resource rename 后旧 deployment 仍访问原 ID |
| RB-05 | same-name recreate | 新 resource ID 不被旧 deployment 访问 |
| RB-06 | cross-account binding | deploy 阶段返回 not found，不泄露存在性 |
| RB-07 | descriptor tamper | warm path 也拒绝，active 不受影响 |
| RB-08 | forged adapter request | token/binding/deployment/hash 任一伪造均失败 |
| RB-09 | props isolation | tenant 无法枚举 resource ID/internal token/backend |
| RB-10 | permission | read-only binding 的 mutation 在 adapter/backend 都拒绝 |
| RB-11 | byte budget | request/result/stream 超限及时 cancel 且不泄漏 pin |
| RB-12 | delete referenced | retained deployment 存在时不能删除 resource |
| RB-13 | delete drain | in-flight stream 完成/超时后才进入物理删除 |
| RB-14 | lifecycle crash | create/delete 每个 crash point 重启后收敛 |
| RB-15 | backend restart | generation token 轮换，旧 adapter call 不可越权 |
| RB-16 | isolated failure | fake resource unavailable 不影响其他 Worker/resource |
| RB-17 | logs | payload/path/token/raw error 不进入 stdout/stderr/audit |
| RB-18 | leak audit | child、listener、temp file、pin、handle 全部归零 |

## 17. P0.3 Exit Gate

进入 P0.4 前必须同时满足：

- migration/repository/lifecycle unit tests 全通过；
- fake driver 的 crash recovery 和 delete fence 通过；
- descriptor tamper 在 cold/warm path 都 fail closed；
- stock workerd 中 custom binding 的 RPC、stream、cancel、props isolation 通过；
- backend 只可通过受控 service capability + generation token 调用；
- retained deployment referrer 能稳定阻止 resource 删除；
- 三轮 fresh-process RB matrix 通过，无子进程/端口/临时文件泄漏；
- P0.2 Gate 无回归。

P0.3 通过后，P0.4 只实现 `kv_namespace` 的产品 controller、driver、engine 和 adapter，不再修改
上述通用 lifecycle/auth/transport 协议。若 KV 实现过程中必须绕过 framework，视为 P0.3 Gate
不完整，应回到本阶段补 contract，而不是在 KV 中增加旁路。

## 18. 参考资料

- [Cloudflare Dynamic Workers：Bindings](https://developers.cloudflare.com/dynamic-workers/usage/bindings/)
- [Cloudflare Dynamic Workers：API reference](https://developers.cloudflare.com/dynamic-workers/api-reference/)
- [Cloudflare Workers RPC](https://developers.cloudflare.com/workers/runtime-apis/rpc/)
- [Cloudflare Service bindings RPC](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/rpc/)
- [Cloudflare Workers RPC lifecycle](https://developers.cloudflare.com/workers/runtime-apis/rpc/lifecycle/)
- [workerd configuration schema](https://github.com/cloudflare/workerd/blob/main/src/workerd/server/workerd.capnp)
- [总体方案](./sqlite-workerd-platform.md)
