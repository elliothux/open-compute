# P0.8：Scheduler Kernel 与 Durable Object Alarms 详细设计

> 状态：已实现并验证（2026-08-25）
>
> 前置依赖：P0.1 至 P0.6 已按当前 checkout 和用户确认跑通；P0.7 必须先通过 Exit Gate。
>
> 直接依赖：[P0.1：Platform Foundation](./p0-1-platform-foundation.md)、
> [P0.2：Workers Runtime](./p0-2-workers-runtime.md)、
> [P0.7：Durable Objects](./p0-7-durable-objects.md)
>
> 后续消费者：P2 Queue、Workflow 和 Cron 只能复用本阶段已被 alarm 证明的 scheduler primitives，
> 不能反向扩大 P0.8 范围。

P0.8 为 Durable Object 补上 `ctx.storage.getAlarm()`、`setAlarm()` 和 `deleteAlarm()`，并首次引入
独立 `scheduler.sqlite`。设计重点不是“准时执行一个 timer”，而是在 object-native SQLite 与
scheduler SQLite 无法做跨文件事务的前提下，仍保证 stale alarm 不误触发、进程崩溃可恢复、
handler 至少执行一次且 retry 有界。

本阶段只实现 alarm 实际需要的 clock、due index、claim lease、random token、conditional commit、
expired-lease recovery、bounded dispatch 和 shutdown drain。不创建通用 task graph、Queue message、
Workflow step、DLQ 或 cron parser。

## 0. 核心不变量

P0.8 固定两层状态：

1. tenant DO facet SQLite 中的 singleton alarm row 是 authority；
2. `scheduler.sqlite.scheduled_jobs` 是可重建 due projection。

每次 set/overwrite 都生成新的 cryptographically random `row_token`。dispatch 必须同时匹配：

```text
namespace_resource_id
object_id
object_generation
row_token
```

因此以下 stale work 都是 no-op：

- 旧时间点已经被新的 `setAlarm()` 覆盖；
- alarm 已被 `deleteAlarm()` 删除；
- object 已 delete/recreate，generation 已改变；
- scheduler claim 在 lease 过期后迟到完成；
- promotion 后仍携带旧 execution generation 的 dispatch。

### 0.1 交付定义

- 每个 DO object 同一时刻最多有一个 authoritative alarm row；
- `setAlarm()` 覆盖旧 alarm，`deleteAlarm()` 不取消已经开始的 handler；
- `getAlarm()` 在 alarm handler 正在执行时返回 `null`，除非 handler 已设置新的 alarm；
- scheduler delivery 是 at-least-once，不承诺 exactly-once；
- handler 失败最多自动 retry 六次，backoff 从 2 秒开始；
- scheduler claim 只持有短 SQLite write transaction，执行 tenant code 时不持锁；
- crash 后 expired lease 可恢复，旧 claim token 不能 commit 新 claim 的结果；
- object row 已提交而 projection 缺失时可由 read/activation/periodic repair 补回；
- projection 存在而 object row 缺失/token 不同，dispatch 不调用 tenant handler并删除 stale projection；
- promotion/rollback 使用 P0.7 restart policy，不冻结旧 DO code；
- shutdown 停止新 claim，给 in-flight dispatch bounded drain，然后依赖 lease recovery；
- scheduler 故障不会阻断普通 Worker/DO fetch、RPC 或非 alarm storage API。

### 0.2 非目标

- exactly-once delivery；
- 毫秒级 real-time SLA 或 Cloudflare 全球调度延迟；
- Queue、Workflow、Cron、DLQ、priority、rate limit 或 delayed-message public API；
- 多进程/多节点 scheduler election；
- 在 S3、NFS 或 SMB share 上运行 SQLite；
- 从 `control.sqlite` 单独恢复完整 alarm 状态；
- Cloudflare plan quota、billing 或所有 internal retry heuristics；
- native facet alarm scheduler；P0.8 只使用 pinned workerd 能稳定提供的 storage/facet primitives；
- `transactionSync()` 内的 alarm mutation；见 4.5 的明确偏差。

### 0.3 当前实现与验证证据

- `scheduler.sqlite` 由 storage crate 独立拥有，使用 WAL/FULL、build-time migration checksum、
  `quick_check`、claim lease、random claim token、expired-lease recovery 和 token-exact completion；
- loaded-isolate wrapper 只在 DO facet class 边界注入 alarm shim 和 hidden `AlarmIndex` capability，
  普通 named Worker entrypoint 保持原有语义；
- object-local reserved row 是唯一 authority，projection 只通过 scoped private binding写入，
  dispatch、repair、promotion/rollback 和 object generation 都再次 fence；
- `SchedulerService` 提供 bounded claim/dispatch、wall-clock floor、timeout lease retention、六次 retry、
  exhaustion cleanup、periodic bounded repair 和 graceful drain；scheduler 不可用时普通 Worker/DO 仍可服务；
