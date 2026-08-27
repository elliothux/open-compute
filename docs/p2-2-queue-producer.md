# P2.2：Queue Producer 详细设计

> 状态：已实现并完成本地 Exit Gate；结论为 Conditional Go，详见
> [P2.2 本地验证结果](./p2-2-results.md)
>
> 前置依赖：[P2.1：Scheduler 多 Workload 内核](./p2-1-scheduler-hardening.md)必须先通过 Exit Gate；
> P1.0 至 P1.7 已由用户确认跑通，P1.8 的 WebSocket hibernation No-Go 不影响本阶段。
>
> 直接依赖：[P0.2：Workers Runtime](./p0-2-workers-runtime.md)、
> [P0.3：Resource 与 Binding Framework](./p0-3-resource-binding-framework.md)、
> [P0.8：Scheduler/Alarms](./p0-8-scheduler-do-alarms.md)、
> [P1：P0 平台加固](./p1-platform-hardening.md)、
> [P2.1：Scheduler 多 Workload 内核](./p2-1-scheduler-hardening.md)
>
> 后续消费者：[P2.3：Queue Consumer/Cron](./p2-3-queue-consumer-cron.md)。在 P2.2 结束前不实现
> `queue()` handler、ack/retry、consumer concurrency、DLQ 或 Cron。

P2.2 实现 Queue resource lifecycle 和 Worker producer binding：`send()`、`sendBatch()`、
`metrics()`、delivery delay、payload limit、持久化、隔离、restart/restore。消息 authority 放在
`scheduler.sqlite`，提交成功后即使没有 consumer 也会一直保留到 retention 到期或 Queue 被显式删除。

本阶段不是“先做一个内存 broker 再说”。Miniflare 的 in-memory broker 可用于 API/serialization
对照，但 SMB self-deploy 的核心要求是进程崩溃后消息仍存在。因此 producer promise 只有在本地
SQLite transaction durable commit 后才 resolve。

## 0. 决策摘要

| 主题 | P2.2 选择 | 原因 |
| --- | --- | --- |
| Queue catalog | `control.sqlite` 新建独立 `queues` catalog | 现有 `resources.kind` CHECK 已冻结，安全升级不应关闭 FK 重建整张引用图 |
| Producer binding | 独立 `queue_producer_bindings`，运行时与现有 binding 合并 | 保留 immutable deployment snapshot，又不破坏 P0 resource FK |
| Message authority | `scheduler.sqlite.queue_messages` | Queue 是 scheduler workload；同库支持 delay、retention 和后续 consumer claim |
| 资源粒度 | 所有 Queue 共用 `scheduler.sqlite`，按 queue ID 隔离 | Queue 需要全局 due index、公平调度和原子 DLQ move；不适合一 Queue 一文件 |
| Delivery | P2.2 只 enqueue，不 dispatch consumer | 将 durable producer 与 at-least-once consumer crash matrix 分阶段验证 |
| Public API | `send`、`sendBatch`、`metrics` | 覆盖当前常用 Worker Queue producer surface |
| 内容类型 | JSON、text、bytes；V8 明确拒绝 | JSON/text/bytes 可稳定复刻；V8 serializer 不能用近似格式冒充 |
| 默认序列化 | 当前 compatibility policy 下默认 JSON | 对齐 `queues_json_messages` 兼容日期后的常用行为 |
| 单消息上限 | 128,000 bytes | 对齐 Cloudflare 常用 API limit，按十进制 byte 计算 |
| batch 上限 | 100 条、总 body 256,000 bytes | 对齐常用 producer API；facade 与 host 双重检查 |
| delay | message > batch > Queue default，显式 0 关闭默认 delay | 对齐 Cloudflare precedence |
| producer transaction | 一个 batch 一次 transaction、全成或全败 | 本地语义清楚，重启后不会出现半 batch |
| message ID | host 生成 UUIDv7 | 不信任 tenant ID，稳定、可排序且不泄露 DB rowid |
| delivery 语义 | producer response 丢失后重试可能重复 | binding API 没有 idempotency key，不能虚构 exactly-once |
| DO output gate | P2.2.0 Hard Gate；失败时 DO 内 Queue producer fail closed | native workerd Queue 会等待 DO output gate，facade 不能静默提早 enqueue |
| retention | Queue 配置 60 秒至 14 天，默认 4 天；P2.2 即实现 sweep | 没 consumer 的 Queue 也必须有有界磁盘生命周期 |
| snapshot | 复用 P1 整机 snapshot 中的 `scheduler.sqlite` | 不再发明 Queue 私有 backup format；备份不保证保密性 |

### 0.1 P2.2 必须守住的不变量

1. Queue identity 是 immutable UUID；rename 只改 display name。
2. 同 account 内 live Queue name 唯一；同名 tombstone 后重建获得新 Queue ID。
3. deployment binding 只引用同 account、`ready/healthy` Queue 和精确 lifecycle generation。
4. binding descriptor 在 staging deployment 中冻结；warm/cold isolate看到同一份 canonical binding。
5. tenant 不能提交 Queue ID、binding ID、account ID或 generation 覆盖 trusted descriptor。
6. `sendBatch()` 在一笔 SQLite transaction 中全成或全败，按输入顺序获得连续 enqueue sequence。
7. producer response 只在 WAL/FULL transaction commit 后返回；不等待 consumer。
8. Queue state/generation/config projection 不一致时 send fail closed，不能使用 stale default delay、
   retention 或 quota。
9. default、batch 和 per-message delay 的优先级固定，显式 `0` 不能被 Queue default 覆盖。
10. payload size 在 loaded isolate facade 和 Rust backend 都检查；backend 是最终 authority。
11. backlog count/bytes 与 message rows 在同一 transaction 更新，restart 后可校验和修复。
12. retention 删除和 producer insert 都受 P1 disk admission 与 Queue backlog quota 约束。
13. Queue delete 有任何 deployment producer binding、未来 consumer 或 DLQ referrer时失败；
    `force` 不能绕过 referrer。
14. 普通 Worker 可以 enqueue；DO 中只有 Hard Gate 证明 output-gate 等价后才开放。
15. P2.2 不产生 consumer delivery，不调用 tenant `queue()` handler。

### 0.2 非目标

- Queue consumer、`MessageBatch`、`ack()`、`retry()`、batch retry 或 max attempts；
- dead-letter Queue、consumer pull API、HTTP enqueue endpoint；
- Cron；
- exactly-once producer 或 exactly-once delivery；
- Cloudflare 全球 Queue、25 GB plan backlog、每秒 5,000 message 的托管容量承诺；
- Queue dashboard、Wrangler 命令完全兼容、billing 或 plan limits；
- V8 serialization、metadata、content-based dedup、priority 或 partition key；
- 多进程 broker、leader election 或跨主机 replication；
- Queue 单资源 export/restore；P2.2 只参加整机 snapshot/restore；
- message body 搜索、list/debug API；operator summary 不返回 tenant payload。

## 1. 兼容目标

### 1.1 tenant-facing interface

P2.2 capability V1 暴露：

```ts
type QueueContentType = "text" | "bytes" | "json";

interface QueueSendOptions {
  contentType?: QueueContentType;
  delaySeconds?: number;
}

interface QueueSendBatchOptions {
  delaySeconds?: number;
}

interface MessageSendRequest<Body = unknown> {
  body: Body;
  contentType?: QueueContentType;
  delaySeconds?: number;
}

interface QueueMetrics {
  backlogCount: number;
  backlogBytes: number;
  oldestMessageTimestamp?: Date;
}

interface Queue<Body = unknown> {
  send(body: Body, options?: QueueSendOptions): Promise<QueueSendResponse>;
  sendBatch(
    messages: Iterable<MessageSendRequest<Body>>,
    options?: QueueSendBatchOptions,
  ): Promise<QueueSendBatchResponse>;
  metrics(): Promise<QueueMetrics>;
}
```

