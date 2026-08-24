# P0.2：Workers Runtime 详细设计

> 状态：Implementation Plan
>
> 前置依赖：[P0.1：Platform Foundation](./p0-1-platform-foundation.md)
>
> 验证基线：[G0 results](./g0-results.md) 与 [`../poc/`](../poc/README.md)
>
> 本文只实现 Workers 的第一条完整数据路径。真实 KV、R2、D1、Durable Object binding 从
> P0.3 开始接入；本阶段只保留无 binding Worker、vars、secrets 和一个测试专用 fake adapter。

## 1. 交付目标

P0.2 在 P0.1 的宿主、SQLite、S3 artifact、cache 和 supervisor 上实现：

```text
create Worker
    └── deploy immutable version
            ├── validate bundle/runtime startup
            ├── retain A/B versions
            └── atomically promote active pointer

public HTTP
    └── resolve route + freeze deployment
            └── workerd loader host
                    ├── LOADER.get(immutable key, callback)
                    ├── getEntrypoint(name)
                    └── fetch(request)
```

完成后，用户可以创建一个 module Worker，提交带 compatibility metadata、vars 和 encrypted
secrets 的不可变 deployment，通过 active route 执行 `fetch()`，在 A/B 版本之间 promotion/
rollback，并在 `platformd` 或 `workerd` restart 后继续工作。

### 1.1 完成定义

- Worker、deployment、route 和 secret metadata 持久化在 `control.sqlite`；
- bundle 是 canonical、content-addressed、存于 P0.1 ArtifactStore 的不可变对象；
- deployment 的所有 runtime-effective 输入在 ready 后不可修改；
- active deployment 只通过单个 SQLite transaction 切换；
- `workerLoader` key 使用 `<account-id>/<worker-id>/<deployment-id>`，不使用 name/`active`；
- cold/warm load、default/named entrypoint、streaming request/response 均通过真实 workerd；
- invalid deployment、缺失 artifact 或 runtime validation failure 不改变当前 active；
- platformd 生成 request/deployment identity，tenant header/body 不能覆盖；
- tenant global `fetch()` 只能访问 public network，不能访问 loopback/private/internal service；
- client disconnect 不被当作 tenant execution 已 abort 的保证；
- vars 以 structured-clone-compatible 值进入 `env`，secrets 在 DB/S3/log 中不出现明文；
- deployment retention/delete 有 referrer fence，不能删除 active 或仍被引用的版本；
- P0.2 regression matrix 连续三轮 fresh process 通过且无 child/port/file leak。

### 1.2 非目标

- 完整 Cloudflare REST API、Wrangler deploy protocol 或 dashboard；
- Service Worker syntax；P0.2 只支持 ES module Worker；
- Python/Pyodide Worker；
- source map upload、build/transpile/bundle 或 npm dependency resolution；
- KV、D1、R2、DO、Queue、Workflow、Cron、Assets binding；
- service binding、tail consumer、analytics engine、Workers AI；
- raw TCP `connect()`；P0.2 只开放 public HTTP(S) `fetch()`；
- edge route propagation、canary percentage/traffic split 或跨节点调度；
- loader cache 主动 eviction/热更新；immutable key 使其不成为正确性前提；
- client disconnect 的完整 abort 语义。

## 2. G0 证据与必须保留的不变量

G0 的 Loader hard matrix 在三轮 fresh process 中验证了 cold/warm load、A/B coexist、promotion、
rollback、restart cold load、cold concurrency、named entrypoint、streaming 和错误隔离。P0.2 不重新
发明调度模型，而是把静态 fixture/内存 registry 替换为正式 authority。

| G0 证据 | P0.2 不变量 |
| --- | --- |
| A、B 使用不同 loader key 可同时存在 | 每个 deployment ID 永久映射同一份 `WorkerCode` |
| promotion/rollback 只改 active route | 不做 loader cache invalidation，不复用 deployment ID |
| warm `LOADER.get()` 不再次调用 callback | hash/invariant check 必须发生在调用 `get()` 之前 |
| cold concurrency 只装配一次 | loader host 按 immutable key singleflight |
| restart 后 cold load 可恢复 | bundle、metadata、vars/secrets 必须来自持久 authority，而非 host memory |
| active route 忽略 body 中伪造 deployment | deployment 由 platformd 在 route snapshot 中确定 |
| identity header 会被剥离，request ID 由 host 生成 | internal envelope/header 在 ingress 边界覆盖，不能透传外部同名值 |
| invalid bundle 被 containment，active 不受影响 | 部署先进入 staging，runtime validation 通过后才可 ready/promote |
| `globalOutbound: null` 能可靠拒绝出站 | 正式开放出站必须只绑定受限 public network capability |
| loader error/log 可脱敏 | control API 和 tenant response 使用稳定 code，不返回 raw workerd message |
| `D-abort` 未通过 | 断连只停止 proxy I/O；资源 limits/timeout 才是执行终止边界 |

G0 的 `callbackCounts`、route map、fixture registry 和 fault endpoints 都是测试设施，不能进入生产
authority。P0.2 用 `control.sqlite`、ArtifactStore 和内部 RuntimeSource service 替换它们。

## 3. 总体架构

```text
                         CONTROL PLANE

Control API ──> platformd ──> control.sqlite
                    │                │
                    │                └─ worker/deployment/route/secret authority
                    ├── validate/canonicalize bundle
                    ├── ArtifactStore ──> S3 + verified cache
                    └── RuntimeValidator ──> workerd validation entrypoint

                          DATA PLANE

Client ──> platformd public ingress
                    │
                    ├── route snapshot -> immutable deployment ID
                    ├── strip/replace internal identity
                    └── stream over internal transport
                              │
                              ▼
                    workerd ingress/loader host
                              │
                 LOADER.get(loaderKey, callback)
                              │ cold only
                              ▼
          RuntimeSource binding -> platformd internal service
                              │
                       manifest/modules/env
                              │
                              ▼
                  loaded tenant Worker fetch()
                              │
                              └── globalOutbound -> public-only network
```

### 3.1 Authority 划分

| 内容 | Authority | Cache/projection |
| --- | --- | --- |
| Worker identity/name/lifecycle | `control.sqlite` | platformd bounded lookup cache |
| deployment state/version/runtime metadata | `control.sqlite` | loader host immutable snapshot |
| active pointer/route generation | `control.sqlite` | platformd route snapshot cache |
| bundle bytes | S3 ArtifactStore | local verified artifact cache |
| vars | immutable deployment rows | RuntimeSource response |
| secret ciphertext | immutable deployment rows | 明文只在一次 assembly 内短暂存在 |
| loaded isolate | workerd `workerLoader` cache | 非 authority，可随 restart 丢失 |
| in-flight request | platformd/workerd memory | 不持久化、不自动 replay |

### 3.2 请求路径上的唯一可信部署 ID

`platformd` 在接收请求时完成 route lookup，并在一个 route snapshot 中冻结：

```text
account_id
worker_id
deployment_id
route_id
route_generation
entrypoint
request_id
```

之后即使 promotion 发生，本请求仍执行已冻结的 deployment。workerd loader host 不自行查询
“当前 active”；它只接受 platformd 已确定的 immutable identity。这样 route change 不与 loader
cache 或长请求竞争。

## 4. `control.sqlite` schema