- authenticated operator API 提供 inspect、pause、resume 和单批 repair；offline CLI 只允许显式隔离坏库并
  创建空 replacement，后续由 live object bounded scan 重建 projection；
- metrics 使用固定低基数 series，health/doctor 分开报告 scheduler policy、SQLite mode、状态汇总和 token invariant。

验证结果：

- `./scripts/test-p0-8.sh` 已连续三轮 fresh process通过 P0.8 stock-workerd Gate，并递归跑通
  P0.7 至 P0.2 的全部三轮 regression Gate；
- P0.8 Gate 覆盖 constructor/class field/fetch/RPC proxy、number/Date/invalid input、past due、
  overwrite/delete/token fence、async transaction commit/rollback/coalesce、`transactionSync()` fail closed、
  read/activation/scan repair、stale authority/projection、transport unknown lease retention、六次 retry 与
  2/4/8/16/32/64 秒 backoff、exhaustion、A -> B -> rollback A 和 KV/SQL/FK/alarm `deleteAll()`；
- `./poc/g0 test all` 三轮 aggregate verdict 为 `Conditional Go`，唯一条件仍是既有精确 allowlist
  `loader:D-abort`；
- workspace format、Clippy、unit/integration、no-default-features、Rust 1.98 MSRV、metadata、dependency
  boundary、diff whitespace 和 coverage 均通过；Rust line coverage 为 90.01%。

## 1. P0.8.0：Facet alarm/shim Hard Gate

动态 tenant class 运行在 facet 上，不能假定普通 native DO namespace 的 alarm scheduler也适用于
facet。WDL 的当前实现采用 injected alarm shim 和独立 index；P0.8 必须先在 pinned stock workerd
`v1.20260823.1` 上确认确切行为，再实现 Rust scheduler。

[G0 结果](./g0-results.md)明确没有覆盖 DO alarms、WebSocket hibernation 和 DO migrations，因此
P0.7 storage/facet 通过不能替代本 Hard Gate。

### 1.1 Gate 内容

| Gate | 断言 |
| --- | --- |
| AG-01 | 记录 facet 原生 `get/set/deleteAlarm` 的确切支持/失败行为 |
| AG-02 | class-specific wrapper 能在 constructor 前把 `ctx.storage` 换成 proxy |
| AG-03 | constructor、class field、fetch、RPC 都看到同一个 storage proxy |
| AG-04 | proxy 不改变 SQL、sync KV、async KV 和 `blockConcurrencyWhile` |
| AG-05 | async transaction 可收集 alarm side effect 并在 commit 后 flush |
| AG-06 | transaction rollback 不写 projection |
| AG-07 | `transactionSync()` 内 alarm mutation fail closed |
| AG-08 | `deleteAll()` 在 pinned compatibility date 下删除 user data 与 alarm |
| AG-09 | internal alarm dispatch 不能由 tenant HTTP/RPC 伪造 |
| AG-10 | facade/shim source hash 进入 deployment descriptor且 cold/warm 一致 |

预计 Gate 会确认 native facet alarm 不可直接作为产品路径；即便某一 workerd 版本“碰巧可用”，
P0.8 仍不能同时维护 native 与 shim 两套 authority。capability V1 固定使用 shim，升级需新 capability
version 和 data migration。

### 1.2 compatibility flags

当前 deployment compatibility date 是 2026-08-22。Cloudflare 在 2026-02-24 之后让
`deleteAll()` 默认删除 alarm。为避免 workerd native alarm scheduler 与项目 shim 争抢状态，加载
tenant facet 时固定 native `delete_all_preserves_alarm` 行为，再由 shim实现当前公开
`deleteAll()` 语义。

实测 pinned facet 的直接 native `storage.deleteAll()` 不能作为产品路径。实现固定
`delete_all_preserves_alarm`，再在一次 native async `storage.transaction()` 内组合删除 KV、tenant SQL
对象和 project alarm row；transaction commit 后才 token-exact 删除 projection。这样 workerd native
alarm scheduler 不介入，project shim 仍只有一条 authority 路径。

P0.8 capability V1 不复制 2026-02-24 之前“deleteAll 保留 alarm”的旧语义：旧 compatibility date
也按当前语义删除 alarm。这是明确偏差，换来只有一条可验证、原子清理 object storage 的路径。

## 2. 交付架构

```text
tenant DO class
    └── wrapped ctx.storage
            ├── SQL/KV/transaction -> native facet storage
            └── get/set/deleteAlarm
                    ├── authoritative __open_compute_do_alarm row
                    └── private AlarmIndex binding
                            └── platformd SchedulerService
                                    └── scheduler.sqlite projection

SchedulerLoop
    1. claim due row + random claim_token + lease
    2. dispatch internal alarm to P0.7 DoRouter/DoHost
    3. facet wrapper validates authoritative row_token
    4. invoke tenant alarm({ retryCount, isRetry })
    5. conditionally complete/retry/discard projection
```