本项目生成的 capability V1 类型不宣传 `v8`；使用 Cloudflare 全量类型的代码若显式传入 `"v8"`，
runtime 仍以稳定 `QUEUE_CONTENT_TYPE_UNSUPPORTED` 拒绝。

`send` 和 `sendBatch` 返回：

```js
{
  metadata: {
    metrics: {
      backlogCount: 12,
      backlogBytes: 4096,
      oldestMessageTimestamp: new Date(1_787_670_000_000),
    }
  },
}
```

这里按 pinned workerd source 把 `oldestMessageTimestamp` 物化为 JavaScript `Date`，空 Queue 为
`undefined`。文档页面有时把它描述成 timestamp number；本项目以实际 pinned runtime 类型与测试为准。

### 1.2 支持矩阵

| surface | P2.2 | 说明 |
| --- | --- | --- |
| `send(body)` | 支持 | 当前 compatibility policy 下默认 JSON |
| `send(..., {contentType:"json"})` | 支持 | UTF-8 JSON bytes |
| `contentType:"text"` | 支持 | body 必须是 string |
| `contentType:"bytes"` | 支持 | body 必须是 ArrayBufferView |
| `contentType:"v8"` | 不支持 | stable `QUEUE_CONTENT_TYPE_UNSUPPORTED` |
| `delaySeconds` | 支持 | 整数 0 至 86,400 |
| `sendBatch(iterable)` | 支持 | 非空、最多 100；不能只接受 Array |
| `metrics()` | 支持 | durable backlog count/bytes/oldest enqueue timestamp |
| metadata | 不支持 | 未在 capability V1 声明 |
| producer in ordinary Worker | 支持 | Hard Gate 通过后 |
| producer in Durable Object | 条件支持 | 必须通过 output-gate Gate |
| producer in Workflow | 不适用 | Workflow 尚未实现 |

### 1.3 limit 与 delay

固定 API compatibility limits：

```text
MAX_MESSAGE_BYTES = 128_000
MAX_BATCH_MESSAGES = 100
MAX_BATCH_BODY_BYTES = 256_000
MAX_DELAY_SECONDS = 86_400
```

字节数只计算序列化后的 message body，不计算 internal envelope、base64、HTTP header 或 SQLite row
overhead。Rust backend 从 binary frame 中重新计算实际 body length，不能信任 facade 提交的计数。

delay precedence：

```text
per-message delaySeconds
    > sendBatch options.delaySeconds
    > Queue delivery_delay_seconds
    > 0
```

字段 absent 才向下继承；显式 `0` 是有效值。

## 2. 为什么 Queue 不写入现有 resources 表

当前 `003_resource_bindings.sql` 把：

```sql
resources.kind
deployment_bindings.kind
```

都用 CHECK 固定为：

```text
kv_namespace | r2_bucket | d1_database | do_namespace
```

并且 `deployment_bindings`、`resource_referrers`、`control_idempotency` 等表通过 FK/trigger 构成已发布
引用图。SQLite 不能直接扩展 CHECK；必须重建表。对当前 schema 做的最小验证表明，即使
`foreign_keys=ON` 且设置 deferred FK，drop/rename 父表的直观迁移也会在 commit 时触发 FK failure。

P2.2 禁止：

- `PRAGMA foreign_keys=OFF` 后重建发布中的核心表；
- 修改 `sqlite_master`；
- 保留新旧两套 resource row；
- 把 Queue 假装成某种现有 kind；
- 只在 Rust 层校验而让数据库接受孤儿 binding。

因此 control migration 009 增加独立 Queue catalog 和 producer binding。它仍复用 P0.3 的 lifecycle、
immutable deployment 和 canonical descriptor原则，但不冒险重写 P0 已冻结的 FK 图。

Queue 与 KV/D1 的物理模型也不同：所有 Queue message 必须参与同一 scheduler fairness/due scan，
未来 DLQ move 在同一个 scheduler DB transaction 内完成更简单。一 Queue 一 SQLite 文件会产生大量
connection/WAL、跨 Queue DLQ 非原子和全局 due 扫描 fan-out，因此本阶段明确不采用。

## 3. 交付架构

```text
Control API
├── control.sqlite
│   ├── queues                         # lifecycle/config authority
│   ├── queue_producer_bindings        # immutable deployment refs
│   └── queue_referrers                # delete guards
└── QueueReconciler
        └── scheduler.sqlite
            ├── queue_state             # runtime lifecycle/config projection
            └── queue_messages          # durable message authority

RuntimeSource
├── deployment_bindings                # KV/R2/D1/DO
└── queue_producer_bindings             # Queue
        └── one canonical env binding list
                └── loader-host QueueTransport
                        └── loaded-isolate Queue facade
                                └── private Rust QueueBackend
                                        └── scheduler.sqlite transaction
```

职责分工：

- `control.sqlite.queues` 决定谁拥有 Queue、名称、lifecycle generation 和配置；
- `scheduler.sqlite.queue_state` 是 send path 可单库验证的运行时 projection；
- `queue_messages` 是已提交 message 的唯一 authority；
- QueueReconciler 只从 control authority 推进 queue_state，不反向改控制面意图；
- RuntimeSource 只从 active immutable deployment 取 binding，不接受 request override；
- tenant-facing facade 负责 JS shape/serialization；Rust backend重新验证并持久化。

## 4. Control schema：`009_queues.sql`

以下是必须表达的字段和约束；最终 SQL 可以按现有 migration helper 调整，但不能削弱不变量。

### 4.1 `queues`

```sql
CREATE TABLE queues (
  id                       TEXT PRIMARY KEY
                           CHECK(length(id) = 36 AND id = lower(id)),
  account_id               TEXT NOT NULL REFERENCES accounts(id),
  name                     TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 128),
  state                    TEXT NOT NULL CHECK(state IN (
                             'creating', 'ready', 'deleting', 'tombstoned'
                           )),
  availability             TEXT NOT NULL CHECK(availability IN (
                             'healthy', 'degraded', 'unavailable'
                           )),
  availability_code        TEXT,
  lifecycle_generation     INTEGER NOT NULL CHECK(lifecycle_generation >= 1),
  config_generation        INTEGER NOT NULL CHECK(config_generation >= 1),
  delivery_delay_seconds   INTEGER NOT NULL
                           CHECK(delivery_delay_seconds BETWEEN 0 AND 86400),
  retention_seconds        INTEGER NOT NULL
                           CHECK(retention_seconds BETWEEN 60 AND 1209600),
  max_message_bytes        INTEGER NOT NULL CHECK(max_message_bytes > 0),
  max_batch_messages       INTEGER NOT NULL CHECK(max_batch_messages > 0),
  max_batch_bytes          INTEGER NOT NULL CHECK(max_batch_bytes > 0),
  max_backlog_bytes        INTEGER NOT NULL CHECK(max_backlog_bytes > 0),
  created_at_ms            INTEGER NOT NULL,
  updated_at_ms            INTEGER NOT NULL,
  deleted_at_ms            INTEGER,
  CHECK(availability_code IS NULL OR
        length(availability_code) BETWEEN 1 AND 128),
  CHECK((availability = 'healthy') = (availability_code IS NULL)),
  CHECK((state = 'tombstoned') = (deleted_at_ms IS NOT NULL))
) STRICT;

CREATE UNIQUE INDEX queues_live_name
ON queues(account_id, name)
WHERE state != 'tombstoned';

CREATE INDEX queues_reconcile
ON queues(state, availability, updated_at_ms, id)
WHERE state IN ('creating', 'deleting') OR availability != 'healthy';
```

