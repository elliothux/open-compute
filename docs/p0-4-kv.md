# P0.4：KV 详细设计

> 状态：Implemented and verified（2026-08-25）
>
> 前置依赖：[P0.3：Resource 与 Binding Framework](./p0-3-resource-binding-framework.md)
>
> 平台基线：[P0.1：Platform Foundation](./p0-1-platform-foundation.md)、
> [P0.2：Workers Runtime](./p0-2-workers-runtime.md)
>
> 兼容目标：还原 Cloudflare Workers KV 最常用的 Worker binding API；不还原 edge cache、全球复制、
> REST bulk write/delete 或完整 Cloudflare 运维语义。

P0.4 是 P0.3 framework 的第一个真实产品。它使用“一 KV namespace 一 SQLite database”，以
P0.3 的 resource lifecycle 管理文件，以 custom JSRPC binding 向 Dynamic Worker 暴露
`KVNamespace` 常用方法。单节点读写是强一致的；`cacheTtl` 仅做参数兼容，不建立 edge cache。

当前实现证据：

- `crates/storage/src/kv/`：namespace catalog、typed paths、SQLite engine、WAL/backup/restore；
- `crates/workers/src/kv.rs`：P0.3 resource lifecycle driver；
- `crates/service/src/kv_backend.rs`、`kv_http.rs`、`binding_backend.rs`：control plane、真实执行器与
  bounded private frame/stream protocol；
- `runtime/system-workers/loader-host.js`：tenant `KVNamespace` adapter；
- `crates/service/tests/p0_4_kv_gate.rs`、`scripts/test-p0-4`：stock-workerd Gate 和 P0.3/P0.2 回归；
- fresh `./scripts/coverage` 已通过 90.00% 门槛，实际 Rust line coverage 为 90.06%。

`binding_backend.rs` 保留完整 private protocol authentication/frame/stream/error matrix，`kv_http.rs`
保留 backup/restore 的 durable orchestration，`engine.rs` 保留 namespace schema/transaction/blob invariant；
这三个 cohesive authority/protocol source 因此超过 800 行。对应测试已拆到独立 `*_tests.rs`，没有把
production logic 放进 coverage exclusion；`binding_backend_tests.rs` 作为共享同一 authenticated
protocol fixture 的完整 legacy/P0.4 failure matrix 也保持在一个 test module 中。

## 1. 交付目标

```text
Control API
    └── KvNamespaceController
            ├── resources(kind=kv_namespace)
            └── kv_namespaces / control.sqlite
                    └── KvResourceDriver
                            └── data/kv/<account>/<namespace>/data.sqlite

Worker env.CACHE
    └── ctx.exports.KVNamespace({ props })
            └── JSRPC adapter
                    └── private BindingBackend /internal/bindings/v1/kv/...
                            └── binding authorization + ResourcePin
                                    └── KvEngine
                                            └── bounded SQLite handle manager
```

完成后，用户可以：

- 创建、list、rename、删除多个 KV namespace；
- 在 Worker deployment 中以 resource ID 绑定多个 namespace；
- 使用 `get`、`getWithMetadata`、`put`、`delete`、array get 和 `list`；
- 使用 text/json/arrayBuffer/stream value 类型；
- 使用 TTL、metadata、prefix 和 opaque cursor；
- 在 restart、WAL recovery、并发写和单 namespace 损坏时保持确定行为；
- 把 namespace 在线备份到已配置 S3，并从备份创建一个新 namespace。

### 1.1 完成定义

- 一个 namespace 的 entry 只存在自己的 SQLite database，不进入 `control.sqlite`；
- namespace ID、binding ID 与 deployment ID 全程使用 P0.3 typed identity；
- key 顺序严格按 UTF-8 bytes，且不做 Unicode normalization；
- value 上限 25 MiB，stream 不经 JSON/base64 全量膨胀；
- `put` 是单 SQLite transaction 的原子替换，失败时旧值仍可见；
- 过期 row 在所有读/list 路径上立即视为不存在，后台 GC 只负责回收空间；
- list cursor 与 namespace/prefix 绑定并带 HMAC，不能伪造跨 namespace 翻页；
- SQLite 阻塞 I/O 不运行在 async runtime core thread；
- connection/stream/temp-file 数量和 bytes 都有硬上限；
- 单文件损坏只隔离一个 namespace，不拖垮平台 readiness；
- 真实 stock workerd Gate 连续三轮 fresh process 通过，P0.2/P0.3 无回归。

### 1.2 非目标

- Cloudflare 全球复制、eventual consistency、colo cache 或 edge cache purge；
- Cloudflare REST namespace API 的完整 path/pagination/authorization 兼容；
- REST bulk put、bulk delete；Workers binding 本身也不提供这两个 bulk method；
- cache tags、transactions、compare-and-swap、watch 或 secondary index；
- 跨 key transaction API；每次 `put`/`delete` 是独立 mutation；
- 多节点 shared SQLite、NFS/SMB 文件系统上的并发 owner；
- namespace in-place restore、point-in-time recovery 或跨节点 replication；
- 把 SQLite database 放进 S3 后直接运行；S3 只保存 backup artifact；
- 模拟 Cloudflare quota/billing/account limits 的全部细节。

## 2. Cloudflare 常用 API 兼容面

以当前官方 Workers KV binding 文档为兼容基线：