`control.sqlite` 只提供 P0.7 namespace/object/deployment authority。它不保存 alarm due time，claim 或
retry count。scheduler claim transaction 也不 `ATTACH`/join control；所需 runtime projection 都在
`scheduler.sqlite`，dispatch 时再经过 P0.7 authority fence。

## 3. Scheduler kernel

### 3.1 独立数据库

新增：

```text
data/
├── control.sqlite
├── scheduler.sqlite
└── do/workerd/...
```

`scheduler.sqlite`：

- 只由 platformd `SchedulerService` 读写；
- WAL mode、`synchronous=FULL`、foreign keys ON；
- 一个 bounded writer lane，多 reader connection；
- local filesystem only；
- 独立 schema/data format marker；
- busy timeout、statement timeout 和 result budget 固定；
- 不与 `control.sqlite` 或 DO SQLite 建跨文件事务。

### 3.2 deterministic Clock

所有 scheduler 时间读取都经：

```rust
trait Clock {
    fn wall_time_ms(&self) -> i64;
    fn monotonic_deadline(&self, after: Duration) -> Instant;
}
```

- persisted `due_at_ms`/`claim_until_ms` 使用 wall time；
- process 内 wait/timeout 使用 monotonic clock；
- test clock 可前进、后退和跳跃，不用真实 sleep；
- wall clock 回退时已经 due 的 row 不能被“撤回”；loop 保存 process-local
  `observed_wall_floor_ms = max(previous, wall_time_ms)`；
- restart 后没有 process floor，expired lease 仍以持久 wall time + conservative grace 判断。

### 3.3 schema

新增 `scheduler` 自己的 `001_scheduler.sql`，不放进 control migrations：

```sql
CREATE TABLE scheduler_meta (
  singleton          INTEGER PRIMARY KEY CHECK(singleton = 1),
  schema_version     INTEGER NOT NULL,
  data_format        TEXT NOT NULL,
  created_at_ms      INTEGER NOT NULL,
  updated_at_ms      INTEGER NOT NULL
) STRICT;

CREATE TABLE scheduled_jobs (
  id                    TEXT PRIMARY KEY,
  kind                  TEXT NOT NULL CHECK(kind = 'do_alarm'),
  namespace_resource_id TEXT NOT NULL,
  object_id             TEXT NOT NULL,
  object_generation     INTEGER NOT NULL CHECK(object_generation >= 1),
  row_token             TEXT NOT NULL,
  due_at_ms             INTEGER NOT NULL CHECK(due_at_ms > 0),
  target_deployment_id  TEXT NOT NULL,
  execution_generation  INTEGER NOT NULL CHECK(execution_generation >= 0),
  state                 TEXT NOT NULL CHECK(state IN (
    'scheduled', 'claimed', 'discarding'
  )),
  retry_count           INTEGER NOT NULL DEFAULT 0 CHECK(retry_count BETWEEN 0 AND 6),
  claim_token           TEXT,
  claim_until_ms        INTEGER,
  last_error_code       TEXT,
  created_at_ms         INTEGER NOT NULL,
  updated_at_ms         INTEGER NOT NULL,
  CHECK(length(object_id) = 64 AND object_id = lower(object_id)),
  CHECK((state = 'claimed') = (claim_token IS NOT NULL)),
  CHECK((state = 'claimed') = (claim_until_ms IS NOT NULL)),
  UNIQUE(namespace_resource_id, object_id, object_generation)
) STRICT;

CREATE INDEX scheduled_jobs_due
ON scheduled_jobs(due_at_ms, id)
WHERE state = 'scheduled';

CREATE INDEX scheduled_jobs_expired_claim
ON scheduled_jobs(claim_until_ms, id)
WHERE state = 'claimed';

CREATE INDEX scheduled_jobs_discarding
ON scheduled_jobs(updated_at_ms, id)
WHERE state = 'discarding';
```

`id` 是随机 UUIDv7，不从 object name 生成。object ID 本身是 P0.7 的 opaque 64-hex token；日志和
metrics 仍不得记录它。scheduler row 不加 control DB foreign key，因为两个 DB 独立。

### 3.4 claim transaction

一次 poll：

```text
BEGIN IMMEDIATE
  1. 把 expired claimed row 恢复为 scheduled
  2. 按 (due_at_ms, id) 取最多 claim_batch 个 due row
  3. 给每行生成 random claim_token
  4. state=claimed, claim_until_ms=now+lease
COMMIT
```

