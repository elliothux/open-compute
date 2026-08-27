# P2.3：Queue Consumer 与 Cron 详细设计

> 状态：待实现
>
> 前置基线：P2.1、P2.2 已由用户确认在当前 checkout 跑通；P2.2 的本地结论仍是
> Conditional Go——普通 Worker 与 named `WorkerEntrypoint` producer 可用，Durable Object
> producer 因 output gate 无法等价而 fail closed。P2.3 不扩大这一接受范围。
>
> 直接依赖：[P2.1：Scheduler 多 Workload 内核](./p2-1-scheduler-hardening.md)、
> [P2.2：Queue Producer](./p2-2-queue-producer.md)、
> [P2.2 本地验证结果](./p2-2-results.md)、
> [P0.2：Workers Runtime](./p0-2-workers-runtime.md)、
> [P1：平台加固](./p1-platform-hardening.md)
>
> 后续消费者：[P2.4：Workflow Core](./p2-4-workflow-core.md)。P2.4 只有在 Queue 的
> claim、lease、dispatch、ack/retry 和 crash recovery 均通过 Gate 后才开始。

P2.3 分成两个有依赖关系、可单独回滚的交付单元：先实现 Queue push consumer，再实现 Cron。
两者共享 P2.1 scheduler kernel 与 `workerLoader`，但不共享业务表或完成协议。Queue 的 authority
是一组 message row；Cron 的 authority 是 schedule projection 与唯一 logical slot。不能为了复用
`due_at` 而把它们压成同一张通用 jobs 表。

设计目标不是复制 Cloudflare 的全球吞吐、autoscaling 或多地域语义，而是在单进程、单 SQLite
authority 下复刻常用 Worker API，并把 at-least-once、冻结版本、故障恢复和运维边界说清楚。

## 0. 决策摘要

| 主题 | P2.3 选择 | 原因 |
| --- | --- | --- |
| 实现顺序 | P2.3A Queue Consumer，P2.3B Cron | Cron 可复用 Queue 已验证的 custom-event dispatch、lease 与 outcome 框架 |
| Queue consumer 数量 | 每个 Queue 最多一个 active push consumer | 对齐当前范围，避免先引入多订阅 fan-out 语义 |
| Consumer 声明 | immutable deployment declaration + live attachment | 配置随 deployment 冻结，运行时又能 pause/update/drain |
| Queue message authority | 延续 `scheduler.sqlite.queue_messages` | claim、ack/retry、DLQ 都需与 message 在同一 transaction |
| Delivery | at-least-once，不承诺 exactly-once 或严格顺序 | crash-after-handler-before-ack 必然可能重复 |
| Batch | size 或 timeout 先满足者触发 | 对齐常用 Cloudflare Queue 行为 |
| Ack/retry | 使用 pinned workerd 原生 `MessageBatch`/`Message` 解析；host 再做集合校验 | 复用 first-call-wins 语义，同时不信任 tenant 返回的 message ID |
| Attempt | 只对已知 handler outcome 计 product attempt；未知 dispatch 不消耗 `max_retries` | 平台崩溃不能在从未确认投递时把消息耗尽 |
| DLQ | source terminal decision 与 DLQ intake 同一 scheduler transaction | 不在两个 SQLite 或异步 HTTP 之间制造丢消息窗口 |
| Concurrency | per-consumer cap + P2.1 Queue pool cap + global cap | 防止单 Queue 或单 workload 吃满平台 |
| Consumer 更新 | stop-claim、drain、generation switch、resume | 旧 deployment 和新 deployment 不并发消费同一 Queue |
| Cron 声明 | deployment 配置支持 `inherit`/`replace`，promotion 生成 live activation | 复刻 omitted 保留、空数组删除的常用部署语义 |
| Cron 时区 | UTC-only、五字段、Cloudflare 文档列出的 Quartz-like 扩展 | 避免本地时区/DST 产生不可移植结果 |
| Cron slot | `(activation_id, generation, scheduled_at_ms)` 唯一 | crash/reconcile 不重复创建同一 logical slot |
| Cron misfire | 不回放完整停机历史；仅在 bounded grace 内生成最近一个 due slot | 适合 SMB 单机，避免恢复后触发 cron storm |
| Cron retry | 固定本地 bounded retry，`controller.noRetry()` 可关闭 | Cloudflare 未公开内部调度细节，不伪装完全一致 |
| SQLite migrations | control 010/011；scheduler 003/004 | Queue 与 Cron 各自可验证、可定位、可 forward-only 升级 |

### 0.1 必须守住的不变量

1. 一个 Queue 同时最多一个 `active` 或 `updating` push consumer。
2. Consumer declaration 只能引用同 account、ready deployment、ready/healthy Queue 和精确 lifecycle generation。
3. Active consumer 的 target deployment、execution generation、consumer generation 在 claim transaction 内冻结。
4. Consumer update 先停止旧 generation 新 claim，再 drain；新旧 target 不重叠消费。
5. Queue batch claim 在短 `BEGIN IMMEDIATE` transaction 中完成；tenant handler 期间不持 SQLite lock。
6. 每个 completion 必须同时匹配 batch ID、32-byte token、consumer ID/generation 和 message membership。
7. 旧 handler、过期 lease、旧 consumer generation 或旧 workerd generation 的完成只能是 no-op。
8. `ack()`/`retry()` 的单消息 first-call-wins 和单消息优先于 batch decision；host 不重新解释冲突顺序。
9. Handler 成功且无显式 decision 的消息 ack；handler 失败且无显式 decision 的消息 retry。
10. Unknown dispatch outcome 不立即释放 claim；lease recovery 后重投，且不消耗 product `max_retries`。
11. 达到 retry 上限时，source message 必须在一个 transaction 中进入 DLQ intake 或被明确 discard。
12. DLQ 是另一个 Queue，不能是自己，必须同 account、ready/healthy 且 generation 精确。
13. Pause 只停止新 claim；已经 dispatch 的 batch 由 token-exact completion 或 lease recovery 收敛。
14. Cron expression 在 deployment validation 时解析；运行时不接受未验证字符串。
15. Cron 使用 UTC；`controller.cron` 保留用户声明的精确字符串，`scheduledTime` 使用 logical slot 时间。
16. 同一 Cron activation generation 的同一 logical slot 最多一条 durable run row。
17. Cron activation handoff 先让旧 projection stop-claim，再切 control authority，再启用新 projection。
18. Queue 与 Cron 的 frozen deployment 都注册 `deployment_referrers`，不能被 artifact GC 删除。
19. Snapshot/restore 包含 consumer、message claim、Cron schedule/run authority；restore 后 stale token 不可提交。
20. Production binary 不暴露 crash endpoint、任意 SQL、虚拟时钟或 tenant payload inspect。