以下是 P0.2 要落地的物理逻辑；migration 可按实现拆分，但列、不变量和索引不能靠应用层约定
替代。

### 4.1 Workers

```sql
CREATE TABLE workers (
  id                    TEXT PRIMARY KEY,
  account_id            TEXT NOT NULL REFERENCES accounts(id),
  name                  TEXT NOT NULL,
  active_deployment_id  TEXT,
  do_storage_id         TEXT NOT NULL,
  route_generation      INTEGER NOT NULL DEFAULT 0 CHECK(route_generation >= 0),
  created_at_ms         INTEGER NOT NULL,
  updated_at_ms         INTEGER NOT NULL,
  deleted_at_ms         INTEGER,
  CHECK(length(name) BETWEEN 1 AND 63)
) STRICT;

CREATE UNIQUE INDEX workers_live_name
ON workers(account_id, name)
WHERE deleted_at_ms IS NULL;
```

`do_storage_id` 在创建 Worker 时就生成，P0.7 直接复用；普通 deployment 不改变它。Worker 删除后
同名重建产生新的 worker ID 和 DO storage ID。

### 4.2 Deployments

```sql
CREATE TABLE worker_deployments (
  id                       TEXT PRIMARY KEY,
  worker_id                TEXT NOT NULL REFERENCES workers(id),
  version_number           INTEGER NOT NULL CHECK(version_number > 0),
  state                    TEXT NOT NULL CHECK(state IN (
                             'staging', 'validating', 'ready',
                             'rejected', 'deleting', 'tombstoned'
                           )),
  artifact_sha256          BLOB NOT NULL CHECK(length(artifact_sha256) = 32),
  artifact_size            INTEGER NOT NULL CHECK(artifact_size >= 0),
  artifact_schema_version  INTEGER NOT NULL,
  main_module              TEXT NOT NULL,
  compatibility_date       TEXT NOT NULL,
  compatibility_flags_json BLOB NOT NULL,
  limits_json              BLOB NOT NULL,
  worker_code_sha256       BLOB NOT NULL CHECK(length(worker_code_sha256) = 32),
  loader_schema_version    INTEGER NOT NULL,
  created_at_ms            INTEGER NOT NULL,
  ready_at_ms              INTEGER,
  rejected_at_ms           INTEGER,
  rejection_code           TEXT,
  deleted_at_ms            INTEGER,
  UNIQUE(worker_id, version_number)
) STRICT;

CREATE INDEX deployments_worker_state
ON worker_deployments(worker_id, state, version_number DESC);
```

`artifact_sha256` 标识 bundle 内容；`worker_code_sha256` 标识所有 runtime-effective 输入，详见
第 8 节。两者不能混为一谈。

`workers.active_deployment_id` 通过 migration 后加 deferred foreign key 或 trigger 保证：目标存在、
属于同一 Worker 且 state=`ready`。SQLite 无法用普通 FK 表达所有条件，因此 promotion transaction
必须执行显式条件 UPDATE，另有 invariant checker 验证。

### 4.3 Vars 和 secrets

```sql
CREATE TABLE deployment_vars (
  deployment_id  TEXT NOT NULL REFERENCES worker_deployments(id),
  name           TEXT NOT NULL,
  value_json     BLOB NOT NULL,
  PRIMARY KEY(deployment_id, name)
) STRICT, WITHOUT ROWID;

CREATE TABLE deployment_secrets (
  deployment_id  TEXT NOT NULL REFERENCES worker_deployments(id),
  name           TEXT NOT NULL,
  revision_id    TEXT NOT NULL,
  key_id         TEXT NOT NULL,
  algorithm      TEXT NOT NULL,
  nonce          BLOB NOT NULL,
  ciphertext     BLOB NOT NULL,
  PRIMARY KEY(deployment_id, name)
) STRICT, WITHOUT ROWID;
```

同一个 deployment 不能同时存在同名 var、secret 或 binding。P0.2 在 transaction 中校验冲突；
P0.3 引入 binding table 后把这一 invariant 扩展到所有 env names。

### 4.4 Routes

P0.2 提供一个总能工作的平台默认 route，并允许最小 exact-host route：

```sql
CREATE TABLE worker_routes (
  id               TEXT PRIMARY KEY,
  account_id       TEXT NOT NULL REFERENCES accounts(id),
  worker_id        TEXT NOT NULL REFERENCES workers(id),
  kind             TEXT NOT NULL CHECK(kind IN ('platform_path', 'exact_host')),
  hostname_ascii   TEXT,
  path_prefix      TEXT NOT NULL,
  entrypoint       TEXT,
  state            TEXT NOT NULL CHECK(state IN ('active', 'disabled', 'tombstoned')),
  generation       INTEGER NOT NULL CHECK(generation > 0),
  created_at_ms    INTEGER NOT NULL,
  updated_at_ms    INTEGER NOT NULL,
  deleted_at_ms    INTEGER
) STRICT;

CREATE UNIQUE INDEX live_exact_routes
ON worker_routes(account_id, hostname_ascii, path_prefix)
WHERE kind = 'exact_host' AND state = 'active';
```

默认 route 是平台保留前缀：

```text
/__workers/<account-id>/<worker-name>/*
```

它不依赖外部 DNS，可用于 smoke test。`exact_host` 使用 IDNA/ASCII canonical hostname 和 normalized
path prefix；P0.2 不做 wildcard/zone/priority graph。相同 host 下采用最长 path-prefix match，唯一
冲突在写入 transaction 中拒绝。

route 指向 Worker，而不是直接指向 deployment。每个请求从同一个 committed snapshot 读取
`route.worker_id + workers.active_deployment_id + route_generation`。

### 4.5 Idempotency 与 audit

```sql
CREATE TABLE control_idempotency (
  account_id       TEXT NOT NULL,
  scope            TEXT NOT NULL,
  idempotency_key  TEXT NOT NULL,
  fingerprint_key_id TEXT NOT NULL,
  request_fingerprint BLOB NOT NULL CHECK(length(request_fingerprint) = 32),
  response_json    BLOB,
  state            TEXT NOT NULL CHECK(state IN ('running', 'complete', 'failed')),
  created_at_ms    INTEGER NOT NULL,
  expires_at_ms    INTEGER NOT NULL,
  PRIMARY KEY(account_id, scope, idempotency_key)
) STRICT, WITHOUT ROWID;

CREATE TABLE control_audit_events (
  seq              INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id       TEXT NOT NULL,
  action           TEXT NOT NULL,
  target_type      TEXT NOT NULL,
  target_id        TEXT NOT NULL,
  request_id       TEXT NOT NULL,
  details_json     BLOB NOT NULL,
  created_at_ms    INTEGER NOT NULL
) STRICT;
```

`request_fingerprint` 使用从 master key 派生的独立 HMAC key 计算，`fingerprint_key_id` 允许未来
rotation 继续验证旧 idempotency row；canonical request 中的 secret 值不能以裸 SHA-256 形式落库，
避免低熵值被离线猜测。`details_json` 只保存 digest、version、state transition 和 actor ID，不保存
bundle source、secret、credential 或 raw error。相同 idempotency key 配不同 fingerprint 返回冲突；
相同 fingerprint 返回原 response，不分配新 deployment version。

## 5. 生命周期状态机

### 5.1 Worker

```text
absent ── create ──> live ── delete ──> tombstoned
                         │
                         └── deploy/promote/route mutation
```