执行 tenant code 在 commit 后发生。claim token 必须是 CSPRNG/UUIDv4；不能使用递增 counter、job ID
或 process ID。所有完成操作都带：

```sql
WHERE id = :id
  AND state = 'claimed'
  AND claim_token = :claim_token
  AND row_token = :row_token
```

affected row 为 0 表示 lease 已被恢复、alarm 已改期或 object 已变化，当前 worker 丢弃结果。

### 3.5 bounded concurrency 与 fairness

```toml
[scheduler]
poll_interval_ms = 100
claim_batch = 32
max_in_flight = 16
claim_lease_ms = 60000
dispatch_timeout_ms = 30000
lease_guard_ms = 5000
repair_batch = 100
repair_interval_ms = 30000
shutdown_drain_ms = 10000
```

启动时必须校验：

```text
claim_lease_ms >= dispatch_timeout_ms + lease_guard_ms
claim_batch <= max_in_flight * 2
```

全局 semaphore 限制 dispatch。单轮按 due time/id 公平取数；同一 object 只有 unique projection，
不会并发 claim 两个 alarm。P0.8 不做 namespace priority 或 tenant weighted fairness。

## 4. Object-local alarm authority

### 4.1 reserved table

injected shim 在 tenant facet SQLite 建一张 singleton table：

```sql
CREATE TABLE IF NOT EXISTS __open_compute_do_alarm (
  id                INTEGER PRIMARY KEY CHECK(id = 1),
  scheduled_time_ms INTEGER NOT NULL CHECK(scheduled_time_ms > 0),
  retry_count       INTEGER NOT NULL DEFAULT 0 CHECK(retry_count BETWEEN 0 AND 6),
  in_flight         INTEGER NOT NULL DEFAULT 0 CHECK(in_flight IN (0, 1)),
  row_token         TEXT NOT NULL,
  last_error_code   TEXT,
  updated_at_ms     INTEGER NOT NULL
) STRICT;
```

这张表与 tenant data 位于同一个 facet SQLite，因此能与 async storage transaction 一起提交。
workerd 没有为 project table 提供 tenant SQL authorizer；tenant 可以主动 drop/modify 这张表。这不构成
跨 tenant 权限提升，因为 row 只代表该 object 自己的 alarm：

- shim 每次读取都严格校验 shape/token/time；
- invalid/missing row 被当作 no alarm；
- stale scheduler projection dispatch 时再次校验，不调用 handler并清除 projection；
- tenant 不能选择其他 namespace/object/generation，因为 AlarmIndex binding props 由 host 注入；
- 文档把 `__open_compute_` 前缀声明为 reserved，collision/tamper 的行为不保证。

不得因为 table 可见就把 scheduler projection 升级为 authority；那会让 stale row 可以误触发。

### 4.2 `setAlarm()`

接受 `number | Date`，转为 finite positive integer milliseconds：

1. 生成新 `row_token`；
2. 在 object SQLite upsert singleton row，`retry_count=0`、`in_flight=0`；
3. 通过 private AlarmIndex binding upsert projection；
4. projection key 使用 namespace/object/generation unique constraint；
5. 相同 token 的重复 upsert 幂等；delete projection 必须 token-exact；
6. side effect 成功后 Promise resolve。

若 projection 写失败，shim 做 token-exact rollback：只删除仍等于本次 token 的 object row，然后抛
`DO_ALARM_INDEX_UNAVAILABLE`。若 crash 发生在 object commit 后、projection 前，authority row 保留，
由 repair 补回。

### 4.3 `getAlarm()`

- 没有 row 返回 `null`；
- invalid row 删除并返回 `null`；
- `in_flight=1` 返回 `null`；
- 正常 row 返回 scheduled time number；
- 返回前 best-effort 幂等 upsert projection，完成 read repair；
- repair 失败不改变已提交的 authority，记录低基数 warning 后仍返回时间。

### 4.4 `deleteAlarm()`

1. 读取当前 row/token；
2. 删除 authority row；
3. AlarmIndex 只删除 token 相等的 projection；
4. projection 写失败时 best-effort 恢复原 row并抛错；
5. 已开始 handler 不取消；它的 conditional completion 因 token 缺失变成 no-op。

重复 delete 没有 row 时 resolve，不创建 projection。

### 4.5 transaction

async `ctx.storage.transaction(async txn => ...)`：

- callback 获得同一个 wrapped transaction storage；
- set/delete 只修改 transaction 中的 authority row，并收集 side effect；
- 多次 mutation coalesce，以 commit 后最终 row 为准；
- callback rollback/throw 时不触发 AlarmIndex；
- commit 后执行一次 projection upsert/delete；
- projection 失败时做 token-exact compensation 并让 transaction call reject；
- crash window由 repair 处理，不能声称跨库原子。