- [Read key-value pairs](https://developers.cloudflare.com/kv/api/read-key-value-pairs/)
- [Write key-value pairs](https://developers.cloudflare.com/kv/api/write-key-value-pairs/)
- [Delete key-value pairs](https://developers.cloudflare.com/kv/api/delete-key-value-pairs/)
- [List keys](https://developers.cloudflare.com/kv/api/list-keys/)
- [How KV works](https://developers.cloudflare.com/kv/concepts/how-kv-works/)

### 2.1 Worker binding surface

```ts
type KVGetOptions = {
  type?: "text" | "json" | "arrayBuffer" | "stream"
  cacheTtl?: number
}

interface KVNamespace {
  get(
    key: string,
    typeOrOptions?: "text" | "json" | "arrayBuffer" | "stream" | KVGetOptions
  ): Promise<string | unknown | ArrayBuffer | ReadableStream | null>

  get(
    keys: string[],
    typeOrOptions?: "text" | "json" | KVGetOptions
  ): Promise<Map<string, string | unknown | null>>

  getWithMetadata(
    key: string,
    typeOrOptions?: "text" | "json" | "arrayBuffer" | "stream" | KVGetOptions
  ): Promise<{ value: unknown | null; metadata: unknown | null }>

  getWithMetadata(
    keys: string[],
    typeOrOptions?: "text" | "json" | KVGetOptions
  ): Promise<Map<string, { value: unknown | null; metadata: unknown | null }>>

  put(
    key: string,
    value: string | ArrayBuffer | ArrayBufferView | ReadableStream,
    options?: KVPutOptions
  ): Promise<void>

  delete(key: string): Promise<void>

  list(options?: {
    prefix?: string
    limit?: number
    cursor?: string
  }): Promise<{
    keys: Array<{ name: string; expiration?: number; metadata?: unknown }>
    list_complete: boolean
    cursor?: string
  }>
}
```

实现使用 P0.3 `WorkerEntrypoint` custom binding，因此目标是 method/value/error 的常用兼容，不
承诺 native `workerd` KV binding 的 prototype、`instanceof`、property enumeration 或非文档
行为完全一致。

### 2.2 兼容矩阵

| 能力 | P0.4 | 说明 |
| --- | --- | --- |
| single `get` | 支持 | 默认 `text` |
| array `get` | 支持 | 最多 100 keys，返回 `Map` |
| `getWithMetadata` single/array | 支持 | missing value/metadata 为 `null` |
| `text` | 支持 | UTF-8 decode |
| `json` | 支持 | decode 后 `JSON.parse` |
| `arrayBuffer` | single 支持 | array get 不支持，与官方常用面一致 |
| `stream` | single 支持 | byte stream，cancel 传播到 SQLite/backend |
| `put` string/buffer/view/stream | 支持 | 最大 25 MiB |
| `delete` | 支持 | missing key 仍成功 |
| `list` prefix/limit/cursor | 支持 | UTF-8 bytes order，默认/最大 1000 |
| expiration/TTL | 支持 | 最短 60 秒 |
| metadata | 支持 | canonical JSON 最大 1024 bytes |
| `cacheTtl` | 接受并校验 | 最短 30 秒；单节点无 edge cache，因此忽略 |
| binding bulk write/delete | 不支持 | 官方 Worker binding 也不提供 |
| REST bulk API | 不支持 | 不属于 P0.4 runtime binding |
| global eventual consistency | 不模拟 | 单进程 SQLite commit 后强一致 |

### 2.3 固定 limits

P0.4 V1 使用与官方当前常用约束对齐的常量：

```rust
const KV_MAX_KEY_BYTES: usize = 512;
const KV_MAX_VALUE_BYTES: usize = 25 * 1024 * 1024;
const KV_MAX_METADATA_BYTES: usize = 1024;
const KV_MAX_MULTI_GET_KEYS: usize = 100;
const KV_MAX_MULTI_GET_RESPONSE_BYTES: usize = 25 * 1024 * 1024;
const KV_DEFAULT_LIST_LIMIT: u16 = 1000;
const KV_MAX_LIST_LIMIT: u16 = 1000;
const KV_MIN_EXPIRATION_TTL_SECONDS: u64 = 60;
const KV_MIN_CACHE_TTL_SECONDS: u64 = 30;
```

这些值属于 `KV_CAPABILITY_VERSION = 1`。未来改变 limit 必须新增明确 version或保持向后兼容，
不能静默让 retained deployment 的行为漂移。

## 3. 与 Cloudflare 有意不同的语义

| Cloudflare KV | 本方案 |
| --- | --- |
| 全球分布、cache 后 eventual consistency | 单节点 SQLite，commit 后新请求强一致 |
| `cacheTtl` 控制 edge cache | 参数兼容但无效果 |
| provider 管理 namespace 数据 | 本地一 namespace 一 SQLite |
| REST 与 binding 共用 Cloudflare service | P0.4 只保证平台 control API + Worker binding |
| 跨 colo list/read 可能看到传播延迟 | 同一数据库 snapshot；分页间仍不提供 snapshot isolation |

还需明确以下 deterministic deviation：

- 同时提供 `expiration` 与 `expirationTtl` 返回 `KV_INVALID_OPTIONS`，不猜优先级；
- lone UTF-16 surrogate 在 adapter 边界返回 `TypeError`，不静默替换成 U+FFFD；
- cursor 有 15 分钟有效期；过期或参数不匹配返回 `KV_CURSOR_INVALID`；
- 不自动 retry mutation，避免 response loss 后重复改变 relative TTL；
- namespace backup/restore 是本平台扩展，不出现在 tenant binding。

## 4. Control plane schema

新增 `004_kv.sql`。

### 4.1 `kv_namespaces`

```sql
CREATE TABLE kv_namespaces (
  resource_id          TEXT PRIMARY KEY REFERENCES resources(id),
  storage_key          TEXT NOT NULL UNIQUE,
  schema_version       INTEGER NOT NULL CHECK(schema_version >= 1),
  quota_bytes          INTEGER NOT NULL CHECK(quota_bytes >= 268435456),
  created_at_ms        INTEGER NOT NULL,
  last_opened_at_ms    INTEGER,
  last_quick_check_ms  INTEGER,
  last_backup_at_ms    INTEGER,
  restore_backup_id    TEXT REFERENCES kv_backups(id)
) STRICT;
```

`storage_key` 是 host 生成的 canonical relative key：

```text
v1/<account-id>/<resource-id>/data.sqlite
```

它不是任意 path，storage layer 解析后必须重新核对 account/resource ID segment。API 永远不返回
`storage_key`。`quota_bytes` 从 operator 配置在 namespace 创建时冻结；V1 默认 1 GiB，最小
256 MiB，operator 可以提高。实现通过 SQLite `max_page_count` 和全局 disk safety floor 共同
约束，不承诺等于 value bytes 之和。

### 4.2 `kv_backups`

```sql
CREATE TABLE kv_backups (
  id                    TEXT PRIMARY KEY CHECK(length(id) = 36 AND id = lower(id)),
  source_resource_id    TEXT NOT NULL REFERENCES resources(id),
  state                 TEXT NOT NULL CHECK(state IN (
                          'creating', 'ready', 'failed', 'deleting', 'tombstoned'
                        )),
  object_key            TEXT,
  sha256                BLOB CHECK(sha256 IS NULL OR length(sha256) = 32),
  size_bytes            INTEGER CHECK(size_bytes IS NULL OR size_bytes >= 0),
  kv_schema_version     INTEGER NOT NULL CHECK(kv_schema_version >= 1),
  created_at_ms         INTEGER NOT NULL,
  completed_at_ms       INTEGER,
  error_code            TEXT,
  idempotency_key       TEXT NOT NULL CHECK(length(idempotency_key) BETWEEN 1 AND 128),
  request_fingerprint   BLOB NOT NULL CHECK(length(request_fingerprint) = 32),
  UNIQUE(source_resource_id, idempotency_key),
  CHECK((state = 'ready') =
        (object_key IS NOT NULL AND sha256 IS NOT NULL AND size_bytes IS NOT NULL))
) STRICT;

CREATE INDEX kv_backups_source
ON kv_backups(source_resource_id, created_at_ms, id);
```

Backup row 引用 tombstoned resource 是允许的，因为 `resources` 保留 tombstone。Backup 不阻止
namespace 删除；它有独立 retention/delete policy。

### 4.3 Cross-table invariants

Trigger/controller 必须保证：

- `kv_namespaces.resource_id` 对应 `resources.kind='kv_namespace'`；
- `resources.state='ready'` 前必须已有 product row 和可验证 physical database；
- product row、storage key、quota、schema version 在 ready 后不可修改；
- resource tombstone 后 product live locator 不再能被 open；需要保留的审计信息进入 tombstone/
  backup row，而不是继续使用 live row；
- backup object key 必须位于系统 prefix，不能接受 caller input；
- restore 创建新的 resource ID/product row，不复用 source identity。

## 5. 物理目录与安全打开

### 5.1 Layout

```text
data/kv/
├── .staging/
│   └── <resource-id>.<nonce>/
│       └── data.sqlite
├── .staging-write/
│   └── <resource-id>/<request-id>
├── .trash/
│   └── <resource-id>.<delete-token>/
│       └── data.sqlite
└── <account-id>/
    └── <resource-id>/
        ├── data.sqlite
        ├── data.sqlite-wal   # 仅打开时可能存在
        └── data.sqlite-shm   # 仅打开时可能存在
```

“一 namespace 一 SQLite”指每个 resource 只有一个 SQLite database；外层独立目录用于把
database、WAL/SHM 和 delete quarantine 作为一个 resource unit 管理。

### 5.2 P0.1 `DataDir` 扩展

当前 P0.1 有意没有创建未来的 `kv/`。P0.4 第一次启用 KV 时由 typed `KvPaths` 创建：

- `kv`、`.staging`、`.trash`、account/resource directory 全部 owner-only；
- 从 canonical typed IDs 构造 segment，不拼接 user name；
- 使用 no-follow/open-relative 语义拒绝 symlink 和非目录中间节点；
- staging/live/trash 必须位于同一 data filesystem，才能 atomic rename；
- 启动 doctor 拒绝 group/world writable、wrong owner、symlink、hardlink count 异常；
- 不扫描或删除无法证明属于本平台 resource ID 的陌生文件。

### 5.3 Create file sequence

```text
control tx A: resources=creating + kv_namespaces(storage_key)
        ↓
mkdir .staging/<id>.<nonce> with exclusive ownership
        ↓
create data.sqlite with journal_mode=DELETE
        ↓
migration + kv_meta identity + quick_check
        ↓
close + fsync database + fsync staging directory
        ↓
atomic rename staging directory -> live resource directory
        ↓
fsync account directory
        ↓
control tx B: resource=ready
```

Create 阶段不用 WAL，避免 rename 时遗留 sidecar。第一次 runtime open 再切换 WAL。每一步按 P0.3
resource ID 幂等 reconcile；发现 live directory 时必须打开并验证内部 identity，不能仅凭存在就
标 ready。

### 5.4 Delete file sequence

```text
resource referrers == 0
        ↓
ResourcePins fence + drain
        ↓
checkpoint(TRUNCATE) + close all handles
        ↓
resource state=deleting
        ↓
atomic rename live directory -> .trash/<id>.<delete-token>
        ↓
resource=tombstoned + product live locator retired
        ↓
bounded background recursive cleanup of exact trash token
```

不对含未识别文件、symlink 或 identity mismatch 的目录做递归删除；标记
`RESOURCE_INVARIANT_VIOLATION` 等 operator 处理。删除是 recoverable quarantine，不使用 broad
glob/未解析环境变量。

## 6. Namespace SQLite schema

### 6.1 关键设计修正：保留 ROWID

早期总方案的示例使用 `WITHOUT ROWID`。P0.4 改为显式 `INTEGER PRIMARY KEY`：Workers KV 单值
可达 25 MiB，而 SQLite incremental BLOB I/O 不支持 `WITHOUT ROWID` table。保留 rowid 才能用
`zeroblob()` + `sqlite3_blob_open()` 在不把大值复制进单个 SQL parameter 的情况下写入/读取。

参考：[SQLite WITHOUT ROWID limitations](https://www.sqlite.org/withoutrowid.html)。

### 6.2 Schema V1

```sql
CREATE TABLE kv_meta (
  key    TEXT PRIMARY KEY,
  value  BLOB NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE kv_entries (
  id             INTEGER PRIMARY KEY,
  key            BLOB NOT NULL UNIQUE
                   CHECK(length(key) BETWEEN 1 AND 512),
  value          BLOB NOT NULL
                   CHECK(length(value) <= 26214400),
  metadata_json  BLOB
                   CHECK(metadata_json IS NULL OR length(metadata_json) <= 1024),
  expires_at_ms  INTEGER
                   CHECK(expires_at_ms IS NULL OR expires_at_ms > 0),
  updated_at_ms  INTEGER NOT NULL CHECK(updated_at_ms >= 0)
) STRICT;

CREATE INDEX kv_entries_expiration
ON kv_entries(expires_at_ms, id)
WHERE expires_at_ms IS NOT NULL;
```

`kv_meta` 至少保存：

```text
format = open-compute-kv
schema_version = 1
resource_id = <canonical UUIDv7>
account_id = <canonical UUIDv7>
created_at_ms = ...
```

`kv_entries.key` 是 BLOB，`UNIQUE` index 与 `ORDER BY key` 使用 binary `memcmp` 顺序，等价于按
UTF-8 bytes 排序，不受 locale/collation 影响。参考
[SQLite datatype/sort order](https://www.sqlite.org/datatype3.html)。

### 6.3 Connection pragmas

每个 live database 首次打开/每个新 connection 设置并核对：

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA foreign_keys = ON;
PRAGMA trusted_schema = OFF;
PRAGMA busy_timeout = 5000;
PRAGMA wal_autocheckpoint = 1000;
PRAGMA temp_store = MEMORY;
```

还要按 frozen quota 设置 `max_page_count`，按 operator memory budget 设置负值 `cache_size`。不能
允许 tenant SQL，因此没有 D1 那样的 SQL authorizer，但所有语句必须是 host 固定 prepared SQL。

WAL 允许 reader 与 writer 重叠，但 SQLite 仍然只有一个 writer；P0.4 明确串行化每 namespace
mutation，不用 busy retry 制造不可预测延迟。参考 [SQLite WAL](https://www.sqlite.org/wal.html)。

## 7. Key、value 与 metadata canonicalization

### 7.1 Key

Adapter 在 RPC 前完成第一层检查，Rust backend 再检查一次：

- 必须是 JS string；
- 不允许空字符串、`.`、`..`；
- 不允许 lone UTF-16 surrogate；
- 不做 NFC/NFD normalization，不做 trim、case fold、path normalization；
- UTF-8 bytes 长度 `1..=512`；
- NUL、`/`、emoji 和合法组合字符均允许；它们只是 BLOB key，不进入 filesystem path；
- list 返回时严格按 UTF-8 decode 回原 string；DB 出现 invalid UTF-8 视为 invariant corruption。

### 7.2 Value

| Input | 存储 bytes |
| --- | --- |
| string | UTF-8 bytes |
| `ArrayBuffer` | exact bytes |
| `ArrayBufferView` | view 的 offset/length 对应 exact bytes |
| byte `ReadableStream` | 按 chunk 顺序连接的 exact bytes |

超过 25 MiB 在读取第一个超限 byte 时立即 cancel input、删除 staging file 并返回
`KV_VALUE_TOO_LARGE`。空 value 合法。

### 7.3 Metadata

`options.metadata` 必须是 JSON-compatible value：

- circular、BigInt、function、symbol、non-finite number 等在 adapter 返回 `TypeError`；
- host 重新 parse 并 canonical serialize，按最终 UTF-8 bytes 计算 1024-byte 上限；
- object key ordering 不影响语义，但存储 canonical bytes 便于 deterministic backup/test；
- `undefined`/未提供表示没有 metadata；显式 `null` 保存 JSON `null`；
- metadata 不进入日志/metric/cursor；list/getWithMetadata 按 JSON value 返回。

## 8. Read path

### 8.1 Single get

固定 SQL：

```sql
SELECT id, length(value), metadata_json, expires_at_ms
FROM kv_entries
WHERE key = ?1
  AND (expires_at_ms IS NULL OR expires_at_ms > ?2);
```

行为：

1. backend 验证 binding/resource 并取得 `ResourcePin`；
2. 使用 host `Clock` 的 non-decreasing effective wall time；不信任 tenant time；
3. 在 read transaction 中查询 row；expired/missing 都返回 not found；
4. 打开 value blob 并在 snapshot/connection/pin 生命周期内读取；
5. adapter 按 type 转换；
6. body cancel/EOF/timeout 后释放 blob、transaction、connection、pin。

默认 `text`。转换规则：

- `text`：WHATWG UTF-8 replacement decode；
- `json`：先 text decode，再 `JSON.parse`；失败抛稳定 `SyntaxError`，不包含 value；
- `arrayBuffer`：在 25 MiB business limit 内返回 exact bytes；
- `stream`：返回 byte `ReadableStream`，遵守 backpressure；
- missing：single get 返回 `null`。

### 8.2 `getWithMetadata`

与 get 使用同一次 read snapshot。missing 返回：

```js
{ value: null, metadata: null }
```

存在但没有 metadata 返回：

```js
{ value: decodedValue, metadata: null }
```

不能先 get value 再单独查 metadata，否则并发 put 可能混合两个版本。

### 8.3 Array get

- `keys.length <= 100`；空 array 返回空 `Map`；
- 只允许 `text`/`json`；
- 在一个 read transaction 内用固定 prepared statement 逐 key 查找，避免动态 SQL；
- backend 对重复 key 只读取一次；返回 `Map` 按输入第一次出现的顺序，重复 key 自然折叠；
- missing key 仍出现在 Map 中，value 为 `null`；
- 包含 framing、key、metadata、value 的总结果超过 25 MiB 时整体失败，不返回部分 Map；
- 使用 length-prefixed binary `KVBatchFrameV1`，不把 bytes base64 进 JSON；
- JSON parse 任何一项失败时整体 reject，错误不包含 key/value。

## 9. Write path

### 9.1 Put options

```ts
type KVPutOptions = {
  expiration?: number
  expirationTtl?: number
  metadata?: unknown
}
```

- `expiration` 是 Unix seconds；必须是安全整数且至少比 backend effective now 晚 60 秒；
- `expirationTtl` 是 seconds；必须是安全整数且 `>= 60`；
- 两者同时出现返回 `KV_INVALID_OPTIONS`；
- TTL 以 backend 接受请求的时间计算，并用 checked arithmetic 转成 milliseconds；
- 未提供表示永不过期；
- tenant `Date.now()` 不参与计算。

### 9.2 Streaming staging

所有 value 可以共用同一 bounded stream pipeline。小 value 可以在内存阈值内直接进入后半段；
大/未知长度 stream：

```text
JS value/stream
    └── JSRPC byte stream
            └── internal HTTP request body
                    └── secure per-request staging file
                            ├── incremental byte count <= 25 MiB
                            ├── cancellation cleanup
                            └── known final length
```

Staging file：

- 位于 `data/kv/.staging-write/<resource-id>/<request-id>` 或 permission-restricted temp root；
- 由 typed path + exclusive create 生成，不使用 key 作 filename；
- 有 per-request、per-resource、global staging byte/count semaphore；
- write/close 后不要求它自身成为 authority；SQLite commit 才是可见点；
- crash 后启动清理超过 generation age 且不被 manifest/pin 引用的 exact staging files；
- cleanup 不跟随 symlink，不 broad-delete。

### 9.3 Atomic upsert with incremental BLOB

拿到确定长度后，在 namespace 单 writer lane 执行：

```text
BEGIN IMMEDIATE
    ├── SELECT id WHERE key = ?
    ├── existing: UPDATE value=zeroblob(N), metadata, expiration, updated_at
    └── missing:  INSERT value=zeroblob(N) ... RETURNING id
        ↓
sqlite incremental blob open(rowid, value, write=true)
        ↓
copy staging -> blob in bounded chunks
        ↓
close blob
        ↓
COMMIT
```

copy/metadata/expiration/commit 任一步失败都 rollback，reader 继续看到旧 committed value。不能在
commit 前向 adapter 返回 success。

`rusqlite` 需要启用 `blob` feature。Blocking transaction 和 blob copy 在 bounded blocking
executor，不占 Tokio core。25 MiB copy 会持 writer lock，但只影响该 namespace；不同 namespace
使用不同 SQLite file 可并行。

### 9.4 Delete

```sql
BEGIN IMMEDIATE;
DELETE FROM kv_entries WHERE key = ?1;
COMMIT;
```

missing key 仍返回 success。Delete 不自动 retry。发生在 commit 边界的 transport loss 返回
`KV_RESULT_UNKNOWN`；caller 可再次 delete，因为 delete 是幂等的。

### 9.5 Mutation response loss

Adapter/backend 不自动 replay `put`/`delete`：

- commit 前明确失败：返回 retryable/non-retryable stable error；
- commit 后收到成功：返回 `void`；
- 无法判断 commit 是否发生：`KV_RESULT_UNKNOWN`；
- caller 重试 put 通常按 key 覆盖，但 relative TTL 会从新请求时间重算；这是选择，不是透明重试。

## 10. Expiration 与 GC

### 10.1 Read semantics

所有 read、getWithMetadata、array get、list 都包含：

```sql
expires_at_ms IS NULL OR expires_at_ms > :effective_now
```

因此过期在逻辑上立即不可见；GC 延迟不影响 API correctness。对已观察为过期的 row 可以在后续
低优先级 writer batch 删除，不能为了清理而阻塞 foreground read。

### 10.2 Clock contract

- 使用 platform host clock abstraction，测试可注入；
- 单进程内 effective wall time 不回退：`max(last_effective, wall_now)`；
- restart 后依赖正确的 host Unix clock；operator readiness/doctor 应报告严重时钟漂移；
- 一旦过期 row 被物理删除，时钟回退不会让它复活；
- 未来时钟误跳可能提前过期，平台不能安全猜测并恢复，必须记录 operator-visible clock error。

### 10.3 GC worker

每个 hot namespace 最多一个低优先级 GC task：

```sql
DELETE FROM kv_entries
WHERE id IN (
  SELECT id
  FROM kv_entries
  WHERE expires_at_ms IS NOT NULL
    AND expires_at_ms <= ?1
  ORDER BY expires_at_ms, id
  LIMIT 256
);
```

- 每批一个短 `BEGIN IMMEDIATE` transaction；
- writer backlog、disk pressure 或 request load 高时让路；
- checkpoint/GC 都由 bounded scheduler 触发，不为每个 namespace 建永久 thread；
- metric 只记录 count/bytes estimate，不记录 key；
- restart 后无需 replay task，index scan 可重建工作。

## 11. List 与 opaque cursor

### 11.1 Query

把 prefix 编码成 UTF-8 bytes，计算严格 upper bound：从尾部找到第一个 `< 0xff` byte，加一并截断；
不存在 successor 时没有 upper bound。

```sql
SELECT key, metadata_json, expires_at_ms
FROM kv_entries
WHERE key >= ?1
  AND (?2 IS NULL OR key < ?2)
  AND (?3 IS NULL OR key > ?3)
  AND (expires_at_ms IS NULL OR expires_at_ms > ?4)
ORDER BY key
LIMIT ?5; -- requested limit + 1
```

返回前 `limit` 个；若存在第 `limit + 1` 个 live row：

```js
{
  keys: [...],
  list_complete: false,
  cursor: "<opaque>"
}
```

否则 `list_complete: true`，不要求返回 cursor。key object：

```ts
{
  name: string
  expiration?: number // absolute Unix seconds
  metadata?: unknown  // 只有实际存在时
}
```

### 11.2 Cursor V1

Cursor payload 使用 compact binary/CBOR-equivalent canonical encoding，不使用可编辑 JSON：

```text
version = 1
resource_id
resource_spec_generation
prefix_sha256
last_key bytes
issued_at_ms
expires_at_ms
key_version
HMAC-SHA256(all previous fields)
```

HMAC key 从 P0.1 master key 用 domain-separated HKDF 派生：

```text
open-compute/kv-list-cursor/v1
```

验证顺序：base64url/长度 -> version -> HMAC constant-time -> expiry -> namespace/generation ->
当前 prefix hash。错误统一为 `KV_CURSOR_INVALID`，不暴露哪一字段错误。

Cursor 不包含 metadata/value，不写日志。15 分钟 TTL 限制旧 generation/master-key token 生命周期。

### 11.3 Pagination consistency

每一页是独立 SQLite read snapshot，不跨页面持有 transaction/connection：

- 已返回 key 不会因 cursor keyset 而重复；
- 在 `last_key` 之后新插入的 key 可能出现在后页；
- 在尚未读取前被删除/过期的 key不会出现；
- 插入到 `last_key` 之前的 key不会出现在本次继续翻页；
- 不承诺 list 开始时的全 namespace snapshot。

这符合 P0“常用 API”目标，也避免长 cursor 持有 WAL reader 阻止 checkpoint。

## 12. Adapter 与内部协议

### 12.1 `KVNamespace` adapter

在 loader-host module graph 静态导出：

```js
export class KVNamespace extends WorkerEntrypoint {
  async get(keyOrKeys, typeOrOptions) { /* validate + typed backend call */ }
  async getWithMetadata(keyOrKeys, typeOrOptions) { /* ... */ }
  async put(key, value, options) { /* ... */ }
  async delete(key) { /* ... */ }
  async list(options) { /* ... */ }
}
```

Adapter 从 `this.ctx.props` 读取 P0.3 trusted props，从 `this.env.BINDING_BACKEND` 读取 private service
capability。它不能向 tenant 导出 props、backend、token 或 helper method。

为避免 Workers RPC reserved method 冲突，module load 时运行静态 assertion；公开 surface 只包含
上表方法。所有参数先在 JS 侧限制结构/数量，再由 Rust authoritative validation 重做。

### 12.2 Endpoint matrix

| Method | Endpoint | Request body | Response |
| --- | --- | --- | --- |
| get single | `/kv/{binding}/get` | binary key + options | headers + byte stream/not-found |
| get metadata | `/kv/{binding}/get-with-metadata` | binary key + options | metadata frame + byte stream |
| get array | `/kv/{binding}/get-many` | bounded key frame | `KVBatchFrameV1` |
| put | `/kv/{binding}/put` | option frame + value stream | empty success/error envelope |
| delete | `/kv/{binding}/delete` | binary key | empty success/error envelope |
| list | `/kv/{binding}/list` | prefix/limit/cursor frame | bounded canonical result frame |

完整实际 path 前缀是 P0.3 `/internal/bindings/v1`。每种 frame 有 magic/version/declared lengths；
decoder 使用 checked arithmetic、拒绝 trailing bytes、unknown flags 和 non-canonical varint。

### 12.3 Streaming

Workers RPC 支持 byte-oriented `ReadableStream` 的传输与 flow control；单 value stream 路径保持：

```text
SQLite blob -> bounded Rust chunks -> HTTP Response.body
    -> loader-side adapter -> tenant ReadableStream
```

- 不把 stream tee 到 log；
- tenant cancel -> adapter cancel -> HTTP body drop -> backend task cancellation -> blob/tx/conn/pin drop；
- backend/tenant 都有 idle timeout 与总 duration upper bound；
- 每个 chunk 计入 result budget；
- stream EOF 前发生 corruption/I/O error，stream errored，不返回截断成功；
- `text/json/arrayBuffer` 由 adapter 在 25 MiB 上限内消费相同 byte stream。

## 13. Handle manager 与并发

### 13.1 Default bounds

所有值可由 operator 配置降低/提高，但启动时有硬上限校验。V1 初始默认：

```text
global SQLite connections: 64
per namespace: 1 writer + up to 2 readers
global active value streams: 16
per namespace active value streams: 4
idle handle TTL: 60s
SQLite busy timeout: 5s
foreground operation timeout: 30s (stream 使用独立 idle/total limit)
```

这些是 SMB 单体默认，不是 API contract。Gate 以更小 limit 注入，强制覆盖 contention/eviction。

### 13.2 Handle state

```text
KvHandle {
  resource_id + spec_generation
  serialized writer lane
  bounded reader/stream admission gates
  stream semaphore
  last_used
}
```

- cache key 不用 namespace name/path；
- first open per key singleflight；
- cold open 验证 `kv_meta` identity/schema；每次 operation 使用受全局和 per-namespace gate 约束的新
  connection，不保留 idle SQLite FD；
- one writer lane 使用 `BEGIN IMMEDIATE`；
- read connection 在 blob stream EOF/cancel 前不释放；
- global LRU eviction 只选没有 active caller 的 handle；
- eviction：从 handle cache 移除 -> checkpoint best effort -> drop；
- delete fence 走强制 checkpoint/close，失败则 resource 保持 deleting/unavailable，不误删文件；
- 所有 rusqlite 操作在 bounded blocking executor；async task 只做 orchestration/stream。

### 13.3 Fairness

- 每 namespace mutation 串行化，不承诺跨异步请求的到达顺序；
- global/per-resource connection 和 stream gate 避免一个 namespace 占满；
- long stream 受独立配额，不占 writer connection；
- GC/backup/checkpoint 使用 background priority，在 foreground backlog 时暂停；
- SQLite busy 不进行无限指数 retry；超过 bounded busy/operation timeout 返回 `KV_BUSY`。

## 14. WAL、checkpoint 与 disk pressure

### 14.1 Checkpoint

- SQLite auto-checkpoint 提供常规控制；
- handle idle eviction、backup前后和 resource delete 执行显式 checkpoint；
- maintenance 采样 WAL size 到固定 bucket metric，不记录 resource label；
- V1 不另设 WAL admission threshold；active stream hard bound、SQLite auto-checkpoint、idle/maintenance
  checkpoint 与 filesystem safety floor 共同限制增长；
- restart 后 SQLite 正常 WAL recovery，随后验证 identity/schema。

### 14.2 Disk safety

Mutation 前后检查：

- namespace `max_page_count`/quota；
- data filesystem free-space safety floor；
- global/per-resource active staging count × 25 MiB 单值上限形成的确定 byte ceiling；
- WAL size bucket 与 SQLite `FULL`/I/O signal；
- mutation failure 后旧 committed value 保持可见。

Disk pressure 时：

- 新 create/put/backup staging 失败为 `KV_STORAGE_FULL`；
- read/list/delete/GC 尽量继续；
- 不删除 tenant entry 或旧 backup 来“自动腾空间”；
- 不把 partial staging 误认 authority；
- platform readiness 可以进入 degraded，但现有可读 namespace 继续服务。

## 15. Backup 与 restore

### 15.1 Online backup

使用 SQLite Online Backup API，而不是直接复制 live `data.sqlite`：

```text
POST backup
    └── kv_backups(state=creating)
            └── ResourcePin + read/backup slot
                    └── SQLite Online Backup -> backup-staging/<backup-id>.sqlite
                            ├── bounded pages per step / foreground yield
                            ├── quick_check + kv_meta verify
                            ├── fsync + SHA-256 + size
                            └── foundation S3 client / immutable system-prefix upload
                                    └── kv_backups(state=ready)
```

参考：[SQLite Online Backup API](https://www.sqlite.org/backup.html)。`rusqlite` 启用 `backup`
feature。S3 object key 由 host 生成：

```text
system/backups/kv/<account-id>/<resource-id>/<backup-id>/data.sqlite
system/backups/kv/<account-id>/<resource-id>/<backup-id>/manifest.json
```

Manifest 至少包含 backup schema、resource ID、SQLite schema version、sha256、size、creation time；
不含 resource name、master key、credential 或 DB path。Upload 完成并 read-after-write verify 后才把 row
标 ready。

Backup failure 不把 live namespace 标 unavailable。失败 staging/partial S3 object 由 idempotent GC
按 exact backup ID 清理。

### 15.2 Restore creates new identity

P0.4 唯一 restore 形式：

```text
POST /kv/namespaces:restore { backupId, newName }
        ↓
new ResourceId, state=creating
        ↓
download by stored object key -> secure staging
        ↓
size/hash/manifest/schema/quick_check verify
        ↓
transactionally rewrite kv_meta account_id/resource_id
        ↓
close/fsync/atomic publish
        ↓
new resource ready
```

- 不覆盖 source/live namespace；
- backup source 可以已 tombstoned，但 account scope 必须一致；
- restore 后需要新 Worker deployment 才能绑定新 resource ID；
- wrong hash、newer schema、corruption 或 quota overflow 失败并隔离 staging；
- in-place restore/rollback 留到后续 maintenance mode，不进入 P0.4。

### 15.3 Backup retention

- namespace delete 默认保留 ready backups；
- backup delete 有自己的 deleting/tombstone 和 S3 artifact ref cleanup；
- 不允许 caller 提供 object key；
- S3 unavailable 不影响 KV primary read/write，只影响 backup/restore readiness；
- orphan audit 只扫描系统 prefix/manifest，不能 broad-delete tenant R2 objects。

## 16. Corruption 与健康隔离

### 16.1 Open validation

每次 cold open：

1. secure path/owner/type/no-symlink 验证；
2. SQLite header/open；
3. `kv_meta.format/schema/account/resource` 精确匹配；
4. schema fingerprint/migration version 检查；
5. 根据 `last_quick_check_ms` 和 policy 决定后台 `quick_check`。

Create/restore 一定跑完整 `PRAGMA quick_check`。普通 request 不每次跑，避免 O(database) 延迟。

### 16.2 Runtime corruption

以下错误触发 namespace isolate：

```text
SQLITE_CORRUPT
SQLITE_NOTADB
identity/schema mismatch
impossible invalid UTF-8 key
invalid canonical metadata bytes
repeated unrecoverable I/O error
```

动作：

- abort 当前 operation/stream；
- handle 标 closing 并停止新 checkout；
- `resources` 持久化 `ready + unavailable` 与 stable code；
- 保留原文件/WAL，不自动删除或覆盖；
- 其他 namespace、Workers 和平台 `/health/live` 继续；
- operator 通过 backup restore 创建新 identity；
- 原 resource 的 deployment bindings仍引用原 ID，因此稳定失败，不会静默切到 restore 后的数据。

### 16.3 Quick-check scheduler

- bounded、低优先级、一次最多一个大 DB；
- 优先最近出现 I/O anomaly 或长期未检查的 namespace；
- foreground busy 时暂停；
- result 只记录 resource ID/code/duration，不记录 entry；
- quick_check 成功可清除 transient degraded，但 `unavailable/corrupt` 需要明确 operator repair/restore。

## 17. Control API

使用平台原生、产品专用 API；路径可以按现有 router 命名调整，语义固定：

```text
POST   /v1/accounts/{account}/kv/namespaces
GET    /v1/accounts/{account}/kv/namespaces
GET    /v1/accounts/{account}/kv/namespaces/{resource-id}
PATCH  /v1/accounts/{account}/kv/namespaces/{resource-id}   # rename only
DELETE /v1/accounts/{account}/kv/namespaces/{resource-id}

POST   /v1/accounts/{account}/kv/namespaces/{resource-id}/backups
GET    /v1/accounts/{account}/kv/backups
POST   /v1/accounts/{account}/kv/namespaces:restore
DELETE /v1/accounts/{account}/kv/backups/{backup-id}
```

规则：

- create/delete/backup/restore 支持 control idempotency key；
- create/restore 可能返回 operation/state，client poll 到 ready/failed；
- list/get 返回 ID/name/state/availability/quota/schema/timestamps，不返回 storage key/path；
- delete referenced 返回 referrer kind/ID 的受限列表，绝不 force cascade；
- runtime key/value API 不通过 public control endpoint 暴露；P0.4 不做 admin data browser；
- account authorization 与 not-found concealment 复用 P0.3。

## 18. Error mapping

在 P0.3 error model 上增加：

| Stable code | Tenant behavior | Retry | Commit unknown |
| --- | --- | --- | --- |
| `KV_KEY_INVALID` | `TypeError` | no | no |
| `KV_KEY_TOO_LARGE` | `TypeError` | no | no |
| `KV_VALUE_TOO_LARGE` | `TypeError`/rejected promise | no | no |
| `KV_METADATA_INVALID` | `TypeError` | no | no |
| `KV_METADATA_TOO_LARGE` | `TypeError` | no | no |
| `KV_INVALID_OPTIONS` | `TypeError` | no | no |
| `KV_TOO_MANY_KEYS` | `TypeError` | no | no |
| `KV_RESPONSE_TOO_LARGE` | rejected promise | caller narrows request | no |
| `KV_CURSOR_INVALID` | `TypeError` | restart listing | no |
| `KV_BUSY` | rejected promise | yes | no if before tx commit |
| `KV_STORAGE_FULL` | rejected promise | after operator action | no/unknown by phase |
| `KV_UNAVAILABLE` | rejected promise | yes/operator | no |
| `KV_CORRUPT` | rejected promise | no automatic retry | no |
| `KV_RESULT_UNKNOWN` | rejected promise | caller decides | yes |
| `KV_INTERNAL_PROTOCOL_ERROR` | rejected promise | maybe | depends on operation |

Tenant error message 只包含 stable code/request ID，不包含 key/value/metadata、SQLite code text、path、
SQL、cursor payload 或 raw cause。JS argument validation 与 Rust backend 应映射到相同 stable code；
Rust 是最终 authority。

## 19. Observability

新增低基数 metrics：

```text
kv_operations_total{operation,outcome,type}
kv_operation_duration_seconds{operation}
kv_operation_bytes{operation,direction}
kv_open_connections{role}
kv_active_streams{}
kv_staging_bytes{}
kv_wal_bytes_bucket{}
kv_gc_entries_total{outcome}
kv_checkpoint_total{outcome}
kv_backup_total{outcome}
kv_restore_total{outcome}
kv_corruption_total{class}
```

禁止 key、prefix、namespace/account ID、binding name、cursor、backup object key 作为 label。Structured
log 可以记录 resource/binding/request/backup ID，不能打印 tenant payload或内部 physical locator。

## 20. 工作包与依赖顺序

### P0.4.0：Control schema、paths 与 lifecycle driver

- `004_kv.sql`、`kv_namespaces`、`kv_backups` repository；
- `KvPaths` secure layout；
- create/reconcile/delete/quarantine；
- namespace control API、idempotency；
- DB identity/schema/quick_check。

完成条件：空目录 create、每个 crash point restart、rename、referenced delete、同名重建通过；尚不
接 Worker binding。

### P0.4.1：SQLite engine 基础 CRUD

- rowid schema 与 pragmas；
- key/metadata/options canonicalization；
- single get/put/delete；
- `zeroblob` + incremental BLOB；
- expiration read filter 与 deterministic Clock；
- writer lane、blocking executor、basic handle open/close。

完成条件：engine-level 原子替换、25 MiB boundary、WAL restart、concurrent reader/writer、disk-full
fault injection 通过。

### P0.4.2：Binding adapter 与 backend

- `KVNamespace@1` descriptor/config/permissions；
- P0.3 BindingFactory 注册；
- private typed endpoints/frame；
- single get/put/delete end-to-end；
- stable error/result-unknown 与 request/resource pin。

完成条件：真实 workerd tenant Worker 能在 cold/warm path CRUD；跨 namespace/account/伪造 binding
失败。

### P0.4.3：Types、metadata、array get 与 stream

- `getWithMetadata`；
- text/json/arrayBuffer/stream；
- array get 与 `KVBatchFrameV1`；
- streaming staging/backpressure/cancel；
- aggregate response budget。

完成条件：binary 25 MiB、metadata 1024、100-key Map、cancel/timeout 没有 connection/pin/temp leak。

### P0.4.4：List、cursor 与 expiration GC

- UTF-8 byte keyset pagination；
- signed cursor V1；
- concurrent mutation pagination semantics；
- expiration GC/background scheduling；
- clock rollback/forward fault tests。

完成条件：prefix edge、Unicode order、cursor tamper/cross-namespace/expiry 与大量 expired row 情况
全部通过。

### P0.4.5：Handle LRU、WAL 与 disk pressure

- global/per-resource bounds、singleflight、LRU；
- checkpoint/long-reader policy；
- quota/max-page/global free-space safety；
- hundreds of mostly cold namespace test；
- delete fence 与 stream race。

完成条件：小 limit 压测无 async starvation、FD/connection/WAL/temp leak，不同 namespace 写可并行。

### P0.4.6：Backup、restore 与 corruption isolation

- Online Backup API、S3 manifest/hash/readback；
- restore-as-new-resource；
- backup lifecycle/retention/GC；
- quick-check scheduler、availability isolation、operator doctor。

完成条件：live writes during backup、S3 timeout/5xx、corrupt/truncated backup、单 live DB corruption 和
restore rebind 均有确定结果。

### P0.4.7：Conformance 与真实 workerd Gate

- API contract tests；
- stock workerd end-to-end；
- P0.2/P0.3 regression；
- 三轮 fresh-process、随机 seed、leak audit；
- 文档 compatibility/deviation matrix 固化。

## 21. 测试矩阵

### 21.1 Resource/lifecycle

| ID | 场景 | 断言 |
| --- | --- | --- |
| KV-R01 | create empty dir | secure layout、identity、ready 正确 |
| KV-R02 | create crash matrix | creating 重启后收敛，无重复 physical DB |
| KV-R03 | rename | ID/path/binding 不变 |
| KV-R04 | referenced delete | retained deployment 阻止删除 |
| KV-R05 | delete drain | stream drain/timeout 后原子 quarantine |
| KV-R06 | same-name recreate | 新 ID/空 DB，旧 deployment 不可访问 |
| KV-R07 | cross account | control/deploy/runtime 都不泄露存在性 |

### 21.2 Key/value/metadata

| ID | 场景 | 断言 |
| --- | --- | --- |
| KV-D01 | empty、`.`、`..` | stable TypeError |
| KV-D02 | 512/513 UTF-8 bytes | boundary 精确 |
| KV-D03 | NUL、slash、emoji、NFC/NFD | exact distinct bytes，不进 path |
| KV-D04 | lone surrogate | adapter/backend 确定拒绝 |
| KV-D05 | empty/25 MiB/25 MiB+1 value | exact boundary，超限不改旧值 |
| KV-D06 | ArrayBufferView offset | 只保存 view range |
| KV-D07 | metadata 1024/1025 | canonical bytes boundary |
| KV-D08 | circular/BigInt/non-finite | deterministic validation error |
| KV-D09 | JSON invalid value | get json reject且不泄露 value |

### 21.3 API semantics

| ID | 场景 | 断言 |
| --- | --- | --- |
| KV-A01 | missing single get | `null` |
| KV-A02 | missing getWithMetadata | `{value:null, metadata:null}` |
| KV-A03 | missing delete | success |
| KV-A04 | text/json/arrayBuffer/stream | bytes/类型正确 |
| KV-A05 | array get mixed missing | Map 顺序、null、重复 key 折叠正确 |
| KV-A06 | 100/101 keys | boundary 精确 |
| KV-A07 | aggregate >25 MiB | 整体失败，无 partial Map |
| KV-A08 | expiration/Ttl min/both/overflow | stable options behavior |
| KV-A09 | cacheTtl 29/30 | 29 拒绝，30 接受但无 cache side effect |
| KV-A10 | read-only binding | get/list成功，put/delete拒绝 |

### 21.4 List/expiration

| ID | 场景 | 断言 |
| --- | --- | --- |
| KV-L01 | byte-order fixture | UTF-8 bytes 顺序与 expected vector 一致 |
| KV-L02 | empty/all-0xff successor edge | query bound 正确 |
| KV-L03 | default/1/1000/1001 limit | contract 正确 |
| KV-L04 | multi-page | 无静态数据时无重复/遗漏 |
| KV-L05 | cursor tamper | HMAC fail，统一 error |
| KV-L06 | cursor cross namespace/prefix | fail closed |
| KV-L07 | cursor expiry/generation | fail closed |
| KV-L08 | concurrent insert/delete | 符合已记录的 keyset 语义 |
| KV-L09 | expired rows between pages | 不返回过期 key，list_complete 正确 |
| KV-L10 | GC restart | API 不受 task 丢失影响，最终回收 |

### 21.5 Concurrency/crash/stream

| ID | 场景 | 断言 |
| --- | --- | --- |
| KV-C01 | two put same key | 每个值原子可见，无 torn bytes |
| KV-C02 | reader during 25 MiB put | 看到旧或新完整值，不见 zeroblob partial |
| KV-C03 | crash before/inside/after commit | old/new/result-unknown 分类正确 |
| KV-C04 | WAL restart | recovery 后 identity/data 正确 |
| KV-C05 | stream backpressure | bounded memory |
| KV-C06 | tenant cancel | blob/tx/conn/pin/temp 释放 |
| KV-C07 | slow stream timeout | 只影响该调用 |
| KV-C08 | many namespaces | LRU/FD/connection hard bound |
| KV-C09 | hot + cold fairness | hot namespace 不饿死其他 resource |
| KV-C10 | disk full | old committed value保留，delete/read 尽量可用 |

### 21.6 Backup/corruption

| ID | 场景 | 断言 |
| --- | --- | --- |
| KV-B01 | writes during online backup | snapshot 自洽、quick_check 通过 |
| KV-B02 | backup S3 timeout/5xx | backup failed，live KV healthy |
| KV-B03 | response loss/retry | idempotency 返回同 backup ID |
| KV-B04 | manifest/hash mismatch | restore 拒绝，不发布 staging |
| KV-B05 | restore as new | 新 ID/name，entry/TTL/metadata 保留 |
| KV-B06 | source deleted | retained ready backup 仍可同 account restore |
| KV-B07 | corrupt one namespace | 该 resource unavailable，其他 namespace 正常 |
| KV-B08 | identity/path swap | invariant violation，绝不打开别的 DB |
| KV-B09 | trash/staging orphan | 只清理可证明属于 exact operation 的对象 |

### 21.7 Real workerd regression

至少用 tenant fixtures 覆盖：

- 同一 deployment 两个 KV binding，key 相同但值隔离；
- A/B deployment 绑定不同 namespace，promotion/rollback 取回正确 binding；
- warm loader 前 tamper binding/DB identity，request fail closed；
- restart 后 cold load、KV data、cursor verification 和 WAL recovery；
- stream request/response cancel；
- global `fetch()` 无法直达 BindingBackend；
- tenant 不能枚举 props/token/path/resource ID；
- invalid deployment 不修改当前 active 或 KV；
- P0.2 no-binding Worker 与 public egress 无回归。

## 22. P0.4 Exit Gate

进入 R2/D1 前必须同时满足：

- P0.3 Exit Gate 保持通过；
- namespace create/delete/reconcile crash matrix 全通过；
- KV common API compatibility matrix 全通过；
- 25 MiB streaming put/get 的内存、backpressure、cancel 和 atomicity 通过；
- UTF-8 list order、signed cursor、TTL/GC 在 deterministic clock 下通过；
- connection LRU、single writer、multi-namespace parallelism 和 disk pressure 通过；
- online backup/restore-as-new 与 S3 failure matrix 通过；
- 单 namespace corruption 完整隔离；
- 三轮 fresh-process stock-workerd Gate 通过；
- 无 child、listener、FD、connection、WAL、staging file、resource pin 泄漏；
- compatibility/deviation 被 API matrix 和本文固定，未实现能力不伪装成功。

2026-08-25 验证记录：

- `cargo fmt --all --check`、workspace clippy（`-D warnings`）、no-default-features、Rust 1.98 MSRV、
  metadata、dependency boundaries、`git diff --check` 全部通过；
- `cargo test --workspace --all-targets --all-features -- --test-threads=1` 全部通过；
- `./scripts/test-p0-4`：P0.4、P0.3、P0.2 各三轮 fresh process 全部通过；
- `./scripts/coverage` 从 clean coverage state 运行全部测试和真实 P0.1–P0.4 Gate，
  `22166 / 24612` production Rust lines covered，90.06%。

## 23. 参考资料

- [Cloudflare Workers KV：Read](https://developers.cloudflare.com/kv/api/read-key-value-pairs/)
- [Cloudflare Workers KV：Write](https://developers.cloudflare.com/kv/api/write-key-value-pairs/)
- [Cloudflare Workers KV：Delete](https://developers.cloudflare.com/kv/api/delete-key-value-pairs/)
- [Cloudflare Workers KV：List](https://developers.cloudflare.com/kv/api/list-keys/)
- [Cloudflare Workers KV：How KV works](https://developers.cloudflare.com/kv/concepts/how-kv-works/)
- [Cloudflare Workers RPC](https://developers.cloudflare.com/workers/runtime-apis/rpc/)
- [Cloudflare workers-types `KVNamespace`](https://github.com/cloudflare/workers-types/blob/master/index.d.ts)
- [SQLite incremental BLOB / WITHOUT ROWID constraint](https://www.sqlite.org/withoutrowid.html)
- [SQLite datatype and BLOB ordering](https://www.sqlite.org/datatype3.html)
- [SQLite WAL](https://www.sqlite.org/wal.html)
- [SQLite Online Backup API](https://www.sqlite.org/backup.html)
- [总体方案](./sqlite-workerd-platform.md)