P0.2 不支持从 tombstoned 恢复。以相同 name 重建是新 Worker identity。

### 5.2 Deployment

```text
staging ── artifact committed ──> validating ── runtime OK ──> ready
   │                                  │
   └──────── request failure ─────────┴──────────────> rejected

ready ── delete requested/referrer clear ──> deleting ── GC ──> tombstoned
```

active 不是 deployment state，而是 `workers.active_deployment_id` 指针。一个 ready deployment
可以在不同时间多次 active/retained；rollback 不改变 deployment 内容或 ID。

以下 transition 非法：

- rejected -> ready；重试创建新 deployment；
- ready -> staging/validating；
- active target -> deleting；
- tombstoned -> 任意其他状态；
- 修改 ready deployment 的 artifact、compatibility、limits、vars、secrets 或 wrapper version。

## 6. Control API

P0.2 提供本平台稳定 API，不声称完整兼容 Cloudflare management API。所有 mutation 需要 admin
auth、account scope、request ID 和 `Idempotency-Key`。

```text
POST   /v1/accounts/{accountId}/workers
GET    /v1/accounts/{accountId}/workers
GET    /v1/accounts/{accountId}/workers/{workerId}
DELETE /v1/accounts/{accountId}/workers/{workerId}

POST   /v1/accounts/{accountId}/workers/{workerId}/deployments
GET    /v1/accounts/{accountId}/workers/{workerId}/deployments
GET    /v1/accounts/{accountId}/workers/{workerId}/deployments/{deploymentId}
DELETE /v1/accounts/{accountId}/workers/{workerId}/deployments/{deploymentId}

POST   /v1/accounts/{accountId}/workers/{workerId}/promotions
POST   /v1/accounts/{accountId}/workers/{workerId}/rollbacks

POST   /v1/accounts/{accountId}/workers/{workerId}/routes
GET    /v1/accounts/{accountId}/workers/{workerId}/routes
DELETE /v1/accounts/{accountId}/workers/{workerId}/routes/{routeId}
```

### 6.1 Create Worker

请求只包含 name；服务端生成 worker ID、DO storage ID 和 default platform-path route。名称：

- 采用 lowercase ASCII slug；
- `1..63` bytes；
- 首尾字母/数字，中间允许 `-`；
- 同 account live name 唯一；
- 不进入 loader key 的 authority，只用于 lookup/default path。

返回值必须包含 opaque IDs。客户端不能自行指定 ID 或复用已删除 identity。

### 6.2 Create Deployment

推荐使用 `multipart/form-data`：一个 JSON metadata part、一个 bundle part；实现也可以先用 streaming
binary body + headers，但必须避免把整个 bundle buffer 到内存。

metadata 最小形态：

```json
{
  "mainModule": "src/index.js",
  "compatibilityDate": "2026-08-22",
  "compatibilityFlags": ["nodejs_compat", "rpc"],
  "vars": {"MODE": "production"},
  "secrets": {"API_TOKEN": "<write-only>"},
  "limits": {"profile": "default"},
  "promote": false
}
```

response 不返回 secret。`promote=true` 仍然先完成完整 validation，再在独立 transaction 中 promote；
失败不改变 active。

### 6.3 Secret mutation

ready deployment 不可原地修改 secret。若产品提供“更新 secret”便利 API，它必须在服务端创建一个
新 deployment：复用当前 bundle和非 secret metadata，复制旧 secret set 后替换指定值，完成
runtime validation，再由调用者选择 promote。这样 loader key 与 `WorkerCode` 始终不可变。

## 7. Worker Bundle V1

### 7.1 输入格式与 canonical representation

API 输入可以是 archive，但 archive 不是 authority。platformd 必须解析成 canonical
`WorkerBundleV1`：

```json
{
  "schemaVersion": 1,
  "mainModule": "src/index.js",
  "modules": [
    {
      "name": "src/index.js",
      "type": "esModule",
      "sha256": "<hex>",
      "size": 1234,
      "offset": 0
    }
  ]
}
```

artifact 内由一个 canonical manifest 加按 manifest 顺序排列的 raw module bytes 构成。编码必须：

- 有 magic/version；
- 对整数、UTF-8 和 key order 有唯一编码；
- 以 length-prefix 读取，不依赖 archive path extraction；
- manifest 本身有 size upper bound；
- 整个 artifact SHA-256 是 bundle content identity；
- reader 在每个 module 与 whole artifact 层面都校验 digest/size。

可以选 canonical CBOR 或自描述的 manifest+blob framing，但同一 schemaVersion 必须只有一种 byte
representation。禁止把 zip/tar 原样作为 loader authority，避免 path traversal、重复文件名、
symlink、compression bomb 和 provider-dependent canonicalization。

### 7.2 支持的 module types

P0.2 allowlist：

```text
esModule
commonJsModule
text
data
json
wasm
```

主模块必须是 `esModule`；是否允许 `commonJsModule` 由 compatibility flag 和当前 workerd 能力决定。
Service Worker script、Python module、Node native addon 和任意 filesystem import 均拒绝。

### 7.3 Module name 规则

- UTF-8，先做约定的 Unicode normalization；
- `/` 作为唯一分隔符，不接受 `\`；
- 不允许空段、`.`、`..`、leading `/`、trailing `/`、NUL 或 control character；
- canonical name byte-wise 唯一，case-sensitive；
- mainModule 必须精确匹配一个 module；
- module count、单 module bytes、总 raw bytes、manifest bytes 都有平台配置 limit；
- archive compressed size 与 expanded size 分别计数，超限立即停止解析；
- 不将 module name 映射成本地任意路径；canonical artifact reader 只按 offset 读。

### 7.4 结构校验

在创建 staging row 前完成：

- Content-Type/schemaVersion；
- 请求和展开 size budget；
- name/type/duplicate/main module；
- manifest offset 不重叠且完全覆盖声明 bytes；
- module/whole SHA-256；
- compatibility date 格式和 allowlist flags；
- env name/JSON value/secret size；
- limits 只能引用平台支持的 profile/字段；
- 禁止 tenant 提供 loader key、deployment ID、globalOutbound 或内部 binding。

## 8. Deployment 不可变性

### 8.1 Loader key

```text
<account-id>/<worker-id>/<deployment-id>
```

三个 ID 都由平台生成并匹配固定 grammar。loader host 只接受解析后与 RuntimeSource metadata 一致的
key；任何额外 path segment、percent decoding 或 Unicode alternate form 都拒绝。

### 8.2 `worker_code_sha256`

仅 bundle digest 不足以保证 `LOADER.get(key)` 每次返回同一 Worker。P0.2 对 canonical descriptor
计算 SHA-256：

```text
WorkerCodeDescriptorV1 = {
  loaderKey,
  artifactSha256,
  artifactSchemaVersion,
  mainModule,
  orderedModules[{name,type,sha256,size}],
  compatibilityDate,
  sortedCompatibilityFlags,
  canonicalVars,
  secretRevisions[{name,revisionId,ciphertextSha256}],
  bindingDescriptors,
  limits,
  globalOutboundPolicyVersion,
  loaderSchemaVersion
}
```

secret 明文不进入 descriptor；每个 secret row 在 deployment 创建时生成不可变的随机
`revision_id`，descriptor 保存排序后的 name、revision 和 ciphertext SHA-256。任何 secret value 或
ciphertext 变化都必须创建新 deployment；P0 不对 ready deployment 做原地重加密，master-key
rotation 需要保留旧 key 或在后续协议中创建新 deployment。ciphertext tamper 同时由 descriptor 与
AEAD 检出。RuntimeSource 每次 assembly 都重建 descriptor，并在调用 `LOADER.get()` **之前** 与 DB 的
`worker_code_sha256` 比对。原因是 warm key 不会调用 loader callback，若先 `get()` 就无法发现同一
key 后端 metadata 被意外改写。

不匹配返回 `DEPLOYMENT_INVARIANT_VIOLATION`，使该 deployment 不可 dispatch，并触发 readiness/
operator alert；绝不能生成新 WorkerCode 覆盖同 key。

## 9. Create Deployment pipeline

```text
1. authenticate + account/worker lookup
2. reserve idempotency key
3. stream parse and structurally validate input
4. canonicalize bundle; compute module + artifact digests
5. put_verified artifact to P0.1 ArtifactStore
6. BEGIN IMMEDIATE
   - verify Worker still live
   - allocate next version_number
   - insert staging deployment
   - insert vars + encrypted secrets
   - insert audit event
   COMMIT