API compatibility limit 固定为 128,000/100/256,000；这些字段仍持久化，是为了：

- runtime projection 自描述；
- future release 可以在 capability 声明下施加更低的 local safety limit；
- snapshot/restore 与旧 deployment 不依赖新 binary 的 default；
- reconciler 可比较完整 config generation。

P2.2 默认：

```text
delivery_delay_seconds = 0
retention_seconds = 345_600  # 4 days
max_message_bytes = 128_000
max_batch_messages = 100
max_batch_bytes = 256_000
max_backlog_bytes = operator-configured local default
```

`max_backlog_bytes` 是本地磁盘保护，不复制 Cloudflare plan 的 25 GB。它受 P1 host reserve 的更严格
限制。

### 4.2 `queue_producer_bindings`

```sql
CREATE TABLE queue_producer_bindings (
  id                         TEXT PRIMARY KEY
                             CHECK(length(id) = 36 AND id = lower(id)),
  deployment_id              TEXT NOT NULL REFERENCES worker_deployments(id),
  name                       TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 64),
  queue_id                   TEXT NOT NULL REFERENCES queues(id),
  queue_lifecycle_generation INTEGER NOT NULL CHECK(queue_lifecycle_generation >= 1),
  capability_version         INTEGER NOT NULL CHECK(capability_version >= 1),
  descriptor_sha256          BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms              INTEGER NOT NULL,
  UNIQUE(deployment_id, name)
) STRICT;

CREATE INDEX queue_producer_bindings_queue
ON queue_producer_bindings(queue_id, deployment_id, id);
```

insert trigger 必须验证：

- deployment 是 `staging`；
- deployment 的 Worker 与 Queue 属于同 account；
- Queue 是 `ready + healthy`；
- lifecycle generation 精确匹配；
- name 符合 P0.3 env identifier规则；
- name 不与 `deployment_vars`、`deployment_secrets`、`deployment_bindings` 或同 deployment 的其他
  Queue binding 冲突；
- capability version 已知。

update 永远拒绝。delete 只允许 staging/deleting deployment。active/retained deployment 的 Queue
binding 与现有 resource binding 一样 immutable。

### 4.3 `queue_referrers`

```sql
CREATE TABLE queue_referrers (
  queue_id       TEXT NOT NULL REFERENCES queues(id),
  referrer_kind  TEXT NOT NULL CHECK(referrer_kind IN (
                   'producer_binding', 'consumer', 'dlq'
                 )),
  referrer_id    TEXT NOT NULL,
  created_at_ms  INTEGER NOT NULL,
  PRIMARY KEY(queue_id, referrer_kind, referrer_id)
) STRICT, WITHOUT ROWID;
```

P2.2 只会创建 `producer_binding`，但 `consumer` 和 `dlq` 是已确定的 P2.3 引用类型，提前固定 CHECK
可避免下一阶段重建 delete-guard 表。trigger 保证 producer binding 与 referrer 同 transaction
创建/删除，孤儿 referrer 和删除 live referrer 都拒绝。

Queue 进入 `deleting` 前必须没有任何 referrer。`force=true` 只决定是否允许 purge backlog，永远
不能绕过 referrer。

### 4.4 control idempotency

```sql
ALTER TABLE control_idempotency
ADD COLUMN queue_id TEXT REFERENCES queues(id) DEFERRABLE INITIALLY DEFERRED;
```

Queue create/update/delete 复用现有 account + scope + key + HMAC fingerprint 协议。canonical fingerprint
覆盖 Queue ID、期望 generation、配置、`force` 与 API version；同 key 不同 payload 返回既有
idempotency conflict。

### 4.5 lifecycle trigger

允许：

```text
creating -> ready
creating -> deleting
ready    -> deleting
deleting -> tombstoned
```

identity、account、created_at、lifecycle generation 和 tombstone immutable。rename/update config 只在
`ready` 发生：

- rename 只改 name，不增加 generation；
- delivery delay/retention/quota update 增加 `config_generation + 1`；
- projection 未同步前 availability 变 `degraded/QUEUE_CONFIG_PENDING`；
- 同一 generation 不能被不同 config 覆盖。

## 5. Scheduler schema：`002_queue_producer.sql`

P2.1 通过后，scheduler migration registry 新增真实 version 2。

### 5.1 `queue_state`

```sql
CREATE TABLE queue_state (
  queue_id                 TEXT PRIMARY KEY
                           CHECK(length(queue_id) = 36 AND queue_id = lower(queue_id)),
  account_id               TEXT NOT NULL
                           CHECK(length(account_id) = 36 AND account_id = lower(account_id)),
  lifecycle_generation     INTEGER NOT NULL CHECK(lifecycle_generation >= 1),
  config_generation        INTEGER NOT NULL CHECK(config_generation >= 1),
  state                    TEXT NOT NULL CHECK(state IN (
                             'accepting', 'configuring', 'deleting'
                           )),
  delivery_delay_seconds   INTEGER NOT NULL
                           CHECK(delivery_delay_seconds BETWEEN 0 AND 86400),
  retention_seconds        INTEGER NOT NULL
                           CHECK(retention_seconds BETWEEN 60 AND 1209600),
  max_message_bytes        INTEGER NOT NULL CHECK(max_message_bytes > 0),
  max_batch_messages       INTEGER NOT NULL CHECK(max_batch_messages > 0),
  max_batch_bytes          INTEGER NOT NULL CHECK(max_batch_bytes > 0),
  max_backlog_bytes        INTEGER NOT NULL CHECK(max_backlog_bytes > 0),
  message_count            INTEGER NOT NULL DEFAULT 0 CHECK(message_count >= 0),
  message_bytes            INTEGER NOT NULL DEFAULT 0 CHECK(message_bytes >= 0),
  created_at_ms            INTEGER NOT NULL,
  updated_at_ms            INTEGER NOT NULL
) STRICT;
```

`queue_state` 没有到 control DB 的 FK；SQLite 文件间不能有 FK。正确性由 lifecycle protocol、
generation fence、reconciler 和 doctor cross-check 保证。

### 5.2 `queue_messages`

```sql
CREATE TABLE queue_messages (
  seq                  INTEGER PRIMARY KEY AUTOINCREMENT,
  id                   TEXT NOT NULL UNIQUE
                       CHECK(length(id) = 36 AND id = lower(id)),
  queue_id             TEXT NOT NULL REFERENCES queue_state(queue_id),
  queue_generation     INTEGER NOT NULL CHECK(queue_generation >= 1),
  enqueued_at_ms       INTEGER NOT NULL,
  available_at_ms      INTEGER NOT NULL,
  expires_at_ms        INTEGER NOT NULL,
  content_type         TEXT NOT NULL CHECK(content_type IN (
                         'json', 'text', 'bytes'
                       )),
  body                 BLOB NOT NULL,
  body_bytes           INTEGER NOT NULL CHECK(body_bytes >= 0),
  state                TEXT NOT NULL DEFAULT 'ready'
                       CHECK(state IN ('ready', 'claimed')),
  attempts             INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0),
  claim_token          BLOB,
  claim_until_ms       INTEGER,
  claimed_at_ms        INTEGER,
  CHECK(body_bytes = length(body)),
  CHECK(available_at_ms >= enqueued_at_ms),
  CHECK(expires_at_ms > enqueued_at_ms),
  CHECK(
    (state = 'ready' AND claim_token IS NULL AND
      claim_until_ms IS NULL AND claimed_at_ms IS NULL)
    OR
    (state = 'claimed' AND length(claim_token) = 32 AND
      claim_until_ms IS NOT NULL AND claimed_at_ms IS NOT NULL)
  )
) STRICT;

CREATE INDEX queue_messages_due
ON queue_messages(queue_id, state, available_at_ms, seq);

CREATE INDEX queue_messages_retention
ON queue_messages(expires_at_ms, queue_id, seq);

CREATE INDEX queue_messages_oldest
ON queue_messages(queue_id, enqueued_at_ms, seq);
```