### 0.2 非目标

- Queue pull consumer、HTTP pull/ack API 或多个 active consumer；
- Queue V8 body、metadata、priority、partition、content dedup 或 exactly-once；
- Cloudflare 的全球 autoscaling、250 concurrency 承诺、plan throughput 或 billing；
- 跨进程 leader election、多节点 Queue/Cron failover；
- 严格 FIFO；retry、并发与 crash 都可能改变相对顺序；
- 任意 timezone、秒字段、year 字段或用户自定义 Cron parser；
- 为停机期间每个历史 Cron slot catch up；
- Workflow cron trigger；P2.3 只 dispatch Worker `scheduled()`；
- tenant-facing Cron history dashboard；只提供 bounded operator summary；
- P2.2 的 Durable Object Queue producer 解禁；该项仍需独立 output-gate 证据。

## 1. 兼容基线与参考实现

### 1.1 Cloudflare surface

P2.3 以以下当前官方文档为外部语义基线：

- [Queue consumer 配置](https://developers.cloudflare.com/queues/configuration/configure-queues/)；
- [Batch、ack/retry 与 delay](https://developers.cloudflare.com/queues/configuration/batching-retries/)；
- [Dead Letter Queue](https://developers.cloudflare.com/queues/configuration/dead-letter-queues/)；
- [Queue limits](https://developers.cloudflare.com/queues/platform/limits/)；
- [Cron Triggers](https://developers.cloudflare.com/workers/configuration/cron-triggers/)；
- [Scheduled handler](https://developers.cloudflare.com/workers/runtime-apis/handlers/scheduled/)。

当前常用 Queue 配置基线是 `max_batch_size` 默认 10、范围 1..100；
`max_batch_timeout` 默认 5 秒、范围 0..60；`max_retries` 默认 3、最大 100；retry delay
最大 86,400 秒。P2.3 复刻这些 API 范围，但整机 concurrency、吞吐和 backlog 仍使用本项目
capability/配置，不复制 Cloudflare plan 数字。

### 1.2 pinned source 与本地参考

实现前后都以当前 lock 中的 `workerd v1.20260826.1` 为准：

- `references/workerd/src/workerd/api/queue.h`、`queue.c++`：`Message`、`MessageBatch`、
  `QueueResponse` 与 Queue custom event；
- `references/workerd/src/workerd/api/scheduled.h`、`scheduled.c++`：
  `ScheduledController` 与 scheduled custom event；
- `references/wdl/rust/scheduler/src/cron.rs` 及子模块：parser、next-run 与 scheduler 组织方式；
- `references/workers-sdk/packages/miniflare/src/plugins/queues/`：本地 API/fixture 对照。

WDL 的 Cron 实现可借鉴 parser fixture、advance-before-fire 和 handler adapter，但其 Redis/mesh
边界不进入本项目。Miniflare 适合做 JavaScript API 与本地行为对照，不作为 durability authority。

## 2. 交付顺序

```text
P2.3.0  Queue custom-event Hard Gate
   ↓
P2.3.1  Consumer control model 与 projection lifecycle
   ↓
P2.3.2  Batch eligibility、claim、lease recovery
   ↓
P2.3.3  workerd dispatch 与 ack/retry completion
   ↓
P2.3.4  DLQ、pause/update、concurrency、运维 Gate
   ↓
P2.3.5  Scheduled custom-event/Cron parser Hard Gate
   ↓
P2.3.6  Cron declaration、activation、slot/run、dispatch
   ↓
P2.3.7  聚合 crash matrix、snapshot/restore 与 Exit Gate
```

P2.3.0 至 P2.3.4 是 Queue Consumer 的独立 Exit Gate。只有该 Gate 为 Go 或被精确限定的
Conditional Go，才进入 Cron。Queue consumer 若无法通过 custom event 返回可靠 disposition，P2.3
必须停在 No-Go，不能用 HTTP fetch handler 假装 `queue()`。

## 3. P2.3.0：Queue custom-event Hard Gate

### 3.1 要证明的事实

使用与生产相同的 dynamic `workerLoader`、RuntimeSource、immutable loader key 和 private transport，
做一个最小 probe：

1. 默认 module export 的 `queue(batch, env, ctx)` 能被 native Queue custom event 调用；
2. `batch.queue`、`messages[].id/timestamp/body/attempts` 与三种 P2.2 body 类型正确；
3. JSON body 在 tenant realm 中是值，text 是 string，bytes 是隔离后的 bytes；
4. `ack()`、`retry({delaySeconds})`、`ackAll()`、`retryAll()` 返回的 native disposition 可解析；
5. 同一 message first-call-wins，单消息 decision 优先于 batch decision；
6. handler return、throw、rejected `waitUntil()`、timeout 和 workerd abort 有可区分结果；
7. warm/cold loader 对相同 immutable deployment 产生相同 handler 与 binding；
8. named `WorkerEntrypoint` 是否可作为 Queue consumer target 必须实测，不能从 fetch route 推断；
9. payload、message count、response disposition 都有 host-side 上限；
10. tenant 无法伪造 batch ID、consumer generation、claim token 或额外 message ID。

### 3.2 Gate 结论

| 结果 | 处理 |
| --- | --- |
| 全部成立 | Go，开放默认 export；named entrypoint 仅在单独 probe 通过后开放 |
| 默认 export 成立、named 不成立 | Conditional Go，默认 export 开放，named 稳定拒绝 |
| disposition 只有 handler 整体 outcome，无法表达 per-message | No-Go，不降级成错误 ack/retry 语义 |
| custom event 不能经 dynamic loader dispatch | No-Go，先修 runtime composition |
| timeout/abort 无法判定是否执行 | 可接受，但必须归类 Unknown、保留 lease，不能立即 retry |

Probe 需要保存：生成的 workerd config、stderr、请求/响应二进制 fixture、workerd lock digest 和
预期 verdict。通过后把结果写入独立 `p2-3-gate-results.md`，不能只留下“本机可用”。

## 4. Queue tenant-facing API

### 4.1 handler 与对象

P2.3 Queue capability V1 暴露常用 JavaScript surface：

```ts
export default {
  async queue(batch: MessageBatch<Body>, env: Env, ctx: ExecutionContext) {
    for (const message of batch.messages) {
      await consume(message.body);
      message.ack();
    }
  },
};

interface Message<Body = unknown> {
  readonly id: string;
  readonly timestamp: Date;
  readonly body: Body;
  readonly attempts: number;
  ack(): void;
  retry(options?: { delaySeconds?: number }): void;
}

interface MessageBatch<Body = unknown> {
  readonly queue: string;
  readonly messages: readonly Message<Body>[];
  ackAll(): void;
  retryAll(options?: { delaySeconds?: number }): void;
}
```

`timestamp` 对应原 message `enqueued_at_ms`，不是本次 claim 时间。第一次已知 delivery 的
`attempts` 为 1。Unknown dispatch 之后的重复投递可能仍看到相同 attempt number；这是为了不让
平台故障耗尽 tenant retry budget，必须写入 capability deviation。

### 4.2 配置

```json
{
  "queue": "orders",
  "maxBatchSize": 10,
  "maxBatchTimeoutSeconds": 5,
  "maxRetries": 3,
  "retryDelaySeconds": 0,
  "maxConcurrency": 4,
  "deadLetterQueue": "orders-dlq"
}
```

约束：

| 字段 | 默认 | 范围/规则 |
| --- | --- | --- |
| `maxBatchSize` | 10 | 1..100 |
| `maxBatchTimeoutSeconds` | 5 | 0..60 |
| `maxRetries` | 3 | 0..100；0 表示首次已知失败即 terminal |
| `retryDelaySeconds` | 0 | 0..86,400 |
| `maxConcurrency` | 本地配置默认 4 | 1..`queues.max_consumer_concurrency` |
| `deadLetterQueue` | 无 | 同 account、非自身、ready/healthy |

`maxConcurrency` 不复制 Cloudflare 托管上限。默认硬上限建议 32，管理员可在 capacity test 后修改；
公开 capability 返回实际 local max。

### 4.3 disposition 规则

Pinned workerd 负责解析调用顺序，host 收到归一化结果后应用：

| message decision | handler outcome | 最终动作 |
| --- | --- | --- |
| explicit ack | 任意 | ack/delete |
| explicit retry | 任意 | retry，使用显式 delay 或 consumer default |
| 无，batch ackAll | 任意 | ack/delete |
| 无，batch retryAll | 任意 | retry，使用 batch delay 或 consumer default |
| 无 | success | ack/delete |
| 无 | failure | retry，使用 consumer default |
| 无 | Unknown | 不完成，等待 lease recovery |

Retry delay precedence 固定为：message explicit > batch explicit > consumer default。显式 0
关闭默认 delay。所有 delay 必须是有限整数 0..86,400；native response 仍由 host 二次验证。

## 5. Control schema：migration 010

P2.3A 建议新增 `crates/storage/migrations/010_queue_consumers.sql`。它不修改 P2.2 的 Queue identity，
而是加入 immutable declaration 和 live attachment。

### 5.1 deployment declaration

```sql
CREATE TABLE deployment_queue_consumers (
  id                         TEXT PRIMARY KEY,
  deployment_id              TEXT NOT NULL REFERENCES worker_deployments(id),
  queue_id                   TEXT NOT NULL REFERENCES queues(id),
  queue_lifecycle_generation INTEGER NOT NULL CHECK(queue_lifecycle_generation >= 1),
  entrypoint                 TEXT,
  max_batch_size             INTEGER NOT NULL CHECK(max_batch_size BETWEEN 1 AND 100),
  max_batch_timeout_seconds  INTEGER NOT NULL CHECK(max_batch_timeout_seconds BETWEEN 0 AND 60),
  max_retries                INTEGER NOT NULL CHECK(max_retries BETWEEN 0 AND 100),
  retry_delay_seconds        INTEGER NOT NULL CHECK(retry_delay_seconds BETWEEN 0 AND 86400),
  max_concurrency            INTEGER NOT NULL CHECK(max_concurrency > 0),
  dlq_queue_id               TEXT REFERENCES queues(id),
  dlq_lifecycle_generation   INTEGER,
  capability_version         INTEGER NOT NULL CHECK(capability_version = 1),
  descriptor_sha256          BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms              INTEGER NOT NULL,
  UNIQUE(deployment_id, queue_id),
  CHECK((dlq_queue_id IS NULL) = (dlq_lifecycle_generation IS NULL)),
  CHECK(dlq_queue_id IS NULL OR dlq_queue_id != queue_id)
) STRICT;
```

Insert trigger 必须验证：

- deployment 仍为 `staging`；
- deployment Worker 与 source/DLQ Queue 属于同 account；
- Queue 均为 `ready/healthy` 且 lifecycle generation 精确；
- entrypoint 名称合法并在 validation probe 中存在；
- source Queue 在同一 deployment 只声明一次；
- descriptor hash 覆盖所有字段，包括 `null` 与默认值；
- declaration 建立 `queue_referrers(kind='consumer')`；配置 DLQ 时同时建立
  `queue_referrers(kind='dlq')`；
- declaration immutable，只能随 staging/rejected/deleting deployment 删除。

这里允许多个未 active deployment 声明同一 Queue；“一个 active consumer”由 live attachment
约束，而不是阻止预先 staging 新版本。

### 5.2 live attachment

```sql
CREATE TABLE queue_consumers (
  id                    TEXT PRIMARY KEY,
  account_id            TEXT NOT NULL REFERENCES accounts(id),
  queue_id              TEXT NOT NULL REFERENCES queues(id),
  worker_id             TEXT NOT NULL REFERENCES workers(id),
  declaration_id        TEXT NOT NULL REFERENCES deployment_queue_consumers(id),
  deployment_id         TEXT NOT NULL REFERENCES worker_deployments(id),
  consumer_generation   INTEGER NOT NULL CHECK(consumer_generation >= 1),
  state                 TEXT NOT NULL CHECK(state IN (
                          'activating', 'active', 'paused', 'updating',
                          'deleting', 'tombstoned'
                        )),
  availability          TEXT NOT NULL CHECK(availability IN (
                          'healthy', 'degraded', 'unavailable'
                        )),
  availability_code     TEXT,
  created_at_ms         INTEGER NOT NULL,
  updated_at_ms         INTEGER NOT NULL,
  deleted_at_ms         INTEGER,
  CHECK((state = 'tombstoned') = (deleted_at_ms IS NOT NULL)),
  CHECK((availability = 'healthy') = (availability_code IS NULL))
) STRICT;

CREATE UNIQUE INDEX queue_one_live_consumer
ON queue_consumers(queue_id)
WHERE state != 'tombstoned';
```

`queue_consumers` 不复制 batch 配置；它引用 immutable declaration，避免 live row 与 frozen
descriptor 分叉。每个非 tombstone attachment 在 `deployment_referrers` 注册：

```text
kind   = queue_consumer
ref_id = queue_consumer.id
```

允许的主要转换：

```text
activating -> active
active     -> paused | updating | deleting
paused     -> active | updating | deleting
updating   -> active | paused | deleting
deleting   -> tombstoned
```

任何 generation/target 改变只能发生在 `updating`，且新 generation = old + 1。Tombstone immutable。

### 5.3 promotion 与更新协议

Consumer target 随 deployment promotion 切换，但使用跨库 fail-closed handoff：

1. 校验新 deployment declaration、entrypoint 和 referrer；
2. control 把 attachment 标成 `updating/degraded(QUEUE_CONSUMER_DRAINING)`，generation + 1；
3. scheduler projection 改为 `draining`，停止旧 generation 新 claim；
4. 等待旧 batch 完成或 lease 到期，bounded drain 超时后由 recovery 收敛；
5. control transaction 切 declaration/deployment，更新 deployment referrer；
6. scheduler transaction 写入新 frozen target/config，状态 `accepting`；
7. control 标成 `active/healthy`。

任一步 crash 后由 reconciler 根据 control state/generation 幂等继续。不能在 drain 之前直接 overwrite
deployment ID；否则旧 handler completion 与新 handler claim 会重叠。

若新 deployment 删除 consumer declaration，流程相同，但第 6 步删除 projection，第 7 步把
attachment tombstone。若另一 Worker 已 active 消费该 Queue，promotion 以
`QUEUE_CONSUMER_CONFLICT` 失败，不隐式抢占。

## 6. Scheduler schema：migration 003

新增 `crates/storage/scheduler-migrations/003_queue_consumer.sql`。必须显式删除 P2.2 的
`queue_messages_update_guard`，再用合法状态转换 trigger 代替；不能简单取消所有保护。

### 6.1 consumer projection

```sql
CREATE TABLE queue_consumer_state (
  consumer_id                    TEXT PRIMARY KEY,
  queue_id                       TEXT NOT NULL UNIQUE REFERENCES queue_state(queue_id),
  consumer_generation            INTEGER NOT NULL CHECK(consumer_generation >= 1),
  deployment_id                  TEXT NOT NULL,
  worker_id                      TEXT NOT NULL,
  execution_generation           TEXT NOT NULL,
  entrypoint                     TEXT,
  state                          TEXT NOT NULL CHECK(state IN (
                                   'staged', 'accepting', 'paused', 'draining', 'deleting'
                                 )),
  max_batch_size                 INTEGER NOT NULL CHECK(max_batch_size BETWEEN 1 AND 100),
  max_batch_timeout_ms           INTEGER NOT NULL CHECK(max_batch_timeout_ms BETWEEN 0 AND 60000),
  max_retries                    INTEGER NOT NULL CHECK(max_retries BETWEEN 0 AND 100),
  retry_delay_seconds            INTEGER NOT NULL CHECK(retry_delay_seconds BETWEEN 0 AND 86400),
  max_concurrency                INTEGER NOT NULL CHECK(max_concurrency > 0),
  dlq_queue_id                   TEXT REFERENCES queue_state(queue_id),
  dlq_queue_generation           INTEGER,
  descriptor_sha256              BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  updated_at_ms                  INTEGER NOT NULL,
  CHECK((dlq_queue_id IS NULL) = (dlq_queue_generation IS NULL)),
  CHECK(dlq_queue_id IS NULL OR dlq_queue_id != queue_id)
) STRICT;
```

Projection upsert 必须 compare generation 与 descriptor digest：相同 generation + 不同 digest 是
`QUEUE_CONSUMER_PROJECTION_CONFLICT`，不能 last-write-wins。

### 6.2 durable batch

```sql
CREATE TABLE queue_delivery_batches (
  id                    TEXT PRIMARY KEY,
  queue_id              TEXT NOT NULL REFERENCES queue_state(queue_id),
  consumer_id           TEXT NOT NULL REFERENCES queue_consumer_state(consumer_id),
  consumer_generation   INTEGER NOT NULL CHECK(consumer_generation >= 1),
  deployment_id         TEXT NOT NULL,
  execution_generation  TEXT NOT NULL,
  entrypoint            TEXT,
  claim_token           BLOB NOT NULL CHECK(length(claim_token) = 32),
  state                 TEXT NOT NULL CHECK(state = 'claimed'),
  claimed_at_ms         INTEGER NOT NULL,
  claim_until_ms        INTEGER NOT NULL,
  message_count         INTEGER NOT NULL CHECK(message_count > 0),
  created_at_ms         INTEGER NOT NULL
) STRICT;

CREATE INDEX queue_delivery_batches_expired
ON queue_delivery_batches(claim_until_ms, id);
```

对 `queue_messages` 增加：

```sql
ALTER TABLE queue_messages ADD COLUMN claim_batch_id TEXT;
ALTER TABLE queue_messages ADD COLUMN consumer_id TEXT;
ALTER TABLE queue_messages ADD COLUMN consumer_generation INTEGER;
```

SQLite 无法用 `ALTER TABLE` 增加跨列 CHECK，因此 migration 用 trigger 强制：ready row 的三个新增
字段与已有 claim 字段全部为 NULL；claimed row 必须全部存在，且对应 batch/consumer/generation。

索引：

```sql
CREATE INDEX queue_messages_claimed_batch
ON queue_messages(claim_batch_id, seq)
WHERE state = 'claimed';

CREATE INDEX queue_messages_batch_eligibility
ON queue_messages(queue_id, available_at_ms, seq)
WHERE state = 'ready';
```

`queue_messages_update_guard` 的替代 trigger 只允许 repository 使用的转换：

```text
ready   -> claimed  exact consumer projection + batch membership
claimed -> ready    exact batch/token/generation recovery or retry
claimed -> DELETE   exact ack/discard/DLQ completion
ready   -> DELETE   retention/purge only
```

Body、content type、message ID、queue generation、enqueue timestamp 和 body bytes 永远 immutable。

### 6.3 DLQ intake

为避免 DLQ 满或临时 unavailable 时重新调用已经耗尽 retry 的 source handler，新增：

```sql
CREATE TABLE queue_dlq_pending (
  message_id              TEXT PRIMARY KEY,
  source_queue_id         TEXT NOT NULL,
  target_queue_id         TEXT NOT NULL REFERENCES queue_state(queue_id),
  target_queue_generation INTEGER NOT NULL,
  terminal_attempts       INTEGER NOT NULL CHECK(terminal_attempts > 0),
  next_attempt_at_ms      INTEGER NOT NULL,
  created_at_ms           INTEGER NOT NULL,
  last_error_code         TEXT
) STRICT;

CREATE INDEX queue_dlq_pending_due
ON queue_dlq_pending(next_attempt_at_ms, message_id);
```

Pending message 仍保留在 `queue_messages`，但必须是 ready/unclaimed 且 claim query 通过 `NOT EXISTS`
排除。正常情况下 terminal completion 在同一 transaction 中：删除 source row，使用同一 message ID、
body/content type 在 target Queue 插入新 row，重置 attempts 为 0，并按 target retention 重新计算
enqueue/expiry。若 target backlog quota 暂时不足，则同一 transaction 清除 claim、插入
`queue_dlq_pending`；source consumer 不再收到它。后台在 target 可用时原子完成 move。

这样“不丢消息”和“DLQ backpressure”都可解释。Pending 仍受 source 原始 retention；到期后明确
discard 并记录 `QUEUE_DLQ_PENDING_EXPIRED`，不能无限占盘。

## 7. Batch eligibility 与 claim

### 7.1 何时形成 batch

对每个 `accepting` consumer，先找 `available_at_ms <= now`、未过 retention、非 DLQ pending 的
ready rows。触发条件：

```text
ready_count >= max_batch_size
OR
now >= oldest_due.available_at_ms + max_batch_timeout_ms
```

`max_batch_timeout_ms = 0` 时第一条 due message 立即可 claim。Timeout 从消息变为 available 开始，
不是从 enqueue 开始；有 delivery delay 的消息不能提前消耗 batch wait。

Scheduler 的 `next_due()` 返回以下最小值：

- 第一条未来 `available_at_ms`；
- 当前不足一批时 `oldest_due.available_at_ms + batch_timeout`；
- expired batch lease；
- DLQ pending retry；
- retention deadline。

### 7.2 claim transaction

一次短 `BEGIN IMMEDIATE`：

1. bounded recover expired batches；
2. 重读 consumer projection，要求 `accepting` 且 generation/digest 精确；
3. 检查 P2.1 Queue pool permit 与 consumer in-flight batch 数；
4. 按 `available_at_ms, seq` 选择至多 `max_batch_size` 条；
5. 生成 UUIDv7 batch ID 和随机 32-byte claim token；
6. 插入 `queue_delivery_batches`，冻结 deployment、entrypoint、execution generation；
7. 将选中 message 改为 claimed，写 batch/token/lease，但不增加已知 attempts；
8. commit 后构造 custom event；
9. runtime 的 `Message.attempts = persisted_attempts + 1`。

不能先 SELECT 后在另一个 transaction UPDATE；即使单进程也会在多 scheduler task 下重复 claim。

### 7.3 lease recovery

Expired batch recovery 只在 exact batch/token membership 上执行：

- message 恢复 ready、清空 claim 字段；
- batch row 删除；
- `attempts` 不增加；
- `available_at_ms` 使用基础设施 backoff，而非 consumer retry delay；
- infra failure 计数进入低基数 metrics/circuit，不进入 tenant `maxRetries`。

未知 outcome 可能意味着 handler 已完成外部副作用，因此重复是 at-least-once 的必要结果。应用应使用
message ID 做幂等键。

## 8. Dispatch 与 completion

### 8.1 private dispatch envelope

`platformd -> workerd` envelope 只包含 host authority：

```json
{
  "batchId": "uuid",
  "queueName": "orders",
  "consumerId": "uuid",
  "consumerGeneration": 4,
  "deploymentId": "uuid",
  "executionGeneration": "...",
  "entrypoint": null,
  "messages": [
    {
      "id": "uuid",
      "timestampMs": 1787700000000,
      "attempts": 1,
      "contentType": "json",
      "body": "...bounded bytes..."
    }
  ]
}
```

Claim token 不进入 tenant realm。Private response 只接受 native workerd 归一化的 disposition，且设置
message count、ID count、delay、body 和 total response bytes 上限。

### 8.2 known completion transaction

Host 先验证 disposition 中所有 ID：

- 必须属于本 batch；
- 不能重复；
- explicit ack/retry 集合不能交叉；
- delay 合法；
- response outcome 是有限 enum。

然后一个 scheduler transaction 对每个 message 应用：

- ack：token-exact DELETE；
- retry：令本次 delivery number `n = attempts + 1`；
- 若 `n <= max_retries`，写 `attempts = n`、ready、`available_at = now + delay`；
- 若 `n = max_retries + 1`，进入 DLQ/discard；
- retention 已到期时直接 discard，不重新排队；
- 最后删除 batch row。

这里 `max_retries = 3` 表示初次 delivery 加最多三次 retry，总共四次已知失败机会。

若 completion transaction 返回 0 row，说明 token/generation 已 stale；记录 stale metric，向 workerd
返回成功吸收，不能覆盖新 claim。

### 8.3 timeout 分类

| 情况 | 分类 | 动作 |
| --- | --- | --- |
| workerd 明确返回 handler success/failure | Known | 立即 completion |
| workerd 在接收前明确拒绝 deployment | Known infrastructure reject | 短 backoff 恢复，不消耗 retry |
| request body 尚未发出且连接失败 | Known not dispatched | 可立即安全恢复 claim |
| body 可能发出、response timeout、进程 abort | Unknown | 保留 lease |
| platformd 自身在 dispatch 后 crash | Unknown | restart 后 lease recovery |

Transport 必须根据写入阶段保守分类；无法证明 not-dispatched 就是 Unknown。

## 9. Concurrency、公平性与生命周期

### 9.1 三层 admission

```text
P2.1 global max_in_flight
        ↓
P2.1 Queue pool cap
        ↓
queue_consumer_state.max_concurrency
```

Consumer in-flight 以 durable `queue_delivery_batches` + 当前 dispatch task 计算。Restart 后旧 claimed
batch 仍占 concurrency，直到 lease recovery，防止刚启动就无界重复。

Pool 内 Queue 选择使用 P2.1 deterministic fairness：有 backlog 的 Queue 轮转，每次至多 claim
一个 batch，再回到 selector。禁止把最大 Queue 连续 drain 到空。

### 9.2 pause、delete、purge

- pause：projection `paused`，停止新 claim，保留 backlog；
- resume：generation 不变，projection 回 `accepting`；
- delete consumer：drain 后删 projection/referrer，Queue backlog 仍在；
- delete Queue：已有 consumer/DLQ/deployment declaration referrer 时拒绝；
- purge Queue 若后续增加，必须先 pause/drain，并与 producer generation fence 配合；不纳入 P2.3。

### 9.3 restart 与 shutdown

Graceful shutdown 顺序：stop new Queue claims → 等待 bounded dispatch drain → checkpoint。超时 batch 保持
claim，restart 后按 lease 恢复。不得在 shutdown timeout 时把所有 batch 直接改 ready。

## 10. P2.3.5：Scheduled custom-event 与 parser Hard Gate

在写 Cron schema 前证明：

1. dynamic loader 可调用默认 export `scheduled(controller, env, ctx)`；
2. `controller.type === "scheduled"`；
3. `controller.cron` 保留 exact expression，`scheduledTime` 是传入 logical slot ms；
4. `controller.noRetry()` 可由 native response 观察；
5. return、throw、`waitUntil()` rejection、timeout/abort 分类稳定；
6. frozen deployment 的 KV/D1/R2/DO/Queue bindings 与普通请求一致；
7. warm/cold loader 与 restart 结果一致；
8. 选定 Rust parser 对官方五字段、月份/星期缩写、`L/W/#` fixture 与预期 UTC next-run 一致。

Parser 建议沿用 WDL 已验证的 `croner` crate，但必须 pin version、纳入 lock，并建立 Cloudflare 文档
fixture；不因 WDL 使用它就跳过兼容测试。

## 11. Cron control schema：migration 011

建议新增 `011_cron_triggers.sql`，不与 Queue migration 010 合并。

### 11.1 immutable deployment config

```sql
CREATE TABLE deployment_cron_configs (
  deployment_id      TEXT PRIMARY KEY REFERENCES worker_deployments(id),
  mode               TEXT NOT NULL CHECK(mode IN ('inherit', 'replace')),
  capability_version INTEGER NOT NULL CHECK(capability_version = 1),
  descriptor_sha256  BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms      INTEGER NOT NULL
) STRICT;

CREATE TABLE deployment_cron_declarations (
  id                  TEXT PRIMARY KEY,
  deployment_id       TEXT NOT NULL REFERENCES worker_deployments(id),
  expression          TEXT NOT NULL,
  expression_sha256   BLOB NOT NULL CHECK(length(expression_sha256) = 32),
  parser_version      INTEGER NOT NULL CHECK(parser_version >= 1),
  created_at_ms       INTEGER NOT NULL,
  UNIQUE(deployment_id, expression)
) STRICT;
```

部署配置语义：

- `triggers`/`crons` omitted → `mode='inherit'`，promotion 保留当前 expression 集合，但 retarget
  到新 deployment；
- `crons: []` → `mode='replace'` 且没有 declaration，promotion 删除所有 activation；
- 非空数组 → `mode='replace'`，exact expression 去重后完整替换；
- validation 时解析；expression 原文和 parser-normalized digest 都冻结；
- ready 后 config/declaration immutable。

### 11.2 live activation

```sql
CREATE TABLE cron_activations (
  id                    TEXT PRIMARY KEY,
  account_id            TEXT NOT NULL REFERENCES accounts(id),
  worker_id             TEXT NOT NULL REFERENCES workers(id),
  deployment_id         TEXT NOT NULL REFERENCES worker_deployments(id),
  expression            TEXT NOT NULL,
  expression_sha256     BLOB NOT NULL CHECK(length(expression_sha256) = 32),
  parser_version        INTEGER NOT NULL CHECK(parser_version >= 1),
  activation_generation INTEGER NOT NULL CHECK(activation_generation >= 1),
  state                 TEXT NOT NULL CHECK(state IN (
                          'staging', 'active', 'retiring', 'tombstoned'
                        )),
  availability          TEXT NOT NULL CHECK(availability IN (
                          'healthy', 'degraded', 'unavailable'
                        )),
  availability_code     TEXT,
  created_at_ms         INTEGER NOT NULL,
  updated_at_ms         INTEGER NOT NULL,
  deleted_at_ms         INTEGER,
  UNIQUE(worker_id, activation_generation, expression),
  CHECK((state = 'tombstoned') = (deleted_at_ms IS NOT NULL))
) STRICT;
```

每个非 tombstone activation 注册 `deployment_referrers(kind='cron_activation')`。表达式字符串是
tenant-visible identity；hash 只用于 integrity，不替代原文。

### 11.3 promotion handoff

为避免 promotion 窗口调用旧 deployment：

1. 解析 desired expressions，生成下一 activation generation；
2. scheduler 写 new `staged` schedules，并把 old schedules 改 `draining`；
3. drain old claimed runs；
4. control transaction promote Worker，创建 new staging activations、retire old；
5. scheduler transaction 同时启用 new、停用 old；
6. control 标记 new active/healthy，old 在无 run/ref 后 tombstone。

Crash 后可能短暂停止 Cron，但不能继续在新 promotion 后向旧 deployment 发新 slot。Reconciler 以
control generation 和 worker active deployment 为 authority，幂等完成或在 promotion 尚未提交时恢复 old。

## 12. Cron scheduler schema：migration 004

### 12.1 schedule projection

```sql
CREATE TABLE cron_schedules (
  activation_id          TEXT PRIMARY KEY,
  account_id             TEXT NOT NULL,
  worker_id              TEXT NOT NULL,
  deployment_id          TEXT NOT NULL,
  execution_generation   TEXT NOT NULL,
  activation_generation  INTEGER NOT NULL CHECK(activation_generation >= 1),
  expression             TEXT NOT NULL,
  expression_sha256      BLOB NOT NULL CHECK(length(expression_sha256) = 32),
  parser_version         INTEGER NOT NULL CHECK(parser_version >= 1),
  state                  TEXT NOT NULL CHECK(state IN (
                           'staged', 'accepting', 'draining', 'deleting'
                         )),
  next_fire_at_ms        INTEGER NOT NULL,
  updated_at_ms          INTEGER NOT NULL
) STRICT;

CREATE INDEX cron_schedules_due
ON cron_schedules(state, next_fire_at_ms, activation_id);
```

`next_fire_at_ms` 始终是 parser 给出的 UTC minute boundary。Projection upsert 同样要求 generation
与 digest 一致。

### 12.2 durable logical run

```sql
CREATE TABLE cron_runs (
  id                    TEXT PRIMARY KEY,
  activation_id         TEXT NOT NULL REFERENCES cron_schedules(activation_id),
  activation_generation INTEGER NOT NULL,
  scheduled_at_ms       INTEGER NOT NULL,
  deployment_id         TEXT NOT NULL,
  execution_generation  TEXT NOT NULL,
  expression            TEXT NOT NULL,
  state                 TEXT NOT NULL CHECK(state IN (
                          'ready', 'claimed', 'complete', 'failed', 'skipped'
                        )),
  attempt               INTEGER NOT NULL DEFAULT 0 CHECK(attempt >= 0),
  no_retry              INTEGER NOT NULL DEFAULT 0 CHECK(no_retry IN (0, 1)),
  next_attempt_at_ms    INTEGER,
  claim_token           BLOB,
  claimed_at_ms         INTEGER,
  claim_until_ms        INTEGER,
  error_code            TEXT,
  created_at_ms         INTEGER NOT NULL,
  completed_at_ms       INTEGER,
  UNIQUE(activation_id, activation_generation, scheduled_at_ms)
) STRICT;

CREATE INDEX cron_runs_due
ON cron_runs(state, next_attempt_at_ms, scheduled_at_ms, id)
WHERE state = 'ready';

CREATE INDEX cron_runs_expired
ON cron_runs(claim_until_ms, id)
WHERE state = 'claimed';
```

Schedule projection 与 run 分表很重要：advance next fire 和 insert unique run 在一个 transaction，
而 dispatch retry 只修改 run，不倒退 schedule。

## 13. Cron slot、misfire 与 retry

### 13.1 slot projection

每次 Cron pool 处理 due schedule：

1. 用 pinned parser 找 `<= now` 的最近 logical slot；
2. 跳过早于 `now - cron_misfire_grace_ms` 的历史 slot，不逐条插入；
3. grace 内最多插入最近一个 slot，依赖 UNIQUE 去重；
4. 同一 transaction 将 `next_fire_at_ms` advance 到严格大于该 slot 的下一个值；
5. commit 后 run 由普通 claim/dispatch 流程处理。

建议默认 `cron_misfire_grace_ms = 300000`，但 capability/doctor 必须显示实际值。恢复五小时停机时不
回放五小时的每分钟任务；恢复后只等待下一个未来 slot。Grace 只覆盖 scheduler 短暂停顿。

### 13.2 dispatch

Claim 冻结 run 的 deployment/execution generation，并生成 32-byte token。Custom event：

```js
controller.type          // "scheduled"
controller.cron          // exact stored expression
controller.scheduledTime // logical scheduled_at_ms
controller.noRetry()
```

Handler success → `complete`；known failure 且 `noRetry=true` → `failed`；known failure 且可 retry →
固定本地 backoff；Unknown → 保留 lease。建议 product retry 为 3 次，2/4/8 秒，加 deterministic
jitter；这是本项目 capability，不声称 Cloudflare 内部相同。Infra Unknown 不消耗 product attempt。

Run history 每 activation 只保留最近 100 条或 7 天中的较小集合，GC 不删除 claimed row。History
不保存 tenant console/body，只保存 outcome、时间、attempt 和低基数 error code。

### 13.3 parser 与 clock

- 只接受五字段；
- minute/hour/day-of-month/month/day-of-week；
- 支持官方表中的 `* , - / L W #` 与大小写不敏感三字母名称；
- weekday 数字按 Cloudflare `1=Sunday ... 7=Saturday` fixture；
- UTC-only；
- 使用 P2.1 wall-clock floor 处理系统时间回拨；
- virtual clock 测试月末、闰年、`LW`、`#`、回拨和大幅前跳。

Parser 若不能精确通过这些 fixture，就只发布实际支持的语法并在 validation fail closed；不能接受后
运行到错误时间。

## 14. Reconciler 与故障矩阵

### 14.1 Queue reconcile

每轮 bounded：

1. `queue_consumers` activating/updating/deleting 与 projection 对账；
2. expired Queue batch recovery；
3. ready message/claimed batch membership 校验；
4. stale consumer generation projection 修复；
5. DLQ pending forward；
6. backlog counters 与实际 row 抽样对账；
7. deployment referrer 泄漏/缺失修复。

### 14.2 Cron reconcile

1. activation 与 schedule projection generation/digest 对账；
2. active deployment 与 activation target 对账；
3. next fire 重算只允许向前，不能倒退创建旧 slot；
4. expired run recovery；
5. stale staged/draining projection 收敛；
6. terminal history GC 与 deployment referrer release。

### 14.3 必测 crash points

Queue：

| Crash point | Restart 后要求 |
| --- | --- |
| producer insert 前/transaction 中/commit 后 | 延续 P2.2：全 batch 或零 batch |
| consumer projection stage 后、control switch 前 | fail closed，旧 consumer 可恢复或继续 handoff |
| batch row insert 后、message claim transaction 内 | transaction rollback，不出现空 batch |
| claim commit 后、dispatch 前 | lease 后 ready，不消耗 retry |
| request 部分写出后 | Unknown，lease 后可能重复 |
| handler 外部副作用后、response 前 | message 重投，ID 相同 |
| disposition 收到后、completion commit 前 | 重投，显式 ack 可能重复执行 |
| completion transaction 中 | 全 batch decision 原子提交或全部 rollback |
| DLQ source delete 与 target insert 之间 | 同 transaction：两者一起 commit 或一起 rollback |
| DLQ quota fallback transaction 中 | message 仍在 source authority 且被 pending 排除 |
| pause/update drain 中 | 无新旧 generation 重叠 claim |

Cron：

| Crash point | Restart 后要求 |
| --- | --- |
| schedule advance 前 | unique slot 后可重新投影 |
| run insert 与 schedule advance transaction 中 | 两者一起 rollback |
| run commit 后、dispatch 前 | lease/retry 后执行同一 slot，不新建第二 row |
| handler side effect 后、completion 前 | 允许重复，同一 logical slot identity |
| noRetry response 后、commit 前 | 可能再投一次；token fence 保证不覆盖新 claim |
| activation old drain/new stage 中 | 暂停可接受，旧 deployment 新 slot 不可接受 |
| wall clock 前跳/回拨 | bounded misfire、无历史 storm、无 slot 倒退 |

外部 harness 继续负责 SIGKILL；fault points 只编译进 test-support binary。

## 15. Error、metrics 与 operator surface

### 15.1 stable errors

至少定义：

```text
QUEUE_CONSUMER_CONFLICT
QUEUE_CONSUMER_NOT_READY
QUEUE_CONSUMER_PROJECTION_PENDING
QUEUE_CONSUMER_GENERATION_STALE
QUEUE_DISPOSITION_INVALID
QUEUE_RETRY_DELAY_INVALID
QUEUE_DLQ_INVALID
QUEUE_DLQ_BACKPRESSURED
QUEUE_CUSTOM_EVENT_UNSUPPORTED
CRON_EXPRESSION_INVALID
CRON_EXPRESSION_UNSUPPORTED
CRON_PROJECTION_PENDING
CRON_ACTIVATION_STALE
CRON_CUSTOM_EVENT_UNSUPPORTED
```

Public error 不包含 SQL、token、absolute data path、S3 credential、tenant body 或 workerd internal URL。

### 15.2 metrics

低基数 label 仅使用 workload/outcome/reason：

```text
open_compute_queue_consumer_batches_total{outcome}
open_compute_queue_consumer_messages_total{outcome}
open_compute_queue_consumer_in_flight
open_compute_queue_consumer_claim_latency_seconds
open_compute_queue_consumer_handler_seconds
open_compute_queue_consumer_stale_completions_total
open_compute_queue_dlq_moves_total{outcome}
open_compute_queue_dlq_pending
open_compute_cron_slots_total{outcome}
open_compute_cron_runs_total{outcome}
open_compute_cron_in_flight
open_compute_cron_lag_seconds
open_compute_cron_stale_completions_total
```

Queue ID、Worker ID、expression、deployment ID 和 error message 不作为 Prometheus label。

### 15.3 operator API

Authenticated operator surface 提供：

- consumer state、generation、target、backlog count/bytes、ready/claimed/DLQ-pending count；
- pause/resume、bounded repair、drain status；
- Cron activation/schedule state、next fire、last outcome、lag；
- pool/global admission、circuit、expired lease count；
- 不返回 Queue body、Workflow payload、secret 或原始 tenant exception stack。

Doctor 增加：consumer projection mismatch、orphan batch、counter drift、DLQ target unavailable、Cron parser
version mismatch、next-fire invalid、active deployment referrer missing。

## 16. Snapshot、restore 与 upgrade

P2.3 参加 P1 整机 maintenance snapshot：先停止新 claim，bounded drain，然后 snapshot
`control.sqlite` 与 `scheduler.sqlite`。备份按既有决定不考虑保密性，但仍要 checksum、manifest 与
source-compatible release 校验。

Restore 后：

1. 所有 pre-snapshot claimed Queue/Cron row 视为 expired，或等待保存的 lease 到期；
2. startup generation 改变，旧 workerd completion 无法认证；
3. reconciler 对齐 control generation 与 scheduler projection；
4. Cron 不 catch up 超出 grace 的停机 slots；
5. Queue backlog/DLQ pending 不丢；
6. runtime lock、schema version 和 parser version 不支持时启动 fail closed。

Migration 为 forward-only。003 必须在旧 002 fixture 上真实升级并验证 P2.2 producer仍能发送；004
必须在已有 Queue backlog/claim fixture 上升级且不改 Queue row。

## 17. 工作包与验收

### P2.3.0 Queue Hard Gate

- 最小 Queue custom event probe；
- disposition/serialization/waitUntil/timeout fixtures；
- warm/cold/restart 与 named entrypoint verdict；
- 固化 gate result。

### P2.3.1 Control model

- migration 010、repository、deployment upload validation；
- consumer declaration/referrer/one-active trigger；
- activation/pause/update/delete/reconcile；
- API isolation/idempotency/audit tests。

### P2.3.2 Claim engine

- migration 003；
- batch eligibility、timeout、fairness、concurrency；
- token/generation exact claim/recovery；
- P2.2 producer regression。

### P2.3.3 Runtime completion

- native custom event dispatch；
- body decode、attempt/timestamp；
- ack/retry precedence 与 delay；
- Known/Unknown transport classification；
- handler/waitUntil/timeout tests。

### P2.3.4 DLQ 与 Queue Gate

- max retries、atomic move、pending backpressure；
- update/drain/pause/restart/snapshot；
- crash matrix、operator/doctor/metrics；
- Queue Consumer Exit Gate。

### P2.3.5 Cron Hard Gate

- scheduled custom event probe；
- parser compatibility fixtures；
- noRetry/waitUntil/timeout/restart verdict。

### P2.3.6 Cron product

- migrations 011/004；
- deploy omitted/empty/replace；
- activation handoff、slot projection、misfire；
- dispatch/retry/history/reconcile。

### P2.3.7 Aggregate Gate

- Queue + Cron concurrency/fairness；
- full crash matrix；
- snapshot/fresh-host restore；
- upgrade from P2.2 fixture；
- aggregate regression 与 coverage。

## 18. 测试矩阵

### 18.1 Queue API

- empty Queue 不 dispatch；1/10/100 message batch；size/timeout whichever first；
- JSON/text/bytes 与 malformed stored row fail closed；
- message/batch ack/retry 全组合与 first-call-wins；
- explicit retry delay 0、1、86,400、越界；
- success/throw/waitUntil reject/timeout/abort；
- `maxRetries` 0、1、3、100；
- no DLQ discard、DLQ move、DLQ full pending、self/cross-account DLQ reject；
- pause/resume、consumer removal、target promotion、old late completion；
- per-consumer/Queue pool/global concurrency；
- two accounts same names/IDs tampering；
- retention 与 claim/DLQ pending race；
- producer `sendBatch()` 与 consumer claim 并发。

### 18.2 Cron

- 每分钟、步进、范围、列表、月份/星期缩写；
- month end、leap day、`L`、`LW`、`W`、`#`；
- UTC boundary、negative epoch reject、overflow；
- omitted/empty/replace 与 rollback promotion；
- exact expression 回传、scheduledTime、noRetry；
- handler success/failure/waitUntil/timeout/Unknown；
- slot UNIQUE、schedule advance crash、misfire grace；
- wall clock forward/backward、restart、snapshot/restore；
- old deployment drain 与 new activation。

### 18.3 既有 regression

- G0 exact `D-abort` allowlist 不扩大；
- P0 Workers/D1/KV/R2/DO/alarms；
- P1 snapshot/upgrade/security/soak 基线；
- P2.1 scheduler fairness/alarm semantics；
- P2.2 lifecycle/producer/delay/quota/retention 与 DO fail-closed verdict。

## 19. Exit Gate

P2.3 只有满足以下条件才为 Go：

1. Queue 与 Cron 两个 Hard Gate 的能力边界有可复现证据；
2. 一个 Queue 一个 active consumer 由 DB trigger 与并发测试共同证明；
3. ack/retry/DLQ 的每个 mutation 都有 token/generation fence；
4. Queue crash matrix 不出现 committed message 无 authority、双 active generation 或 stale overwrite；
5. Cron logical slot 唯一，promotion 后不再创建指向旧 deployment 的新 slot；
6. Unknown outcome 均保留 lease，不被误判为安全重试；
7. upgrade、restart、snapshot/fresh-host restore 通过；
8. Queue/Cron backlog 下 Alarm 和其他 scheduler pool 不饥饿；
9. operator/doctor 可以定位 stuck claim、projection drift、DLQ backpressure 和 Cron lag；
10. aggregate regression 与项目 coverage gate 通过。

允许的 Conditional Go 必须精确到 surface，例如“默认 Queue consumer 可用、named entrypoint
consumer 不开放”。不允许用 Conditional Go 接受 disposition 丢失、stale completion 可提交、DLQ
非原子丢消息或 Cron 向旧 deployment 发新 slot。

完成 P2.3 后，P2.4 可以复用这些已验证资产：Workflow pool admission、token/lease recovery、
dynamic custom dispatch、frozen deployment referrer、Known/Unknown outcome 分类、virtual clock 与 crash harness。