7. state staging -> validating
8. perform runtime validation in real workerd
9. if valid: validating -> ready
   else: validating -> rejected with safe code
10. optional promotion transaction
11. complete idempotency response
```

### 9.1 Version allocation

version number 在同一个 `BEGIN IMMEDIATE` transaction 中用 `MAX(version_number)+1` 或 Worker 内
counter 分配。并发 deploy 必须得到不同单调版本；失败/rejected 可以留下 gap，不能复用版本号。

deployment ID 在 upload 前即可生成以支持日志/idempotency，但只有 transaction commit 后才成为
可查询实体。upload 后、DB commit 前 crash 形成 S3 orphan，由 P0.1 grace-period GC 处理。

### 9.2 Runtime validation

结构校验不能发现 V8 parse、module link、Wasm compile 和 top-level initialization error。ready 前
必须让当前 pinned workerd 真实加载 candidate，同时不调用用户 `fetch()`：

1. 使用 candidate immutable deployment key 的 validation namespace；
2. 构造平台拥有的 validation wrapper；
3. wrapper import tenant main module，使 parse/link/top-level initialization 真正发生；
4. wrapper 暴露固定 validation entrypoint，只返回内部 success；
5. validation env 不含 secret 或产品 binding，`globalOutbound=null`；wrapper 只 import module、
   检查必需 export 并返回内部 nonce；
6. 使用严格 startup CPU/memory/wall limit；
7. 只以“wrapper 成功加载并返回内部 nonce”判断通过，不以 tenant handler 2xx 判断；
8. raw error 只进入 redacted diagnostics，API 返回 stable rejection code。

tenant top-level 代码仍会执行，因此 validation isolate 没有网络、磁盘、control service 或 fake
binding capability。若 top-level 无限运行，由 runtime limits 结束。

该 Gate 证明 module 可以 parse/link/initialize 且声明的 export 存在，不证明用户 `fetch()` 一定成功，
也不执行具有应用副作用的 handler。entrypoint constructor 或 handler 的应用错误仍由正式 dispatch
contain；named route 创建时另外对目标 export 做 probe。

validation key 与正式 loader key 必须隔离，避免 validation wrapper 的 WorkerCode 占用正式 key：

```text
validate/<account-id>/<worker-id>/<deployment-id>/<worker-code-sha256>
runtime/<account-id>/<worker-id>/<deployment-id>
```

正式 runtime key 对外仍以第 8.1 节三段逻辑 ID 表示；prefix 是 host 内部 namespace。

### 9.3 Failure behavior

| Failure point | Deployment state | Active pointer | Cleanup |
| --- | --- | --- | --- |
| request/structure invalid | 无 row | 不变 | abort stream/partial |
| artifact upload failed | 无 row | 不变 | partial/orphan cleanup |
| DB insert failed | 无 committed row | 不变 | artifact becomes grace orphan |
| validation parse/startup failed | rejected | 不变 | artifact retained until rejected retention GC |
| validation transport/runtime crash | staging/validating -> rejected or retryable safe state | 不变 | bounded retry with same immutable input |
| optional promotion conflict | ready | 不变 | client may explicitly promote later |

validation transport failure与 tenant code deterministic failure 分开编码。只有前者可在同一 deployment
上有限重试；parse/link/limit failure 直接 rejected。

## 10. RuntimeSource capability

### 10.1 为什么需要 RuntimeSource

G0 loader host 从 read-only fixture disk 读取 module，并在内存 registry 查 metadata。生产中：

- metadata/secret authority 在 `platformd` 的 `control.sqlite`；
- bundle authority 在 S3/verified cache；
- S3 credential 和 DB path 不能给 workerd；
- 因此 static loader host 只获得一个 scoped `RuntimeSource` service binding。

workerd config 中将该 binding 指向一个 `ExternalServer`，address 留空；`platformd` spawn 时用
`--external-addr <service>=127.0.0.1:<ephemeral-internal-port>` 注入实际地址。地址不进入缓存的
binary config，且 external service 只能由显式 binding 调用。

### 10.2 Internal API

RuntimeSource 不是 public Control API。最小协议：

```text
POST /internal/runtime/v1/deployments/resolve
POST /internal/runtime/v1/artifacts/open
```

也可以合并为一个 streaming response，但必须满足：

- 只监听 loopback/Unix socket；
- 使用 P0.1 每次 workerd generation 独立的 internal token；
- request body 只接受 canonical loader key、expected descriptor hash 和 startup generation；
- 服务端重新从 DB 验证 deployment state 与 identity；
- 只允许 ready deployment，validation 使用单独 scope；
- artifact bytes 只能来自 verified ArtifactStore/cache；
- response 有 manifest/module/env budgets，不提供通用 file/S3 fetch；
- request/response header 中的 internal auth 不传入 tenant Worker；
- secret 明文不被响应 tracing/body dump 中间件采集；
- workerd generation 变化后旧 token 立即失效。

### 10.3 Snapshot consistency

RuntimeSource 先在一个只读 SQLite snapshot 中取出 deployment、vars、secrets 和未来 bindings，验证
state/identity/descriptor，再释放 transaction并打开 immutable artifact。由于 ready row 不可修改，
两阶段读取仍得到同一逻辑版本。若 artifact digest 与 row 不符，fail closed。

## 11. Loader host

### 11.1 Load algorithm

```text
dispatch(loaderKey, entrypoint, request)
  1. validate key grammar and internal dispatch envelope
  2. resolve immutable descriptor metadata from RuntimeSource
  3. recompute/check worker_code_sha256 BEFORE LOADER.get
  4. singleflight by loaderKey
  5. LOADER.get(loaderKey, async () => assembleWorkerCode(snapshot))
  6. stub.getEntrypoint(entrypoint, limits)
  7. invoke fetch(request)
  8. map response/error; never expose loader internals