P2.2 只写 `state='ready'`、`attempts=0`，不 claim message。`claimed` 与 claim columns 是下一阶段已知
hot-row 状态，预先固定可避免 P2.3 重建整张 backlog 表；P2.2 的 trigger 拒绝任何产品路径写 claimed。
P2.3 才开放对应 repository 方法和状态迁移。

不在 SQLite CHECK 中加入 `v8`。未来若实现 exact V8 serializer，必须通过新 capability 和 migration
显式扩展，不能让 P2.2 写无法读取的 body。

### 5.3 counters 与 invariant

insert/delete trigger 在同一 transaction 更新 `queue_state.message_count/message_bytes`。insert
还必须验证：

- queue_state 为 `accepting`；
- message queue_generation 等于 lifecycle generation；
- body/message/batch/backlog limit；
- `expires_at_ms` 与当前 retention config 一致；
- queue state account/generation 已由 trusted binding 验证。

SQLite trigger 无法安全表达 batch total 和 current host disk reserve；Rust transaction 在 insert
前做 batch/quota/admission 检查，trigger 作为 row-level最后防线。

doctor 提供只读 invariant query：

```sql
SELECT
  q.queue_id,
  q.message_count,
  COUNT(m.seq) AS actual_count,
  q.message_bytes,
  COALESCE(SUM(m.body_bytes), 0) AS actual_bytes
FROM queue_state q
LEFT JOIN queue_messages m ON m.queue_id = q.queue_id
GROUP BY q.queue_id
HAVING q.message_count != actual_count OR q.message_bytes != actual_bytes;
```

repair 是 authenticated/offline bounded operation：先 pause Queue pool，按 Queue ID 重算 counters，
transactional compare-and-set 后恢复；不能在每次 `metrics()` 都全表扫描。

## 6. Queue lifecycle 与跨库收敛

control 和 scheduler SQLite 无法跨文件 transaction。P2.2 使用显式 generation 和可重复 reconciler，
不使用 `ATTACH` 模拟跨库 authority。

### 6.1 Create

```text
transaction A / control:
  reserve idempotency
  insert queues(state=creating, availability=degraded,
                code=QUEUE_PROJECTION_PENDING, generations=1)

scheduler transaction:
  insert queue_state(state=accepting, exact config/generations, counters=0)

transaction B / control:
  compare id + generations
  creating -> ready
  availability -> healthy
  complete idempotency response
```

| crash point | reconcile |
| --- | --- |
| A 前 | 无 Queue，安全重试 |
| A 后、scheduler 前 | 创建缺失 queue_state |
| scheduler commit 后、B 前 | probe exact row/generation 后标 ready |
| B 后、response 前 | idempotency replay 返回同 Queue |

`creating` Queue 不能绑定或 send。

### 6.2 Rename

单 control transaction 修改 display name：

- Queue ID、lifecycle/config generation 和 scheduler row 不变；
- existing binding 不变；
- runtime/log/metric 继续使用 Queue ID；
- rename 不需要 workerd reload。

### 6.3 Config update

```text
control transaction A0:
  reserve idempotency; do not change Queue config yet

scheduler transaction A:
  require current lifecycle/config generation and state=accepting
  state=accepting -> configuring

control transaction B:
  validate expected config_generation
  write new config
  config_generation += 1
  availability=degraded, code=QUEUE_CONFIG_PENDING

scheduler transaction C:
  require state=configuring and same queue_id/lifecycle_generation
  replace config only if incoming config_generation is exactly old + 1
  remain state=configuring

control transaction D:
  compare exact config_generation
  availability=healthy

scheduler transaction E:
  compare exact lifecycle/config generation
  state=configuring -> accepting
```

projection pending 时所有 send fail `QUEUE_CONFIG_PENDING`。不能继续用旧 Queue default delay/retention；
这比短暂不可用更安全。rename 不走此流程，因为它不影响 send semantics。

先在 scheduler DB 建立 `configuring` fence，再修改 control authority，避免两个 SQLite 文件之间的窗口
继续使用旧配置。crash recovery：A 后/B 前看到 control 仍是旧 healthy generation，恢复旧配置并重新
accepting；B/C/D 之间按 control 的新 pending generation 继续投影；D 后/E 前把已验证的新 projection
切回 accepting。任一步都不能让 stale config 接受 message。

已有 message 的 `available_at_ms` 和 `expires_at_ms` 不随配置更新重写。新配置只影响更新完成后新提交
的 message。

### 6.4 Delete

默认只允许删除空 Queue：

1. scheduler read transaction 验证 exact generation 并读取 backlog；`force=false` 且非空时直接返回
   `QUEUE_NOT_EMPTY`，不改变 lifecycle；
2. control transaction 验证 ready/healthy、expected generation、`queue_referrers` 仍为空，然后
   `ready -> deleting`；这一步同时阻止新的 producer/consumer/DLQ referrer；
3. scheduler transaction 把 queue_state `accepting/configuring -> deleting`，阻止任何 stale runtime
   send；
4. `force=true` 时 bounded purge messages，直到 counters 为零；非 force 的 Queue 已由第 1 步证明为空，
   且零 referrer 意味着期间没有合法 producer/consumer 可重新增加 backlog；
5. 删除 scheduler queue_state；
6. control transaction `deleting -> tombstoned` 并完成 idempotency；
7. reconciler 继续任何 crash 中断的 fence/purge/tombstone。

delete 一旦在 control 进入 `deleting` 就不能被新 deployment 绑定。旧 retained deployment 已由
referrer 阻止进入 deleting，因此不存在“删除 Queue 但旧 active binding 继续发”的合法路径。

`force` 只允许清空无 referrer Queue 的 backlog，并在 response/audit 中明确 `purgedMessages` 和
`purgedBytes`；不保证可恢复。物理删除是 material destructive action，control API/runbook 必须要求
operator 明确传 `force=true`。

### 6.5 同名重建

tombstoned Queue name 可以重用，但新 Queue 获得新 UUID。旧 binding frozen Queue ID 永远不会自动
指向新 Queue；如果 retained deployment 仍存在，它本应作为 referrer 阻止旧 Queue tombstone。

## 7. Deployment、descriptor 与 RuntimeSource

### 7.1 Control input

沿用 P0.3 deployment payload：

```json
{
  "bindings": {
    "EVENTS": {
      "type": "queue_producer",
      "id": "019..."
    }
  }
}
```

API 接受 Queue ID，不按 name 绑定。部署 staging transaction：

1. canonicalize vars、secrets、resource bindings 和 Queue bindings 的 name 全集；
2. 同 account 查询 ready/healthy Queue；
3. 冻结 lifecycle generation 与 capability version；
4. 插入 immutable `queue_producer_bindings` + referrer；
5. 构造 canonical descriptor/hash；
6. descriptor 加入 deployment `worker_code_sha256`；
7. commit 后再进入现有 artifact/runtime validation。

validation scope 不注入真实 Queue，防止部署验证意外写消息。

### 7.2 descriptor

```json
{
  "schemaVersion": 1,
  "bindingId": "019...",
  "name": "EVENTS",
  "kind": "queue_producer",
  "queueId": "019...",
  "queueLifecycleGeneration": 1,
  "capabilityVersion": 1
}
```

不含 display name、message limits、delivery delay、retention、account ID、DB path 或 internal token。
动态配置由 backend 通过 queue_state generation 验证，避免旧 isolate 缓存 stale config。