如果 workerd 因 conflict 重跑 transaction callback，每次 attempt 都必须创建独立 side-effect list；
只有最终成功 commit 的 attempt 可以 flush，失败 attempt 的 token/effect 全部丢弃。

`transactionSync()` callback 不能 await AlarmIndex，也无法安全 flush side effect；P0.8 在其中调用
set/deleteAlarm 立即抛 TypeError，要求改用 async `transaction()`。这是明确兼容偏差。

### 4.6 `deleteAll()`

shim 使用一次 pinned native async `storage.transaction()` 作为原子边界，在 transaction 内删除 KV、
按 dependency-safe 顺序 drop tenant SQL object和自有 alarm table，随后 token-exact 删除 scheduler projection：

- native `delete_all_preserves_alarm` 只阻止 workerd 自己的 alarm scheduler介入；
- transaction 开启 deferred foreign keys，按 trigger、view、反向创建顺序 drop user tables；
- transaction commit 前不写 scheduler；成功后再删除 projection；
- crash在两步之间只留下 stale projection，dispatch token validation会清除且不会调用 handler；
- projection删除失败时返回 error，但已经删除的 object authority不能伪装回滚；
- 2026-02-24 之前的 preserve-alarm 语义不支持。

AG-08 已用同时存在 KV、直接 SQL table、foreign key和alarm的 stock-workerd fixture覆盖原子清理；
direct native facet `deleteAll()` 的失败行为也保留为 Hard Gate 结论，产品路径不依赖它。

## 5. Projection protocol 与 repair

### 5.1 没有跨文件原子事务

`setAlarm()` 的 effect 顺序固定为：

```text
object authority commit -> scheduler projection upsert -> return success
```

`deleteAlarm()`：

```text
object authority delete -> scheduler token-exact delete -> return success
```

不能反过来先写 projection。否则 due job 可以在 authority 尚未 commit 时 claim；虽然 token validation
会拦截，但 crash 后 projection 会长期制造无效 dispatch。

### 5.2 repair sources

按成本从低到高：

1. **dispatch validation**：projection stale 时即时删除；
2. **getAlarm read repair**：常用读取路径补 projection；
3. **object activation repair**：facet constructor/wrapper 首次激活时读取 authority 并 upsert/delete；
4. **startup bounded scan**：从 P0.7 `do_objects` authority 分批激活/probe live object；
5. **periodic bounded scan**：按 stable cursor 扫描，防止永久漏掉 crash window。

scan 不读取 workerd 文件。它通过 private DoRouter repair operation 让 object 自己返回一个严格 DTO：

```text
{ exists, scheduledTimeMs?, retryCount?, rowToken? }
```

每批最多 `repair_batch`，支持 restart cursor，且受独立低优先级 concurrency semaphore 限制。repair
失败不阻断普通 object dispatch。

### 5.3 projection conflict

`row_token` 是随机 fence，不是可排序 revision。P0.8 不伪造 token 大小关系：

- 较大的 object generation 可以替换较小 generation；
- 相同 token 的 upsert 是幂等 repair/retry；
- delete 只删除相同 token；
- 同一 generation 的迟到旧 upsert 可能暂时替换 due projection；dispatch 会发现 authority token
  不同并 token-exact 删除 stale row，随后 activation/read/periodic repair 恢复当前 alarm；
- stale projection 最坏会造成 bounded repair delay，但绝不能调用错误 handler 或删除新 alarm。

如果实际测试表明同 object alarm mutation 会高频跨 await 乱序，可以在新 schema version 增加持久
`mutation_revision`；P0.8 不在随机 token 上偷偷定义顺序。

AlarmIndex backend 在写 projection 前重新验证 P0.7 binding scope/object state。tenant 不能通过构造 DTO
替另一个 object 设置 alarm。

## 6. Alarm dispatch state machine

### 6.1 claim 与 object-side claim

Scheduler claim 后调用 P0.7 private alarm path，携带：

```text
job_id
namespace_resource_id
object_id
object_generation
row_token
retry_count
target_deployment_id
execution_generation
claim_token (只供 scheduler response correlation，不交给 tenant)
```

DoRouter 先检查 object live generation，再按 P0.7 restart policy解析当前 active deployment。facet shim
随后在一次 native storage transaction 中：

1. 读取 singleton row；
2. 缺失/token 不同/generation stale -> 返回 `stale`；
3. due time 仍在未来 -> 返回 `not_due` 并提供 authoritative time用于修正 projection；
4. 设置 `in_flight=1` 和当前 retry count；
5. commit 后调用 tenant `alarm({ retryCount, isRetry })`。

internal alarm dispatch 用保留 module/hidden capability识别，不能只依赖可伪造 URL/header。

### 6.2 handler success

handler 返回后，shim 条件删除：