```

步骤 2–3 即使 warm load 也执行。RuntimeSource 可以在 platformd 做 bounded immutable metadata cache，
但 cache key 必须包含 deployment ID + descriptor hash，且 DB migration/restart 可清空。

### 11.2 Assembly

`assembleWorkerCode`：

- 从 verified artifact stream 读取 canonical manifest/module；
- 再次验证 module name/type/digest/size；
- `mainModule` 仍是 tenant 声明的 module；正式 runtime 不插入会改变 export surface 的 wrapper；
- module 集合和顺序按 canonical 规则装配；validation wrapper 只存在于隔离的 validation key；
- 设置 deployment 自己的 compatibility date/flags；
- env 只含 vars、secrets 和当前阶段允许的 fake binding；
- 由 loader host 的 `ctx.exports.OutboundGateway({props})` 创建 scoped stub，并设置为受限
  `globalOutbound`；
- 设置 resource limits；
- 返回 native `WorkerCode`；
- 不把 source、secret 或完整 env 写日志。

P0.2 wrapper 只完成 entrypoint/export 适配和内部 metadata 隔离，不 polyfill 产品 API。tenant 默认
entrypoint 及 named `WorkerEntrypoint` 由 `stub.getEntrypoint(name)` 访问；未知 entrypoint 返回稳定
404/部署错误，不 fallback 到 default。

### 11.3 Singleflight 与缓存

- 同一 workerd 进程、同一 loader key 的 cold assembly 只有一个 promise；
- follower 等待同一结果；失败后 entry 从 singleflight map 删除，允许后续 bounded retry；
- 不实现跨进程 distributed lock；workerd restart 后自然 cold load；
- callback count、warm/cold 和 assembly duration 可做 bounded metrics；
- 不依赖 `LOADER.load()` 的非缓存行为实现正式 dispatch；正式路径使用 `get()`；
- P0.2 不假设可以显式 evict loaded Worker。

### 11.4 Loader errors

loader host 只向 platformd 返回 stable internal code：

```text
DEPLOYMENT_NOT_READY
DEPLOYMENT_INVARIANT_VIOLATION
ARTIFACT_UNAVAILABLE
ARTIFACT_INTEGRITY_ERROR
BUNDLE_RUNTIME_INVALID
ENTRYPOINT_NOT_FOUND
RESOURCE_LIMIT_EXCEEDED
RUNTIME_INTERNAL
```

workerd/V8 raw message、module source line、internal URL、SQLite/S3 error 不进入 tenant response。
operator diagnostics 可保存 redacted cause chain 和 digest/request ID。

## 12. WorkerCode capability surface

### 12.1 Env

P0.2 tenant `env` 只允许：

```text
declared vars
declared secrets
TEST_FAKE (仅 test profile，生产 build/config 不存在)
```

不默认暴露 account/worker/deployment/internal request identity。未来若产品需要版本信息，应提供明确的
只读 binding，而不是泄露 internal dispatch envelope。

P0.2 conformance test 必须枚举 `Object.keys(env)` 和 prototype/callable surface，确认无通用 Fetcher、
RuntimeSource、loader、disk、process、S3 或 control capability。

### 12.2 Vars

- name 使用 Workers-compatible identifier allowlist，并拒绝平台保留 prefix；
- value 是 JSON-compatible scalar/object/array，canonical JSON 后计入 size；
- 进入 WorkerCode 时转为 structured-clone-compatible value；
- 不接受 function、BigInt、cycle、prototype、自定义 class 或 arbitrary binary object；
- 整个 env 有总 size limit；
- ready 后不可修改，变化创建新 deployment。

### 12.3 Secrets

- P0.2 secret value 是 UTF-8 string；
- API request 做 per-secret/total size limit；
- 入库前调用 P0.1 `SecretCrypto.encrypt(context, plaintext)`；
- associated data 绑定 account/worker/deployment/name/revision；
- RuntimeSource 校验 secret `revision_id`、AEAD context 和 ciphertext 后 just-in-time decrypt；
- platform-owned loader/runtime-source log 只记录 secret count/name hash，不记录值、ciphertext 或
  nonce；
- tenant Worker 得到 secret 明文是预期能力，平台不能阻止 tenant code主动外传或输出自己的
  secret；tenant application log 必须与 platform operator log 分流，P0.2 默认不采集/回显 raw tenant
  console output；
- secret 不可由 GET API 读回；只能 replace/delete 并产生新 deployment。

### 12.4 Test fake adapter

fake adapter 只存在于 integration-test runtime config，用于保留 G0 的 binding-scoped proof：

- capability 通过 `ctx.exports`/stub props 固定 account + resource ID；
- tenant 参数不能选择 scope；
- 有冷/暖、structured clone、safe error 和 fault point tests；
- production release config/hash 中不能包含该 service；
- P0.3 用正式 BindingFactory 替换测试 adapter，不让 fake 变成公共 API。

## 13. Compatibility policy

### 13.1 Host 与 tenant flags 分离

static loader/ingress host 使用 release lock 的 compatibility date/flags；tenant deployment 使用自己
持久化的 date/flags。两者不能合并成一个 global config。

P0.2：

- 精确保存客户端请求的 compatibility date；
- flags 去重并 canonical sort 后存储；
- 只允许当前 platform policy allowlist；
- 拒绝 host-only/experimental process capability；
- 不因为当前 workerd “某 flag 已默认开启”就重写旧 deployment metadata；
- runtime upgrade 后同一 deployment descriptor 保持不变；
- date 超出当前 pinned runtime 支持范围时 deployment validation 失败而非静默降级。

Node compatibility 由 date/flags 和 pinned workerd 决定。P0.2 不自己实现 Node polyfill，也不自动给
所有 deployment 注入 `nodejs_compat`。

### 13.2 API 支持清单

P0.2 release 必须生成基于真实 conformance tests 的 API matrix，至少区分：

```text
supported
supported with documented deviation
unsupported / throws deterministic error
```

matrix 覆盖 Fetch/Request/Response/Headers、URL、Streams、Web Crypto、Timers、WebSocket client、
Node compat 和 outbound fetch。不要用“使用 workerd”推导所有 Cloudflare API 已兼容。

## 14. Egress

### 14.1 默认能力

Workers 常用 `fetch()` 需要出站网络，因此 P0.2 从 G0 的 `globalOutbound=null` 进化为两层
capability：

```capnp
(name = "publicInternet", network = (allow = ["public"]))
(name = "outboundGateway", worker = (... PUBLIC_NETWORK -> publicInternet ...))
```

Dynamic Worker 的 `globalOutbound` 必须指向 `ctx.exports.OutboundGateway({props})` 产生的 scoped
stub，而不是直接指向 network service。Gateway 只实现 `fetch()`、不实现 `connect()`，校验 scheme
后把请求转发到自己的 `PUBLIC_NETWORK` binding。底层 workerd Network policy 在 DNS 解析后的
地址层过滤 private/local，因此可以防止只做 hostname 字符串检查所遗漏的 DNS rebinding/
alternate IP representation。

### 14.2 P0.2 policy

- 只支持 `http:`/`https:` fetch；
- private、loopback、link-local、Unix socket、platformd/workerd internal address 全部拒绝；
- raw TCP `connect()` 不属于 P0.2；tenant `globalOutbound` 没有直接 network capability，gateway
  不导出 `connect()`；
- redirect 每一跳重新应用同一 network policy；
- DNS 返回多个地址时只使用 policy 允许的地址；无允许地址按不可达处理；
- 限制 subrequest count、connect/TLS/header/body/time budget；
- outbound response 支持 streaming/backpressure，不默认完整 buffer；
- error 不回显解析出的 internal address；
- platform-owned internal service 必须通过显式 service binding，不经过 tenant public egress。

gateway props 固定 account/deployment/policy version，tenant 不能自行构造其他 scope。如果后续需要
per-account domain allow/deny、credential injection 或审计，在这个 gateway 内增量实现；P0.2 不再
额外引入一个 platformd 通用 HTTP proxy。

### 14.3 Egress Gate

必须用真实 DNS/HTTP fixtures 验证：public IPv4/IPv6 allow，127/8、RFC1918、link-local、IPv6 local、
metadata address、直接 IP、hostname 解析到 private、public redirect 到 private 全部 deny。测试网络
不能依赖公网稳定性，应在隔离 network namespace/可控 DNS 中验证 workerd address policy。

## 15. Public ingress 与 route resolution

### 15.1 Ingress boundary

`platformd` 是唯一 public listener：

1. 解析 Host/path/method/header 并应用 public limits；
2. 删除全部 platform internal header，无论大小写/重复形式；
3. canonicalize route lookup input，不修改 tenant-visible URL 语义；
4. 在一个 DB/cache generation snapshot 中解析 route + active deployment；
5. 生成 CSPRNG request ID；
6. 构建 internal dispatch metadata；
7. 以 streaming/backpressure 转发 body；
8. 过滤 internal response header，再流式返回客户端。

保留 header allow/deny 策略必须覆盖 hop-by-hop headers、伪造 `Forwarded`/`X-Forwarded-*`、内部 auth、
deployment ID、account ID、request ID 和 route generation。只有在 trusted proxy allowlist 内的上游
才允许提供原始 client metadata。

### 15.2 Route lookup cache

SQLite 是 authority；platformd 可维护 bounded cache：

```text
key    canonical host + path lookup bucket
value  route ID, worker ID, deployment ID, entrypoint, route generation
```

每次 route/deployment promotion transaction 递增 `workers.route_generation` 并在 commit 后发布本进程
invalidation。由于单节点只有一个 `platformd`，不需要 Redis pub/sub。为防止 invalidation bug：

- cache entry 有短的最大 age；
- dispatch envelope 带 generation；
- loader host不解析 active，但 RuntimeSource 可验证 deployment 仍 ready/未 tombstoned；
- promotion 后新 lookup 必须看到新 generation；已冻结的 in-flight A 可以继续。

### 15.3 Streaming 与 budgets

- request body 在 platformd -> workerd -> tenant path 保持 stream；
- response 同样保持 stream，支持 backpressure；
- header 有 count/name/value/total bytes limit；
- body size可按 platform profile限制；无上限 streaming 仍受 wall time/egress bytes/quota；
- tenant 在读取 request body 前抛错时，platformd 停止继续上传；
- workerd 在返回 headers 后 crash，不能改写成干净 JSON error；连接被截断并记录
  `RUNTIME_RESULT_UNKNOWN`；
- request 自动 replay 永远关闭。

### 15.4 Client disconnect

沿用 G0 Conditional Go 限制：

- downstream disconnect 后 platformd cancel 自己的 proxy task并关闭 internal body/response stream；
- 不断言 tenant `request.signal.aborted === true`；
- 不等待 handler 结束才释放 client-facing resource；
- isolate 继续受 runtime CPU/memory/subrequest 与 outer wall deadline；
- 不因单请求未 abort 杀掉整个 workerd；
- acceptance test验证 disconnect 不影响后续请求/health，并记录限制，而不是要求 `D-abort` 通过。

## 16. Resource limits

P0.2 定义本平台 limits profile，不复制 Cloudflare 套餐数字：

```text
ingress header bytes/count
request body bytes
response header bytes/count
deployment archive/expanded/module bytes
module count
env vars/secrets count + bytes
startup validation CPU/memory/wall
request CPU/memory/wall
subrequest count
outbound connect/TLS/header/body deadlines
concurrent requests per deployment/account
```

能由 WorkerLoader/entrypoint `ResourceLimits` 强制的，交给 workerd；只能由 proxy/host观察的，在
platformd 强制。wall timeout 是 admission/response boundary，不应被宣传为必然同步停止 isolate；CPU/
memory limit才负责 runtime containment。

limit metadata 是 immutable deployment descriptor 的一部分。operator 修改默认 profile不应悄悄改变
已经 ready 的 deployment；要么 profile version 固定，要么生成新 deployment。

超过 limit 的错误在 response headers 未发送前映射为 `RESOURCE_LIMIT_EXCEEDED`；已经开始 streaming
时关闭 stream并记录具体 operator reason。

## 17. Promotion 与 rollback

### 17.1 Promotion transaction

```sql
BEGIN IMMEDIATE;