### 7.3 RuntimeSource merge

RuntimeSource 在同一 control read snapshot 中读取：

- `deployment_bindings`；
- `queue_producer_bindings`；
- deployment vars/secrets。

按 binding name bytes 排序并再次检查全集唯一。任何：

- duplicate env name；
- descriptor hash mismatch；
- missing referrer；
- deployment state/generation mismatch；
- unknown Queue capability；
- Queue lifecycle generation mismatch；

都返回 `DEPLOYMENT_INVARIANT_VIOLATION`，warm isolate 也不能继续使用。

`BindingKind` 可以增加 `QueueProducer` 作为 runtime descriptor enum，但 generic `ResourceDriver`、
`resources` repository 和 resource lifecycle match 不得因此把 Queue 当普通 P0 resource。代码中要把
“runtime binding kind”和“generic resource driver kind”的分支写清楚。

## 8. Facade、transport 与内部协议

### 8.1 loader host

参考 WDL 的薄 facade 模式：

```js
function makeBinding(ctx, descriptor) {
  const capability = descriptor.kind + "@" + descriptor.capabilityVersion;
  switch (capability) {
    case "queue_producer@1":
      return ctx.exports.QueueTransport({
        props: trustedQueueProps(descriptor),
      });
  }
}
```

loaded-isolate wrapper 为 tenant 注入 `QueueProducer` facade，使 default Worker、named
`WorkerEntrypoint` 和通过 Gate 的 DO class 看到标准 method shape。tenant 不能访问 `QueueTransport`
props 或 internal fetch。

### 8.2 分层校验

Facade：

- JS argument shape；
- `undefined` body；
- content type 与 body type；
- JSON serialization；
- generic iterable 的有界消费；
- per-message/batch byte limit；
- delay integer/range；
- 构造 binary request frame。

Rust backend：

- internal authentication/channel；
- binding ID、active deployment、Queue ID 和 generation；
- Queue `ready/healthy` 与 queue_state exact projection；
- content type、body length、message count、batch total、delay；
- P1 disk admission、Queue backlog quota；
- SQLite transaction、ID、timestamps、counters；
- stable structured response。

两层检查不是重复浪费：facade 提供接近 Cloudflare 的 TypeError，backend 防止 compromised/malformed
runtime 绕过。

### 8.3 private backend

使用 fixed method surface，不允许 tenant 传 path ID：

```text
QueueTransport.send(frame)
QueueTransport.sendBatch(frame)
QueueTransport.metrics()
```

如果现有 loader transport 只能通过 internal HTTP，固定为：

```text
POST /internal/bindings/v1/queue/{binding-id}/send
POST /internal/bindings/v1/queue/{binding-id}/batch
GET  /internal/bindings/v1/queue/{binding-id}/metrics
```

但 path 中 binding ID 只能来自 trusted props，不能由 tenant method argument构造。internal generation
fence/auth token 不进入 tenant env、exception 或 log。

frame 必须有：

- magic + protocol version；
- operation enum；
- fixed-width count/length；
- content type enum；
- delay；
- raw body bytes；
- bounded total frame length。

避免 base64 JSON 把 256 KB batch 放大并产生多份内存 copy。未知 version、truncated/trailing bytes、
count/length overflow 一律在 SQLite transaction 前拒绝。

## 9. Serialization

### 9.1 默认 JSON

当前 platform compatibility date 已晚于 `queues_json_messages` 的 2024-03-18 切换点，capability V1
默认 JSON。对旧 compatibility date 不回退 V8；本地平台在 capabilities/deviations 中明确：

```text
Queue producer default is JSON for all supported platform compatibility dates.
V8 queue messages are unsupported in capability v1.
```

这样只有一条可验证的数据格式，不会让旧 deployment 写入未来无法 decode 的伪 V8。

### 9.2 JSON

- body `undefined` 拒绝；
- 使用当前 isolate 的 JSON serialization；
- cycle、BigInt 或其他 JSON 不支持值按 runtime TypeError 拒绝；
- body bytes 是序列化 JSON 的 UTF-8 bytes；
- size 在序列化后检查；
- backend 不 parse/re-stringify，只保存 facade提交的 bytes；
- P2.3 consumer 再用 JSON parser materialize body。

### 9.3 text

- body 必须是 JS string；
- 存 UTF-8 bytes；
- size 是 UTF-8 byte length，不是 UTF-16 code units；
- backend 不做 Unicode normalization。

### 9.4 bytes

- body 必须是 ArrayBufferView，与 pinned workerd source一致；
- facade 在跨异步边界前 copy 出 owned bytes；
- detachable/resizable buffer 的 observable detail纳入 Hard Gate；
- message commit 后修改原 buffer 不影响已持久化 body。

### 9.5 V8

返回 stable TypeError/code：

```text
QUEUE_CONTENT_TYPE_UNSUPPORTED:
v8 contentType is not supported; use json, text, or bytes
```

不能用 Node `v8.serialize`、JSON、`structuredClone` 或自定义 CBOR 冒充 workerd V8 wire format。

### 9.6 `sendBatch` iterable

官方 shape 是 `Iterable<MessageSendRequest>`。实现必须：

1. 调用 `Symbol.iterator`；
2. 逐项验证 request object 与 own `body`；
3. 第 101 项立即停止并拒绝；
4. 累计序列化 body 超 256,000 bytes 立即拒绝；
5. 异常时按 JS iterator closing 语义调用 `return()`；
6. 空 iterable 拒绝；
7. 不先 `Array.from()` 无界消费 generator；
8. 所有 item 验证完成后才调用 backend，因此 validation error 不产生 partial batch。

## 10. Producer transaction 与 durability

### 10.1 `send`

backend 在一次 scheduler store blocking task 中：

```text
BEGIN IMMEDIATE
  read queue_state by trusted queue_id
  verify accepting + lifecycle/config generation
  verify limits/backlog quota
  reserve P1 disk admission budget
  generate UUIDv7 message ID
  compute enqueued/available/expires timestamps with checked arithmetic
  INSERT queue_messages(state=ready)
  trigger increments counters
  read transaction-local metrics
COMMIT (WAL + synchronous=FULL)
release admission reservation
return ID-internal result + public metrics
```

public response 不暴露 message ID，因为 Cloudflare producer response 也不以 message ID 作为 API contract。
host log 可以在 debug trace 中使用 hashed/correlated internal ID，但不记录 body。

### 10.2 `sendBatch`

整个 batch 一次 `BEGIN IMMEDIATE`：

- 在 transaction 前完成 JS serialization 与 frame validation；
- transaction 内重新验证所有 limits；
- 读取 queue_state 一次；
- 为每项生成 UUIDv7；
- 按 input order insert，因此获得严格递增 `seq`；
- 任一 insert/counter/quota failure rollback 全 batch；
- commit 后一次返回 metrics。

SQLite transaction 不能跨 await，也不能在 transaction 内调用 workerd、S3 或 control DB。

### 10.3 resolve 与 unknown result

`send()/sendBatch()` resolve 表示：

> message 已写入本机 `scheduler.sqlite` durable transaction。

它不表示 consumer 已读取或处理。若 transaction commit 后 process/transport 在 response 前崩溃：

- caller 收到 reject/connection loss；
- message 可能已经存在；
- caller 重试可能产生 duplicate；
- binding API 没有 idempotency key，平台不得自动重放或去重。

stable host category 为 `QUEUE_SEND_RESULT_UNKNOWN`，但 facade只能在 transport 能区分时暴露通用 Queue
error；不能把 unknown误报成确定未写。

### 10.4 ordering

P2.2 保证：