```sql
DELETE FROM __open_compute_do_alarm
WHERE id = 1 AND row_token = :dispatched_token;
```

如果 handler 内调用 `setAlarm()`，row token 已更新，旧 completion 删除 0 行，新 alarm 保留。Scheduler
随后用 claim+row token 条件删除旧 projection；若新 projection 已 upsert，旧 completion 同样不能删它。

### 6.3 handler failure 与 retry

Cloudflare 公开语义是 alarms at-least-once，失败后最多自动 retry 六次，exponential backoff 从 2 秒
开始。P0.8 capability V1 固定：

```text
retry 1:  2s
retry 2:  4s
retry 3:  8s
retry 4: 16s
retry 5: 32s
retry 6: 64s
```

可加 deterministic、bounded jitter，但测试 clock 必须可预测；V1 默认不加 jitter。

失败处理：

1. object row token-exact 更新 `in_flight=0`、`retry_count+1`、new due；
2. scheduler claim-token + row-token 条件 reschedule；
3. response loss 时旧 claim 等 lease expire；repair 以 object authority 修正 projection；
4. 第六次 retry 仍失败时，object-side transaction先 token-exact 删除 exhausted authority，再返回
   `exhausted`；
5. Scheduler 将仍匹配 claim/row token 的 projection 条件标记为 `discarding`；
6. authority 已清除或 row 已 stale 后，Scheduler 才 token-exact 删除 `discarding` job。

不能先删 job 再清 authority，否则 repair 会把 exhausted alarm重新插回。

### 6.4 timeout 与 duplicate

dispatch timeout 时不立即把 row改回 scheduled，否则 slow handler 与 retry 会重叠。保留 claim 到
`claim_until_ms`；lease recovery 后可能重复 delivery。handler 必须幂等，这是 at-least-once 的正常
代价。

P0.8 不尝试用任意较长 lease 宣称 exactly-once。operator 调大 timeout 时必须同步满足 lease guard。

### 6.5 set/delete during handler

- `setAlarm()` 创建新 token；当前 handler 继续运行，旧 completion 不删新 row；
- `deleteAlarm()` 删除 row；当前 handler 继续运行，旧 completion no-op；
- `getAlarm()` 在只有当前 in-flight row 时返回 null；
- handler 内 set 新 alarm 后 `getAlarm()` 返回新 time；
- 相同 object 仍依赖 actor serialization，不并行调用两个 alarm handler。

## 7. Deployment、delete 与 shutdown

### 7.1 promotion/rollback

DO alarms 服从 P0.7 restart policy，不像未来 Workflow 那样冻结 definition version：

- alarm row/projection 保存 set 时观察到的 deployment/generation用于 fence 和诊断；
- dispatch 前 DoRouter 读取当前 active deployment；
- 若 route generation 已增加，使用 P0.7 abort/reload 新 class；
- scheduler 条件更新 projection 的 target deployment/generation；
- 迟到旧 generation 不能重新加载旧 facet；
- rollback 也产生新 generation，因此 pending alarm在 rollback 后运行 rollback target code。

promotion 不枚举所有 object 或 alarm；pending row 在 dispatch/activation/repair 时 lazy retarget。

### 7.2 object/namespace delete

P0.7 object delete fence 建立后：

1. scheduler 不再 claim新的该 object job；
2. delete path 先 token-exact 删除 projection；
3. 再执行 facet native delete；
4. crash 后 P0.7 deleting reconciler重复 cleanup；
5. tombstoned generation 的迟到 claim只会得到 stale。

namespace force delete 批量复用 object 流程。不能只 `DELETE FROM scheduled_jobs` 就声称 object alarm
已删除；authority 仍可能被 repair 插回。

### 7.3 graceful shutdown

收到 shutdown：

1. ready/health 标记 scheduler draining；
2. 停止 poll 和新 claim；
3. 等待 in-flight 最多 `shutdown_drain_ms`；
4. 已完成的 conditional commit正常写回；
5. 未完成 claim留在 DB，不做无条件 release；
6. 下次启动在 lease expiry 后恢复。

强行把所有 claim立即 release 会与仍在 workerd 内执行的 handler重叠，因此禁止。

## 8. Failure policy

| 故障 | 行为 |
| --- | --- |
| `scheduler.sqlite` busy | bounded retry；alarm mutation失败，普通 DO 可用 |
| scheduler file corrupt | scheduler unavailable；不自动删除/重建 |
| DO localDisk unavailable | alarm dispatch/storage unavailable；projection保留待恢复 |
| projection upsert response loss | object authority保留；read/activation/scan repair |
| claim 后 platformd crash | lease expiry后恢复，可能重复 |
| handler success 后 commit response loss | token fence + repair收敛，可能重复 |
| projection stale | object validation no-op 并删除/修正 projection |
| object row malformed/tampered | 只隔离该 object alarm，删除 stale projection |
| wall clock jump forward | 到期 alarm bounded batch触发，不一次无限 claim |
| wall clock jump backward | process wall floor 防止已到期 alarm重新等待 |