SELECT state, worker_id
FROM worker_deployments
WHERE id = :target_deployment_id;

-- application verifies state='ready', same worker, not deleting

UPDATE workers
SET active_deployment_id = :target_deployment_id,
    route_generation = route_generation + 1,
    updated_at_ms = :now
WHERE id = :worker_id
  AND deleted_at_ms IS NULL
  AND (:expected_active_id IS NULL
       OR active_deployment_id = :expected_active_id);

-- require exactly one row, insert audit event
COMMIT;
```

API 支持 optional `expectedActiveDeploymentId` 做 compare-and-swap，避免两个 operator 覆盖。commit
之后再通知 route cache；即使通知前 crash，下次进程从 DB 读取新 active。

### 17.2 Linearization point

promotion 的线性化点是 SQLite commit：

- commit 前进入 route snapshot 的请求执行旧 deployment；
- commit 后新 snapshot 执行新 deployment；
- 已加载 A、B 都可留在 workerLoader cache；
- 不存在“修改 active key 内容”的过渡状态；
- promotion response 丢失时，client 使用 idempotency/readback 判断是否已提交，不能盲目重放。

### 17.3 Rollback

rollback 是 promotion 到一个旧的 ready deployment，使用相同 transaction和 Gate。旧 deployment
若已 tombstoned、artifact 不可读或不再通过当前 runtime compatibility policy，不可 rollback。

在 P0.2 尚未接 DO 时，rollback 只涉及 HTTP Worker。P0.7 会在相同 promotion event 上追加 DO facet
restart policy，但不能改变 active pointer 的线性化语义。

## 18. Retention、delete 与 GC

### 18.1 Referrer

deployment 可删除前必须没有：

- `workers.active_deployment_id`；
- 正在执行的 validation/dispatch pin；
- P0.2 的 control idempotency response 所需 live reference；
- 后续 P0.7/P2 引入的 DO alarm、Queue consumer、Workflow instance 等 referrer。

不要用 `COUNT(*)` 遗漏未来表。实现一个集中 `DeploymentReferrer` registry/query，P0.3 之后每个新
产品显式注册 referrer 和 deletion test。

### 18.2 Delete deployment

1. `BEGIN IMMEDIATE` 验证 target ready/rejected且非 active；
2. 标记 `deleting`，禁止新 dispatch pin；
3. 等待当前 in-flight pin 到 bounded deadline；
4. 超时可保持 deleting 后异步重试，不能强删 metadata；
5. 标记 tombstoned、删除/保留 secret ciphertext 按 retention policy；
6. artifact refcount 到零且超过 grace period后删除 S3/cache；
7. 写 audit event。

workerd 没有被本方案依赖的逐 key强制 eviction 协议。tombstoned deployment 因 route/source fence
不可再达，但旧 isolate 的内存不承诺立即清除；高安全场景可由 operator 执行受控 workerd restart。

### 18.3 Delete Worker

- 先 transaction disabled/tombstone routes，active pointer 清空，Worker 标记 deleted；
- 新请求立即 route miss；已冻结请求可到 bounded deadline结束；
- deployment 按 retention/GC 异步清理；
- worker ID/name tombstone 和 DO storage ID 不复用；
- P0.7 起 DO storage 默认不随 Worker delete立即物理删除，需显式 destructive operation；
- DELETE API 是异步 operation 时返回 operation ID/status，不谎报所有 S3/内存 bytes 已清除。

### 18.4 Automatic retention

保留最近 ready/rejected 版本数、最小 age、artifact orphan grace 和 audit retention 都由 operator
config 控制。GC 只选择：非 active、无 referrer、超过 retention、未被 pin 的 deployment；每批有
上限并可重入。P0.2 不硬编码 Cloudflare retention 数字。

## 19. 错误协议

Control API 统一 envelope：

```json
{
  "error": {
    "code": "BUNDLE_INVALID",
    "message": "The deployment bundle is invalid.",
    "requestId": "req_...",
    "details": {"field": "mainModule", "reason": "not_found"}
  }
}
```

P0.2 最小 code：

```text
ACCOUNT_NOT_FOUND
WORKER_NOT_FOUND
WORKER_NAME_CONFLICT
WORKER_DELETED
DEPLOYMENT_NOT_FOUND
DEPLOYMENT_NOT_READY
DEPLOYMENT_ACTIVE
DEPLOYMENT_REFERENCED
DEPLOYMENT_INVARIANT_VIOLATION
BUNDLE_INVALID
BUNDLE_TOO_LARGE
BUNDLE_RUNTIME_INVALID
COMPATIBILITY_UNSUPPORTED
ARTIFACT_UNAVAILABLE
ARTIFACT_INTEGRITY_ERROR
ROUTE_NOT_FOUND
ROUTE_CONFLICT
ENTRYPOINT_NOT_FOUND
SECRET_INVALID
IDEMPOTENCY_CONFLICT
RESOURCE_LIMIT_EXCEEDED
RUNTIME_UNAVAILABLE
RUNTIME_RESULT_UNKNOWN
INTERNAL
```

tenant HTTP handler自己返回的 status/body 原样作为应用响应，不包装成 control error。只有 dispatch
前平台错误、runtime containment 或 transport failure 使用平台错误映射。5xx message 不能包含
loader key、S3 key、module source、secret、SQLite path 或 raw upstream exception。

## 20. 可观测性

### 20.1 Structured logs

每个 stage 使用共同字段：

```text
timestamp
level
component
event
request_id
account_id_hash
worker_id_hash
deployment_id_hash
route_id_hash
workerd_generation
loader_cache = cold|warm|unknown
duration_ms
result_code
```

ID 默认 hash/截断用于 operator correlation，完整 ID只在经过鉴权的 control audit 中出现。
platform-owned 日志禁止 body、module source、env values、secret/ciphertext、authorization/cookie、
signed URL、internal token。tenant 主动写出的 application log 是另一条不受信任数据流；P0.2 默认
丢弃，未来开放时必须单独授权、限额并明确其可能包含 tenant 自己输出的 secret。

### 20.2 Metrics

```text
worker_control_request_total{operation,result}
worker_deployment_total{state}
worker_deployment_validation_duration_seconds{result}
worker_promotion_total{result}
worker_dispatch_total{result,entrypoint_kind}
worker_dispatch_duration_seconds{result}
worker_loader_total{result,cache}
worker_loader_assembly_duration_seconds{result}
worker_inflight_requests
worker_request_bytes_total{direction}
worker_egress_total{result,scheme}
worker_limit_exceeded_total{limit_kind}
worker_client_disconnect_total
```

worker/deployment/account/host/request ID 不做 metrics label。route host、URL 和 error message 也不做
label。

### 20.3 Tracing

可选 trace span：ingress、route lookup、runtime source、artifact cache/open、loader get、entrypoint
fetch、egress。trace sampling前做 header/body redaction；不能把 tenant-provided trace ID直接当可信
internal trace identity。

## 21. 实现工作包

### P0.2.0：Schema 与 control model

- Worker/deployment/vars/secrets/routes/idempotency/audit migrations；
- typed repository、state transition 和 invariant checker；
- UUIDv7/名称/route canonicalization；
- create/list/get/delete skeleton；
- migration rollback/future schema tests。

### P0.2.1：WorkerBundleV1

- streaming input parser；
- module/type/path/size validation；
- canonical manifest/framing writer/reader；
- digest/offset/integrity checks；
- ArtifactStore integration；
- malicious archive/fuzz corpus。

### P0.2.2：Deployment pipeline 与 secret snapshot

- idempotent staging/version allocation；
- vars canonicalization；
- AEAD secret rows；
- immutable descriptor/`worker_code_sha256`；
- rejected/orphan recovery；
- create/list/get API。

### P0.2.3：RuntimeSource bridge

- workerd `ExternalServer` binding + spawn-time address；
- per-generation internal auth；
- ready/validation resolve；
- verified artifact/module streaming；
- just-in-time secret decrypt；
- budgets/redaction/snapshot invariant。

### P0.2.4：Loader host

- key grammar；
- pre-`get()` descriptor verification；
- per-key singleflight；
- WorkerCode assembly；
- default/named entrypoint；
- stable error mapping；
- cold/warm/restart/concurrency tests。

### P0.2.5：Runtime validation

- isolated validation key/wrapper；
- parse/link/top-level/Wasm validation；
- no-capability validation env；
- CPU/memory/wall limits；
- state transition/retry/rejection diagnostics。

### P0.2.6：Public ingress 与 routing

- default platform path + exact host route；
- SQLite route snapshot/cache generation；
- internal identity stripping/creation；
- streaming request/response/backpressure；
- disconnect/result-unknown behavior；
- trusted proxy policy。

### P0.2.7：Public egress

- workerd `Network(allow=["public"])`；
- 只导出 fetch 的 scoped OutboundGateway；
- Dynamic Worker `globalOutbound` 指向 gateway stub；
- HTTP(S)-only/subrequest budgets；
- DNS/private/local/redirect SSRF matrix；
- outbound stream/error mapping。

### P0.2.8：Promotion、rollback、retention、delete

- CAS promotion transaction；
- route cache invalidation/readback；
- rollback；
- in-flight pins/referrer registry；
- deployment/Worker tombstone；
- artifact ref/GC/restart recovery。

### P0.2.9：Conformance 与 integration Gate

- supported/deviation/unsupported API matrix；
- G0 Loader/Binding regression migration；
- real packaged workerd、real SQLite、S3 test provider；
- three fresh-process rounds；
- crash/invalid/security/stream matrix；
- no leaked process/port/file/secret。

严格依赖顺序：

```text
P0.2.0 -> P0.2.1 -> P0.2.2 -> P0.2.3 -> P0.2.4 -> P0.2.5
                                                    │
                                                    ├-> P0.2.6 -> P0.2.7
                                                    └-> P0.2.8