- 同一成功 batch 的 rows 按输入顺序拥有递增 seq；
- 同 Queue 同一 SQLite writer commit 顺序可观察为 enqueue seq；
- delay 可能让后发送消息更早 available；
- 不承诺 strict FIFO、consumer exactly-once 或跨 Queue ordering。

## 11. P2.2.0：stock-workerd / output-gate Hard Gate

native workerd `WorkerQueue::send` 和 `sendBatch` 会调用
`IoContext::waitForOutputLocksIfNecessary()`：在 Durable Object 中，Queue write 要等待 storage output
gate 打开，避免 storage transaction 尚未确认时消息先发出。

当前平台通过 `workerLoader` + service entrypoint facade 动态注入 binding，不能假设普通 service RPC
自动继承 native Queue output-lock 行为。实现前必须在 pinned stock workerd 上验证。

### 11.1 Gate 矩阵

| Gate | 断言 |
| --- | --- |
| QG-01 | default Worker 的 facade `send/sendBatch/metrics` shape、error与 promise lifecycle 可用 |
| QG-02 | named `WorkerEntrypoint` cold/warm/RPC 路径看到同一 Queue binding |
| QG-03 | loaded-isolate generic iterable、Date metrics、bytes deep-copy 可精确实现 |
| QG-04 | ordinary Worker send 只有 backend durable commit 后 resolve |
| QG-05 | DO storage write commit 后 Queue send 才到 backend |
| QG-06 | DO storage transaction rollback/handler throw 时未打开 output gate 的 send 不 enqueue |
| QG-07 | output gate 打开后、response 丢失时结果按 unknown 处理，不自动 replay |
| QG-08 | DO restart/cold activation 不复用 stale Queue transport |
| QG-09 | tenant 无法调用 internal transport或伪造 binding/generation |
| QG-10 | facade/transport source hash、capability 和 compatibility policy进入 descriptor/release identity |

### 11.2 verdict

```text
Go:
  ordinary Worker 与 DO 均满足 durable commit 和 output-gate Gate。

Conditional Go:
  ordinary Worker 满足 durable commit，但 service facade 无法继承 DO output gate。
  P2.2 只给普通 Worker/WorkerEntrypoint 注入 Queue；
  DO class 中调用同名 binding 必须 fail closed：
  QUEUE_DO_OUTPUT_GATE_UNSUPPORTED。

No-Go:
  ordinary Worker 也无法建立 trusted facade、bounded frame 或 commit-before-resolve。
```

Conditional Go 不是“DO 里先凑合用”。capabilities、TypeScript/dev tooling、runtime wrapper 和测试都必须
明确禁止 DO producer；不能允许消息在 DO storage rollback 前已经提交。后续只有新的 Gate/能力版本
能解除限制。

Hard Gate fixture 放在 `poc/p2-2-queue-gate` 或等价现有结构，继续使用 pinned stock workerd，不 fork。

## 12. Metrics API、retention 与 P2.1 workload

### 12.1 `metrics()`

一个只读 scheduler transaction：

1. trusted binding authorize；
2. exact lifecycle/config projection check；
3. 读取 queue_state counters；
4. 用 `queue_messages_oldest` index 找最早 `enqueued_at_ms`；
5. 返回 count、bytes、`Date | undefined`。

`backlogCount` 包括 delayed messages；P2.3 后也包括 ready/claimed/retry-wait 等未最终删除的 message。
`backlogBytes` 是 stored body bytes，不含 SQLite overhead。

metrics 不是强一致 consumer lag SLA；它只是在同一 SQLite snapshot 中的 durable backlog summary。

### 12.2 retention sweep

P2.2 已注册一个 Queue maintenance workload 到 P2.1，但不注册 consumer dispatch：

```text
next_due = MIN(queue_messages.expires_at_ms)
claim/sweep = bounded DELETE ... RETURNING by expires_at_ms, queue_id, seq
complete = same SQLite transaction; triggers update counters
```

实现可用“bounded retention maintenance adapter”，不把每条 message 当成独立 scheduler claim。要求：

- 每批有 row/byte/time budget；
- 删除 transaction 短；
- backlog 仍有 expired rows 时立即 re-notify；
- 没有 expired row 时等待 earliest expiry；
- Queue pool admission 与 Alarm pool隔离；
- retention error 只让 Queue maintenance degraded，不阻断 Alarm；
- P1 disk hard reserve 下 delete/GC 仍可使用 emergency reserve。

P2.1 的 Queue pool在 P2.2 capability release 时启用，但只执行 retention/config/delete reconcile；
P2.3 才增加 consumer message claim/dispatch。

### 12.3 no-consumer 行为

没有 consumer 时：

- send 正常成功；
- message 计入 backlog；
- 到 `available_at` 也不会被 dispatch；
- 到 `expires_at` 被 retention sweep 删除；
- Queue health 不因“没有 consumer”降级；
- operator summary可以显示 consumerConfigured=false，但不列 message body。

## 13. Isolation、quota 与安全

### 13.1 authorization

每次 backend call 都从 immutable binding解析：

```text
active deployment
-> binding ID
-> queue ID + lifecycle generation
-> owning Worker account
-> queues account
-> scheduler queue_state generation
```

任一步 mismatch 统一返回 scoped not-found/invariant error，不能向 tenant泄露“另一个 account 的 Queue
存在”。tenant body/options 永不参与 Queue lookup。

### 13.2 quota/admission

写入必须同时通过：

- per-message/batch API limit；
- Queue `max_backlog_bytes`；
- account Queue count limit；
- P1 host disk soft/hard reserve；
- backend frame/body in-flight memory budget；
- blocking SQLite task/connection pool budget；
- Queue producer per-binding request concurrency。

quota check 与 insert/counter 在同一 scheduler transaction，防止并发 producer 同时越过 backlog limit。
P1 disk reservation 先保守预留，commit/rollback 后结算；不得用 `body_bytes` 等同实际 SQLite 增量做
过度精确承诺。

### 13.3 body hygiene

- body、JSON、text、bytes 不进入 application log、audit detail、metrics label、health、doctor 或 support
  bundle；
- error 只含 index、limit、content type 等非 payload 信息；
- internal frame buffer 用后及时释放；
- support bundle 只列 Queue ID、state/generation、counts/bytes 和 stable code；
- SQLite snapshot 本身包含明文 message body；按既有 P1 决策，备份不提供保密性，只保证完整性与恢复。

### 13.4 malicious iterable/frame

测试：

- infinite generator；
- iterator getter throw、`next()` throw、`return()` throw；
- 101st message；
- length/count integer overflow；
- truncated/trailing frame；
- forged binding ID/generation；
- decompression 不存在，禁止 zip bomb；
- JSON cycle/BigInt；
- huge string/typed array；
- resizable/detached ArrayBuffer；
- concurrent config/delete/send race。

## 14. Error model

建议稳定类别：