`scheduler.sqlite` corruption 不能从 `control.sqlite` 完整恢复，但可以在 operator显式移走坏文件、创建
空 scheduler DB 后，从 live `do_objects` bounded scan 重建 alarm projection。这个 repair 会激活对象且
可能耗时，必须是显式 operator command，不在启动时静默 wipe。

## 9. Observability 与 operator surface

metrics：

```text
oc_scheduler_jobs{kind,state}
oc_scheduler_claim_total{outcome}
oc_scheduler_dispatch_duration_seconds{kind,outcome}
oc_scheduler_claim_expired_total{kind}
oc_scheduler_in_flight{kind}
oc_do_alarm_mutation_total{operation,outcome}
oc_do_alarm_delivery_total{outcome,retry_bucket}
oc_do_alarm_repair_total{source,outcome}
oc_do_alarm_lag_seconds
```

禁止 namespace/object/job ID 作为 label。structured log 只记录 request ID、job keyed hash、retry count、
stable error code 和 timing。

health：

- scheduler DB open/migration failure -> scheduler component unavailable；
- oldest due lag 超 warning threshold -> degraded；
- expired claim持续增长 -> degraded；
- repair backlog过旧 -> degraded；
- 普通 HTTP/DO health与 scheduler component分别报告。

`doctor`：

- SQLite path/local filesystem/mode/free space；
- schema/data format/WAL/FULL；
- lease > timeout + guard；
- due/claimed/discarding count与 oldest age；
- expired claim sample和token invariant；
- projection指向 live object generation的 bounded sample；
- private alarm dispatch probe；
- repair dry-run count，不读取 workerd files。

operator commands 只需要：pause/resume scheduler、inspect summary、run bounded repair、explicit corrupt-file
recovery。P0.8 不提供任意 SQL console或手工“标记完成”接口。

当前接口是 authenticated `GET /v1/scheduler`、`POST /v1/scheduler/pause`、
`POST /v1/scheduler/resume`、`POST /v1/scheduler/repair`，以及 daemon 停止后的：

```text
platformd --config /absolute/path/config.toml scheduler recover-corrupt \
  --backup-name scheduler-corrupt-<operator-unique-suffix>
```

offline recovery 会把主库、WAL、SHM 一起隔离到 `data/diagnostics/scheduler-recovery/` 并验证空库；
重启后重复执行 bounded repair，直到 summary 收敛。

## 10. 安全边界

- AlarmIndex binding由 host按 namespace/object/generation scope构造，tenant不能提交 identity override；
- scheduler dispatch走 private capability，不通过 public route；
- internal alarm URL/header在进入 tenant fetch前不可见，也不能单独作为信任依据；
- row token和 claim token使用独立随机值，均不写 tenant response/log；
- SQL参数绑定，不拼接 object/token；
- object ID虽 opaque，仍不作为日志/metric label；
- scheduler不加载 tenant code，只通过 P0.7 DoRouter dispatch；
- repair operation只返回 alarm DTO，不允许任意 object SQL；
- scheduler writer queue、claim batch、dispatch bytes和concurrency全部 bounded；
- tenant tamper reserved table最多影响自己 object alarm，token/object fence阻止跨资源操作。

## 11. Work packages

### P0.8.0：Pinned facet alarm/shim Gate（已完成）

- AG-01 至 AG-10；
- freeze native flag与shim path；
- constructor/transaction/deleteAll proxy proof。

### P0.8.1：`scheduler.sqlite` kernel（已完成）

- independent migration/connection owner；
- Clock/test clock；
- due claim、random token、lease recovery；
- bounded concurrency、shutdown drain。

### P0.8.2：DO storage alarm shim（已完成）

- reserved authority table；
- get/set/deleteAlarm；
- async transaction coalescing；
- transactionSync reject与deleteAll compatibility。

### P0.8.3：Projection protocol与repair（已完成）

- private AlarmIndex binding；
- token/generation conditional upsert/delete；
- read/activation repair；
- startup/periodic bounded object scan。

### P0.8.4：Dispatch、retry与token fence（已完成）

- object-side claim；
- internal alarm invocation；
- success/reschedule/discard；
- six retries、timeout与lease overlap tests。

### P0.8.5：Promotion、delete、shutdown与recovery（已完成）

- lazy retarget current deployment；
- object/namespace delete integration；
- graceful drain；
- crash boundary与corruption recovery。

### P0.8.6：Observability、doctor与operator commands（已完成）

- metrics/health/log redaction；
- inspect/repair/pause/resume；
- degraded/unavailable separation。