all ---------------------------------------------------> P0.2.9
```

P0.2.6、P0.2.7 和 P0.2.8 在 loader/runtime validation 契约稳定后可以并行，但不能在 P0.2.9 前
分别宣布 P0.2 完成。

## 22. 测试矩阵

### 22.1 继承 G0 Loader cases

正式 regression suite 必须迁入：

- L01 cold load、L02 warm load；
- default/named/unknown entrypoint；
- request body、response streaming；
- identity forgery、host request ID；
- unknown/unimplemented kind；
- A/B coexist、promote、rollback；
- active route 忽略 body deployment；
- invalid bundle；
- outbound denied baseline；
- immutable key reuse invariant；
- sanitized error/log；
- workerd restart cold load；
- concurrent cold load singleflight；
- fake adapter scope/capability/structured-clone/safe-error cases。

`D-abort` 继续作为已知限制记录，不计入 hard fail；替代 hard assertion 是断连后服务健康、无自动
replay、资源 limit 仍能结束恶意 handler。

### 22.2 Control/schema

| 场景 | 断言 |
| --- | --- |
| create 同名并发 | 一个成功，其余 stable conflict；无 duplicate route/DO ID |
| deploy 同 idempotency key | 相同 request 返回相同 deployment；不同 digest conflict |
| concurrent deploy | version 唯一单调，允许 gap，不复用 rejected version |
| process crash at each pipeline stage | active 不变；row/artifact 状态可恢复/GC |
| future/checksum-bad migration | workerd 不启动，authority 不被修改 |
| ready row mutation attempt | DB/repository 拒绝；descriptor invariant保持 |
| delete/recreate same name | worker ID、loader key、DO storage ID 均不同 |

### 22.3 Bundle/runtime validation

- empty/missing/duplicate main；
- `..`、absolute、backslash、NUL、Unicode alternate、case collision；
- symlink/device/archive bomb/overlapping offset/trailing bytes；
- per-module/total/count/manifest/env limit；
- digest/size mismatch、S3 corruption、cache corruption；
- JS parse error、missing import、cycle、top-level throw/hang；
- valid/invalid Wasm；
- unsupported module type/compatibility flag/date；
- validation code不能访问 network/RuntimeSource/control/fake binding；
- validation failure不改变 active，raw source/secret不进 error/log。

### 22.4 Loader/route/stream

- DB-backed route 在 platformd/workerd restart 后恢复；
- promotion transaction 前/后 route snapshot 分别冻结 A/B；
- cache invalidation丢失时最大 age/readback恢复；
- forged account/worker/deployment/request/internal-auth headers 均无效；
- loader key malformed/cross-account/cross-worker；
- warm path也执行 pre-`get()` descriptor check；
- 100+ concurrent cold requests只 assembly一次；
- request/response大流 backpressure、slow client、early response、midstream crash；
- response headers 前/后 runtime crash错误行为不同且明确；
- client disconnect后后续 request/health成功，无 automatic replay。

### 22.5 Vars/secrets/capability

- JSON scalar/nested/Unicode/size boundary；
- env name conflict/reserved name/prototype pollution input；
- DB page/search、S3 artifact、cache、argv、platform log、metrics、platform error 中无 secret
  plaintext；tenant console 默认不采集；
- wrong master key/associated data/ciphertext tamper fail closed；
- secret update创建新 deployment，旧 version值不变；
- `Object.keys(env)` 只出现声明项；
- production config中不存在 TEST_FAKE；test fake不能跨 scope。

### 22.6 Egress/security

- public HTTP/HTTPS/IPv4/IPv6 allow；
- private/loopback/link-local/metadata/Unix/direct-IP deny；
- public hostname -> private DNS deny；
- public URL -> private redirect deny；
- URL alternate encoding、userinfo、IPv4 integer/IPv6 mapped address；
- subrequest/connect/TLS/header/body timeout与count limit；
- tenant不能访问 RuntimeSource port/token/control API；
- error/log不泄露 resolved private address/internal topology。

### 22.7 Lifecycle

- A deploy -> promote A -> B deploy -> promote B -> rollback A；
- invalid C在每个 validation stage失败，active仍为当前版本；
- promotion CAS conflict/readback/idempotency；
- active deployment delete拒绝；
- in-flight/referrer deployment delete等待/重试；
- tombstoned route不可达，warm isolate不可通过旧 key重新dispatch；
- shared artifact只在最后 ref + grace后GC；
- crash during deleting/GC可重入；
- Worker delete和同名重建完全隔离。

## 23. P0.2 Exit Gate

- [ ] 无 binding module Worker可 create/deploy/validate/route/fetch；
- [ ] A -> B -> rollback A通过，并有 promotion linearization并发测试；
- [ ] cold、warm、concurrent cold和workerd restart cold load通过；
- [ ] `worker_code_sha256` 在 warm `get()` 前验证，同 key内容变化 fail closed；
- [ ] invalid bundle/runtime deployment不会改变 active；
- [ ] default/named/unknown entrypoint行为稳定；
- [ ] request/response streaming/backpressure与midstream failure有明确语义；
- [ ] vars/secrets不可变，secret未出现在平台持久层、argv、platform log/metrics/error；tenant
  console 默认不采集；
- [ ] tenant env/capability枚举无 RuntimeSource/S3/SQLite/internal Fetcher；
- [ ] public egress允许公网、拒绝 private/local/metadata和redirect绕过；
- [ ] disconnect限制按G0结果记录，不用错误的 abort保证做资源回收；
- [ ] retention/delete/referrer/artifact GC可 crash recovery；
- [ ] Linux/macOS 三轮 fresh-process suite 无 leaked child/port/file/secret；
- [ ] API compatibility matrix由真实 tests生成，unsupported项有确定错误。

## 24. 向 P0.3 提供的稳定接口

P0.3 Resource/Binding Framework 可以依赖：

```text
DeploymentSnapshot
  account_id / worker_id / deployment_id
  immutable descriptor + compatibility + limits