| Code | 类别 | Result | 说明 |
| --- | --- | --- | --- |
| `QUEUE_NOT_FOUND` | not found | definite | 不存在或不属于 scope |
| `QUEUE_NAME_CONFLICT` | conflict | definite | live name 重复 |
| `QUEUE_NOT_READY` | conflict | definite | creating/deleting/tombstoned |
| `QUEUE_CONFIG_PENDING` | unavailable | definite no-write | scheduler projection 未收敛 |
| `QUEUE_REFERENCED` | conflict | definite | producer/consumer/DLQ referrer存在 |
| `QUEUE_NOT_EMPTY` | conflict | definite | non-force delete 有 backlog |
| `QUEUE_CONTENT_TYPE_UNSUPPORTED` | TypeError | definite no-write | V8/unknown content type |
| `QUEUE_INVALID_MESSAGE` | TypeError | definite no-write | body/type/JSON/iterable invalid |
| `QUEUE_MESSAGE_TOO_LARGE` | TypeError | definite no-write | 单消息超 128,000 bytes |
| `QUEUE_BATCH_LIMIT_EXCEEDED` | TypeError | definite no-write | count/total bytes 超限 |
| `QUEUE_DELAY_INVALID` | TypeError | definite no-write | 非整数或超 0..86,400 |
| `QUEUE_BACKLOG_LIMIT_EXCEEDED` | limit | definite no-write | Queue local quota |
| `QUEUE_STORAGE_UNAVAILABLE` | unavailable | usually no-write | DB unavailable/busy beyond budget |
| `QUEUE_SEND_RESULT_UNKNOWN` | unavailable | unknown | commit 可能成功、response 丢失 |
| `QUEUE_DO_OUTPUT_GATE_UNSUPPORTED` | unsupported | definite no-write | Conditional Go 下 DO producer禁用 |
| `QUEUE_INVARIANT_VIOLATION` | internal | fail closed | catalog/projection/counter/generation损坏 |

tenant exception 不包含 SQL、path、body、account/Queue ID 或 internal cause。host structured log 走 P1
redaction，只在 authorization 完成后记录 scoped Queue ID。

## 15. Observability 与 operator surface

### 15.1 low-cardinality metrics

```text
queue_producer_requests_total{operation,outcome}
queue_producer_duration_seconds{operation}
queue_producer_messages_total{operation,outcome}
queue_producer_body_bytes_total{operation,outcome}
queue_backlog_messages
queue_backlog_bytes
queue_retention_deleted_total{outcome}
queue_retention_deleted_bytes_total{outcome}
queue_reconcile_total{operation,outcome}
queue_projection_lag_seconds
queue_result_unknown_total{operation}
```

聚合 gauge 是平台/account-safe aggregate；若 metrics endpoint 是 operator scoped，可按固定
`availability` 聚合。禁止 Queue ID/name、account、deployment、binding、message ID label。

### 15.2 control API

```text
POST   /v1/accounts/{account_id}/queues
GET    /v1/accounts/{account_id}/queues
GET    /v1/accounts/{account_id}/queues/{queue_id}
PATCH  /v1/accounts/{account_id}/queues/{queue_id}
DELETE /v1/accounts/{account_id}/queues/{queue_id}?force=false
```

mutation 需要 request ID、authorization 和 idempotency key；PATCH 需要 expected config generation。
list 使用 opaque cursor、稳定 `created_at_ms,id` 排序和 bounded limit。响应不包含 backlog message
内容；Queue get 可返回 summary metrics，但控制面不可用不能成为 producer hot path。

P2.2 不提供：

```text
POST /queues/{id}/messages
GET  /queues/{id}/messages
POST /queues/{id}/pull
```

tenant 写入只走 bound Worker API。

### 15.3 operator

P2.1 scheduler inspect 增加 Queue pool：

```json
{
  "kind": "queue",
  "enabled": true,
  "mode": "producer_retention_only",
  "state": "ready",
  "readyMaintenance": 0,
  "inFlight": 0,
  "oldestDueAt": null
}
```

Queue catalog health/doctor列：

- live/creating/deleting/tombstoned counts；
- total messages/body bytes；
- oldest enqueue/expiry age；
- config projection mismatch count；
- counter invariant mismatch count；
- referrer invariant；
- retention lag；
- no message body。

## 16. Snapshot、restore、upgrade 与 cleanup

### 16.1 snapshot

P1 offline snapshot 已包含完整 `scheduler.sqlite`，因此自动包含：

- queue_state；
- queue_messages body；
- counters、delay、expiry、future claim columns。

`control.sqlite` 同一整机 snapshot 包含 Queue catalog/bindings/referrers。restore 到 fresh data-dir 后：

1. release/schema/master-key/S3 preflight；
2. 恢复 control + scheduler SQLite；
3. quick_check/checksum；
4. Queue cross-DB generation/counter invariant；
5. 先启动 reconciler/retention，再开放 producer traffic。

备份对象不加密、不承诺保密性；沿用 P1 已确认的明文 snapshot 决策。

### 16.2 upgrade

- control schema 8 -> 9、scheduler 1 -> 2 是 forward-only；
- upgrade 前必须有已验证 P1 snapshot；
- migration 不读取/改写 tenant body；
- rollback binary 只能恢复升级前 snapshot，不能直接读新 schema；
- old deployment 无 Queue binding，升级后保持不变；
- P2.2 deployment descriptor/capability 只有新 binary 能创建。

### 16.3 cleanup

- retention、force delete 和 tombstone cleanup 使用 bounded batches；
- `VACUUM` 不在 request path 自动运行；
- WAL checkpoint 复用 P1 policy；
- deleted bytes 不等于文件立刻缩小，capacity docs分别报告 logical backlog 和 physical DB bytes；
- emergency reserve 允许 delete/checkpoint/doctor 恢复空间。

## 17. 实现工作包

### P2.2.0：stock-workerd Hard Gate

- default/named entrypoint facade；
- generic iterable、Date metrics、bytes buffer 行为；
- commit-before-resolve；
- DO output gate commit/rollback/throw/restart；
- trusted internal transport；
- 给出 Go/Conditional Go/No-Go，不隐藏偏差。

### P2.2.1：schema 与 repositories

- `009_queues.sql`；
- `002_queue_producer.sql`；
- Queue/Message ID newtypes；
- catalog、binding、referrer、scheduler repositories；
- trigger/property/migration/old-data-dir tests；
- counter doctor/repair。

### P2.2.2：lifecycle/reconciler

- create/rename/config/delete；
- cross-DB generation protocol；
- idempotency/audit；
- crash after every control/scheduler transaction；
- startup bounded reconcile。

### P2.2.3：deployment/runtime descriptor

- `queue_producer` control input；
- env-name全集 conflict；
- immutable binding/referrer；
- RuntimeSource merge/hash/warm validation；
- active/retained/rollback/deletion行为。

### P2.2.4：facade/transport/serialization

- Queue facade与 private transport；
- JSON/text/bytes；
- generic bounded iterable；
- binary frame；
- stable TypeError/error mapping；
- V8 fail closed。

### P2.2.5：producer transaction

- send/sendBatch SQLite commit；
- UUIDv7、timestamps、delay precedence、expiry；
- message/batch/backlog/disk limits；
- metrics response；
- result-unknown 与 restart persistence。

### P2.2.6：retention/scheduler

- P2.1 Queue maintenance adapter；
- earliest expiry wake；
- bounded sweep；
- pool fairness、pause/resume/drain/circuit；
- no-consumer behavior。

### P2.2.7：ops/release Gate

- control/operator API；
- metrics/health/doctor/support bundle/capabilities/deviations；
- snapshot/restore/upgrade rehearsal；
- security/fuzz/crash/soak；
- 三轮 P2.2 aggregate + P2.1/P1/P0/G0 regression。

## 18. 测试矩阵

### 18.1 API/serialization

- send default JSON、explicit JSON、text、bytes；
- undefined、cycle、BigInt、wrong text/bytes type；
- empty body和恰好 128,000/128,001 bytes；
- delay absent/0/1/86,400/negative/fraction/86,401/overflow；
- Queue default、batch、per-message precedence；
- empty/single/100/101 item iterable；
- generator、Set、自定义 iterator、throw/return；
- total 256,000/256,001 bytes；
- V8/unknown content type；
- metrics Date/undefined 与 response shape；
- buffer mutate/detach/resize after call。

### 18.2 transaction/durability