### P0.8.7：Conformance与P0 Exit Gate（已完成）

- public API semantics；
- deterministic time/retry suite；
- stock workerd三轮；
- P0.2-P0.7 regression；
- combined Worker/DO/alarm fixture。

依赖顺序固定为 `0 -> 1 -> 2 -> 3 -> 4 -> 5 -> 6 -> 7`。P0.8.1 不得为了未来 Queue/Workflow
增加没有 alarm测试消费者的 abstraction；如果一段代码只被未来产品“可能需要”，先不实现。

## 12. Test matrix

### 12.1 API

- set number/Date、past time立即 due、zero/NaN/Infinity/type reject；
- get null/time；
- overwrite只触发最新 token/time；
- delete idempotent；
- constructor/class field/fetch/RPC均得到 wrapped storage；
- direct reserved table tamper只隔离当前 alarm；
- async transaction commit/rollback/multiple mutation coalesce；
- transactionSync mutation reject；
- deleteAll current语义和旧 compatibility date明确偏差。

### 12.2 scheduler

- due ordering、same due deterministic ID order；
- claim batch/concurrency bound；
- random claim token；
- conditional completion affected-row=0；
- expired lease recovery；
- dispatch timeout不立即 release；
- shutdown drain/kill/restart；
- wall clock forward/backward、monotonic timeout；
- busy/readonly/full/corrupt scheduler DB。

### 12.3 delivery

- handler success删除 exact row；
- handler内set新 alarm保留；
- handler内delete不取消当前 execution；
- getAlarm在 in-flight时为null，reschedule后返回新 time；
- retryCount/isRetry shape；
- 2/4/8/16/32/64秒六次 retry；
- retry exhaustion先清authority再删projection；
- stale token/generation/object deletion no-op；
- response loss/lease expiry产生允许的 duplicate但不误删新 alarm。

### 12.4 repair与deployment

- crash在 object commit/projection前；
- crash在 projection commit/response前；
- projection丢失由 get/activation/scan修复；
- stale projection由dispatch清除；
- scheduler DB显式重建后从object scan恢复；
- deploy A -> B -> rollback A，pending alarm使用当前 code；
- promotion发生在 claimed/handler-running/set-new-alarm阶段；
- object/namespace delete与late claim race。

### 12.5 P0.8/P0 Exit Gate

完成必须同时满足：

1. AG-01 至 AG-10 连续三轮 fresh process通过；
2. fake Clock suite不使用真实 sleep且覆盖clock jump；
3. every-cross-DB/crash-boundary matrix通过；
4. at-least-once、六次retry、token fence和handler reschedule通过；
5. A -> B -> rollback A pending alarm运行当前deployment；
6. platformd/workerd SIGKILL分别恢复；
7. 一个 fixture Worker同时使用 KV、R2、D1、DO fetch/RPC/storage/alarm/basic WebSocket；
8. P0.2 至 P0.7 regression Gate全部通过；
9. format、Clippy、MSRV、unit/integration和diff whitespace检查通过；
10. Queue/Workflow/Cron public schema、API和speculative engine均未进入P0.8。

## 13. 建议实现文件边界

```text
crates/core/src/scheduler.rs
crates/storage/scheduler-migrations/001_scheduler.sql
crates/storage/src/scheduler.rs
crates/service/src/scheduler.rs
crates/service/src/alarm_index.rs
runtime/system-workers/do-alarm-shim.js
runtime/system-workers/do-alarm-transport.js
runtime/system-workers/do-host.js
scripts/test-p0-8.sh
```

ownership 原则：

- `SchedulerService` owns scheduler SQLite和claim state machine；
- tenant facet shim owns object authority row/API semantics；
- P0.7 DoRouter owns object/deployment authorization；
- loaded-isolate wrapper只负责注入，不拥有scheduler状态；
- control plane不读取workerd DO SQLite。

## 14. 参考

- [Cloudflare Durable Object alarms](https://developers.cloudflare.com/durable-objects/api/alarms/)
- [Cloudflare SQLite-backed storage API](https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/)
- [Cloudflare Durable Object state API](https://developers.cloudflare.com/durable-objects/api/state/)
- [Cloudflare Durable Object lifecycle](https://developers.cloudflare.com/durable-objects/concepts/durable-object-lifecycle/)
- [WDL alarm shim](https://github.com/wdl-dev/wdl/blob/main/do-runtime/alarm-shim-source.js)
- [WDL DO runtime actor](https://github.com/wdl-dev/wdl/blob/main/do-runtime/actor.js)
- [workerd config schema](https://github.com/cloudflare/workerd/blob/v1.20260823.1/src/workerd/server/workerd.capnp)
- [Miniflare Durable Objects plugin](https://github.com/cloudflare/workers-sdk/tree/main/packages/miniflare/src/plugins/do)