BindingDescriptor slot
  unique env name
  type + physical resource ID + immutable config
  participates in worker_code_sha256

WorkerCodeAssembler
  modules + env + binding stubs + outbound + limits

RuntimeSource
  scoped immutable snapshot/artifact stream

DispatchContext
  request ID + frozen deployment + entrypoint + limits

DeploymentReferrer
  register/query/fence live product references
```

P0.3 不得：修改 ready deployment binding、把 display name当 physical ID、把通用 platform Fetcher
交给 tenant、绕过 RuntimeSource 读取 SQLite/S3，或把真实产品 binding 塞进 P0.2 test fake adapter。

## 25. 参考资料

- [G0 results](./g0-results.md)
- [G0 POC README](../poc/README.md)
- [G0 loader host](../poc/workerd/loader-host.js)
- [G0 code assembly](../poc/workerd/code.js)
- [G0 registry](../poc/workerd/registry.js)
- [总体方案](./sqlite-workerd-platform.md)
- [P0.1 Platform Foundation](./p0-1-platform-foundation.md)
- [Cloudflare Dynamic Workers API](https://developers.cloudflare.com/dynamic-workers/api-reference/)
- [Cloudflare Dynamic Worker bindings](https://developers.cloudflare.com/dynamic-workers/usage/bindings/)
- [Cloudflare Dynamic Worker egress control](https://developers.cloudflare.com/dynamic-workers/usage/egress-control/)
- [Cloudflare Workers compatibility dates](https://developers.cloudflare.com/workers/configuration/compatibility-dates/)
- [Cloudflare Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/)
- [workerd configuration schema](https://github.com/cloudflare/workerd/blob/main/src/workerd/server/workerd.capnp)