| case | crash/failure | 断言 |
| --- | --- | --- |
| T-01 | insert 前 | 无 message/counter变化 |
| T-02 | batch 第 N row 后 transaction abort | 整 batch 不可见 |
| T-03 | counter trigger 后 commit 前 SIGKILL | row/counter 一起 rollback |
| T-04 | commit 后 response 前 SIGKILL | message 存在；caller result unknown |
| T-05 | restart | message/body/delay/expiry/metrics 保持 |
| T-06 | concurrent producers near quota | 不超 backlog limit |
| T-07 | config projection pending | send definite no-write |
| T-08 | delete fence 与 send race | 只有一个 transaction顺序获胜 |
| T-09 | retention 与 send 同时运行 | counters/invariants精确 |
| T-10 | snapshot/restore | backlog 与 metrics一致 |

### 18.3 lifecycle

- create 四 crash boundaries；
- rename 与同名 conflict；
- config update 三 crash boundaries、stale generation；
- delete empty/non-empty/force/referrer；
- delete purge 每批 crash/restart；
- tombstone 同名重建；
- retained deployment、rollback、deployment GC；
- cross-account Queue/binding ID probing；
- orphan binding/referrer/projection；
- control/scheduler future schema/checksum。

### 18.4 scheduler/fairness

- Queue retention backlog 不饿死 Alarm；
- Alarm 无限 due 不阻止 Queue expiry bounded progress；
- Queue pool pause只阻止 maintenance，不阻止 control config durability；
- circuit-open Queue pool不阻止 Alarm；
- virtual clock推进 24h delay和 14d retention；
- wall clock前跳/后跳；
- lost wake；
- shutdown/hung retention；
- cold first event 是 expiry maintenance，不需普通 fetch预热。

### 18.5 output gate

- ordinary Worker send、sendBatch；
- DO outside transaction storage write + send；
- DO transaction commit + send；
- DO transaction rollback + send；
- handler throw；
- blockConcurrencyWhile；
- cold/warm/restart；
- response loss；
- Conditional Go 时每条 DO path 都稳定 fail closed且 zero enqueue。

### 18.6 security/fuzz

- internal frame parser corpus/property fuzz；
- loader descriptor mutation/hash/generation；
- binding env collision；
- account/deployment/Queue isolation；
- malicious iterable和巨大 input memory bound；
- log/error/audit/support bundle payload redaction；
- production binary 不包含 Gate/fault endpoint；
- SQLite malformed/corrupt/counter mismatch fail closed。

## 19. Exit Gate

P2.2 只有全部满足才可进入 Queue consumer：

- [x] P2.2.0 对 pinned stock workerd 给出书面 Go/Conditional Go/No-Go；
- [x] Conditional Go 时 DO producer明确 fail closed，capabilities/deviations/test一致；
- [x] control migration 009 不关闭 FK、不重建 P0 `resources`/`deployment_bindings`；
- [x] scheduler migration 002 连续、checksummed、old P1 data-dir 可升级；
- [x] Queue create/rename/config/delete/referrer/idempotency 全 crash matrix通过；
- [x] RuntimeSource cold/warm/rollback 使用 immutable Queue descriptor；
- [x] default JSON、text、bytes、iterable和 TypeError shape通过 conformance；
- [x] V8 明确拒绝，没有近似 serializer；
- [x] 128,000/100/256,000/86,400 exact boundary通过；
- [x] delay precedence与显式 0 通过；
- [x] send/sendBatch commit-before-resolve；batch全成或全败；
- [x] commit 后 response loss被分类为 unknown，不自动 replay；
- [x] restart、process SIGKILL、snapshot/restore保留 message/counters；
- [x] metrics backlog count/bytes/oldest Date 与 SQLite snapshot一致；
- [x] no-consumer backlog按 retention bounded删除；
- [x] Queue maintenance不饿死 Alarm，Alarm 不饿死 Queue maintenance；
- [x] quota/disk admission/concurrent producer不超限；
- [x] cross-account/binding/generation/forged frame全部 fail closed；
- [x] message body不进入 log、metric、audit、health、doctor或 support bundle；
- [x] production binary 不包含 test crash path；
- [x] P2.1 Gate连续三轮通过；
- [x] P1 aggregate、P0 aggregate与 `./poc/g0 test all` regression通过；
- [x] format、Clippy、unit/integration、MSRV、no-default-features、dependency boundary、
      `git diff --check` 与 coverage Gate 通过。

建议新增入口：

```bash
./scripts/test-p2-2.sh
```

保持当前本地-only验证方式，不增加 CI、Codecov、远端上传或自动部署。

## 20. 明确偏差

P2.2 capability/deviations 至少记录：

1. 单节点 SQLite authority，不提供 Cloudflare global delivery/replication；
2. producer durable commit 是本机 `scheduler.sqlite` commit；
3. V8 Queue content type 不支持；
4. 所有支持的 compatibility date 默认 JSON；
5. Queue metadata 不支持；
6. 没有 consumer/DLQ/Cron；
7. producer response 丢失后重试可能重复；
8. local backlog/disk quota不是 Cloudflare plan limit；
9. Queue snapshot是整机 offline snapshot的一部分，不提供资源级 PITR；
10. 若 Hard Gate 为 Conditional Go，Durable Object 中 Queue producer 不支持。

这些偏差进入 `platformd capabilities --json`、P1 conformance manifest、operator docs 和测试 fixture，
不能只写在 README。

## 21. 参考实现与资料

实现前固定核对：

- Cloudflare Queues JavaScript APIs：
  <https://developers.cloudflare.com/queues/configuration/javascript-apis/>
- Cloudflare Queues batching/retries/delay precedence：
  <https://developers.cloudflare.com/queues/configuration/batching-retries/>
- Cloudflare Queues configure/default delay/retention：
  <https://developers.cloudflare.com/queues/configuration/configure-queues/>
- Cloudflare Queues limits：
  <https://developers.cloudflare.com/queues/platform/limits/>
- pinned workerd producer API：
  `references/workerd/src/workerd/api/queue.h`、
  `references/workerd/src/workerd/api/queue.c++`
- WDL facade/reference：
  `references/wdl/runtime/bindings/queue.js`、
  `references/wdl/docs/modules/queues-cron.zh.md`
- Miniflare API/broker fixture：
  `references/workers-sdk/packages/miniflare/src/plugins/queues/index.ts`、
  `references/workers-sdk/packages/miniflare/src/workers/queues/broker.worker.ts`

使用原则：

- Cloudflare 文档决定常用公开 API/limit；
- pinned workerd source决定实际 JS type、serialization和 output-gate行为；
- WDL 决定 loaded-isolate薄 facade的可参考结构，但不照搬 Redis authority；
- Miniflare 用于 conformance fixture和开发体验参考，不把 in-memory broker当生产 durability设计；
- 本项目的 P0/P1 invariants决定 SQLite、deployment、snapshot、安全和运维边界。

## 22. 完成定义

P2.2 完成时，一个普通 Worker 能从 immutable Queue binding：

1. 按 JSON/text/bytes发送一条或一批 bounded message；
2. 使用 Queue/batch/message delay；
3. 在 promise resolve 后确认消息已经 durable commit到 `scheduler.sqlite`；
4. 重启、SIGKILL和整机 snapshot/restore后仍读取到同一 backlog metrics；
5. 在没有 consumer时由 retention有界清理；
6. 在跨 account、stale deployment、config pending、delete和磁盘不足时 fail closed；
7. 不影响 Alarm scheduler公平进展；
8. 清楚知道哪些行为与 Cloudflare不同，尤其是 V8、exactly-once和 DO output gate。

P2.2 不以“消息能插入数据库”为验收终点。只有 lifecycle、runtime binding、JS API、durable transaction、
unknown-result、retention、isolation、snapshot 与 crash recovery同时有证据，才有资格在 P2.3 上面增加
at-least-once consumer。
