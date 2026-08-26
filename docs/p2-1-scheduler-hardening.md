# P2.1：Scheduler 多 Workload 内核详细设计

> 状态：详细设计，待实现
>
> 前置基线：P1.0 至 P1.7 已由用户确认在当前 checkout 全部跑通；P1.8 维持既有
> WebSocket hibernation No-Go 结论，不阻塞 P2。
>
> 直接依赖：[P0.8：Scheduler Kernel 与 Durable Object Alarms](./p0-8-scheduler-do-alarms.md)、
> [P1：P0 平台加固](./p1-platform-hardening.md)
>
> 后续消费者：[P2.2：Queue Producer](./p2-2-queue-producer.md)、P2.3 Queue Consumer/Cron、
> P2.4 至 P2.6 Workflow。

P2.1 不增加新的 Cloudflare 产品 API。它把 P0.8 中只服务 Durable Object alarm 的单 workload
循环，收敛成 Alarm、Queue、Cron、Workflow 可以共同使用的单节点 scheduler 内核，同时完整保留
已经验证过的 alarm authority、claim token、lease、retry 和 repair 语义。

核心取舍是：共享调度机制，不共享业务 authority。Alarm 的 authority 在 object-local SQLite，
Queue message 的 authority 在 scheduler SQLite，Cron slot 和 Workflow wakeup 也各有自己的状态机。
P2.1 因此不会创建一张 polymorphic `jobs` 表，也不会把不同产品压成同一种完成协议；它只提供
admission、fairness、wake、clock、lease fence、bounded drain 和可测试的故障边界。

## 0. 决策摘要

| 主题 | P2.1 选择 | 原因 |
| --- | --- | --- |
| 产品范围 | 只重构 scheduler kernel，production 仍只注册 Alarm workload | 不让基础设施重构偷偷变成 Queue/Cron/Workflow 半成品 |
| 业务 authority | 每个 workload 自己拥有 authority、projection 和完成协议 | Alarm、Queue、Cron、Workflow 的正确性条件不同 |
| 调度模型 | 一个全局 budget + 四个独立 pool | 限制整机负载，同时防止一个 backlog 吃掉所有执行槽 |
| 公平性 | work-conserving weighted deficit round-robin | 非空 pool 可借用空闲容量，持续繁忙的 pool 不能饿死其他 pool |
| pool 内顺序 | oldest due first，再按稳定 canonical ID | 测试可重复，不依赖 SQLite 未定义的返回顺序 |
| claim | 每个 pool 各自进行短、有限批量的 SQLite claim transaction | tenant/runtime 执行期间不持数据库锁 |
| fence | typed source generation + random claim token + lease | 旧 deployment、旧 object、旧 queue generation 和迟到完成均 fail closed |
| retry | scheduler 基础设施退避与产品 retry 分层 | 不改变 P0.8 alarm handler 的 2/4/8/16/32/64 秒语义 |
| wake | event notification + next due deadline + bounded safety reconcile | 没有工作时不固定频率空轮询，也不因丢一次通知永久睡眠 |
| 时间测试 | wall/monotonic/timer 全部由可推进的 test clock 驱动 | 不使用真实 `sleep` 验证小时级 lease、backoff 和 wall-clock 回拨 |
| fault injection | 只编译进 test-support binary；外部 harness 负责 SIGKILL | production binary 不暴露任意 crash endpoint |
| schema | P2.1 保持 `scheduler.sqlite` schema version 1 | 没有持久状态变化就不制造空 migration |
| migration runner | 改成连续、带 checksum 的 migration registry | 为 P2.2 的 scheduler migration 002 做准备 |

### 0.1 P2.1 必须守住的不变量

1. P0.8 的 `scheduled_jobs` 仍只保存 `kind = 'do_alarm'`，表结构和已验证数据语义不变。
2. object-local alarm row 仍是 authority；scheduler row 仍只是可修复 projection。
3. 一个 workload 的 backlog、慢 dispatch、数据库错误或 circuit-open 不能耗尽其他 workload 的
   pool permits。
4. 全局 in-flight 数不得超过 global budget；每个 pool 的 in-flight 数不得超过自己的 cap。
5. 空闲 pool 不保留容量；有持续 backlog 的 pool 可以使用当前无人使用的 global permits。
6. 任一 SQLite claim transaction 都在调用 tenant code、workerd RPC 或外部 S3 前提交。
7. 未能确定 dispatch 是否发生时，不立即释放 claim；保留 lease，等待 recovery。
8. claim 过期、generation 改变或 token 不匹配时，迟到完成只能成为 no-op。
9. wall clock 向后跳不能让已观察到的 due 时间重新变成“未来”；延续 P0.8 wall-clock floor。
10. shutdown 先停止新 claim，再 bounded drain；超时工作由 lease recovery 接管。
11. production 构建中不存在 crash-point HTTP API、任意 SQL API或虚拟时钟开关。
12. P2.1 完成后，不创建 Queue/Cron/Workflow control row，不向 tenant 暴露相应 binding。

### 0.2 非目标

- Queue lifecycle、message enqueue、consumer、ack/retry、DLQ；
- Cron parser、next-run 计算或 scheduled handler；
- Workflow definition、instance、step、sleep 或 event；
- exactly-once execution；
- 多进程 leader election、多节点 failover 或分布式 lease；
- priority queue、tenant-defined scheduler priority 或抢占；
- 把所有 product state 迁移进 `scheduler.sqlite`；
- 实时系统 SLA；单机重负载下只承诺 bounded fairness 和可恢复；
- 修改 P0.8 alarm public API、retry 次数或 handler 语义；
- 为未来 workload 建立动态 plugin registry、boxed callback graph 或通用 DAG。

## 1. 当前实现证据与需要修复的边界

当前 checkout 已具备以下可复用基础：

- `crates/storage/src/scheduler.rs` 拥有独立 `SchedulerStore`、WAL/FULL、migration checksum、
  due claim、random claim token、lease recovery 和 token-exact completion；
- `crates/service/src/scheduler.rs` 拥有 alarm claim/dispatch、global `max_in_flight`、
  `claim_batch`、dispatch timeout、periodic repair、pause/resume 和 bounded drain；
- `crates/core/src/scheduler.rs` 已有 `SchedulerClock` 与 test-only
  `DeterministicSchedulerClock`；
- `001_scheduler.sql` 把 `scheduled_jobs` 明确限制为 `do_alarm`；
- authenticated operator surface 已提供 inspect、pause、resume、repair；
- P1 已提供 data-dir ownership、disk admission、snapshot/restore、upgrade、metrics、doctor、
  fault harness 与长稳基线。

但它目前仍是 alarm loop，不是多 workload 内核：

1. 一个 semaphore 同时承担“整机总量”和“alarm 容量”，无法表达 pool 隔离；
2. claim 顺序只有一个表，尚未定义 workload 间 fairness；
3. loop 的 interval、timeout 和 sleep 仍有真实 Tokio time 依赖，virtual clock 只能覆盖局部逻辑；
4. metrics 和 inspect 的 `kind` 仍硬编码为 `do_alarm`；
5. migration runner 只有一份 include，不足以承载后续连续 scheduler migrations；
6. wake 以 polling 为主，新写入的更早 due work 与 backoff deadline 没有统一通知协议；
7. fault boundaries 没有被组织成可复用的 claim/dispatch/complete crash matrix。

P2.1 的任务是修复这些边界，同时让所有现有 alarm regression 保持 bit-for-bit 可解释。

## 2. 交付架构

```text
SchedulerKernel
├── GlobalAdmission(max_in_flight)
├── FairSelector(weighted deficit round-robin)
├── WakeCoordinator
│   ├── generation-based Notify
│   ├── earliest due deadline
│   └── bounded safety reconcile deadline
├── WorkloadPool<Alarm>
│   ├── per-pool admission
│   ├── AlarmWorkload adapter
│   └── existing scheduled_jobs projection
├── WorkloadPool<Queue>       # P2.2/P2.3 注册
├── WorkloadPool<Cron>        # P2.3 注册
└── WorkloadPool<Workflow>    # P2.4+ 注册

每个 pool 的一次循环：
peek next due -> fairness select -> claim bounded batch
-> commit SQLite -> dispatch without DB lock
-> workload-specific complete/recover
```

P2.1 production composition 只创建并注册 `AlarmWorkload`。Queue、Cron、Workflow 的类型和
pool identity 可以先存在，但不能返回 synthetic ready work，也不能创建业务表。后续阶段通过显式
composition 把各自 adapter 注册进去。

### 2.1 为什么不用一张通用 jobs 表

四类 workload 看起来都有 `due_at`，但其 authority 和完成条件不同：

| workload | authority | claim 后正确完成的条件 |
| --- | --- | --- |
| Alarm | object-local singleton row | object generation、row token、execution generation 均匹配 |
| Queue | durable message row | consumer generation、message claim token、ack/retry/DLQ 决策匹配 |
| Cron | schedule definition + logical slot | definition generation 和 slot key 未被取代 |
| Workflow | instance/step state machine | run token、step attempt、instance generation 均匹配 |

如果把这些字段放入一张 nullable-column `jobs` 表，最终只会得到四套隐含状态机、复杂 CHECK 和一次
全局 hot-table migration。共享 kernel + typed adapter 能复用调度，又让每个产品在自己的 transaction
里维护不变量。

### 2.2 建议代码边界

在不制造 crate 级插件系统的前提下，把当前大文件拆成：

```text
crates/core/src/scheduler/
├── mod.rs          # SchedulerKind、fence、公共 summary
├── clock.rs        # wall/monotonic/timer abstraction
└── fault.rs        # 仅 test-support 可见的 fault-point enum

crates/storage/src/scheduler/
├── mod.rs          # store open/health/migration registry
├── alarm.rs        # 现有 scheduled_jobs repository
└── migrations/
    └── 001_scheduler.sql

crates/service/src/scheduler/
├── mod.rs          # composition 与 public handle
├── kernel.rs       # global admission、主循环、shutdown
├── fairness.rs     # deterministic selector
├── wake.rs         # notification/deadline
└── alarm.rs        # AlarmWorkload adapter
```

如果现有文件仍小于项目可维护阈值，可以先只抽 `kernel`、`fairness`、`wake`；文档给的是责任边界，
不是要求为了目录而拆目录。

## 3. Typed workload contract

### 3.1 固定 workload identity

```rust
pub enum SchedulerKind {
    Alarm,
    Queue,
    Cron,
    Workflow,
}
```

外部 JSON、metrics label 和 operator filter 使用固定小集合：

```text
do_alarm | queue | cron | workflow
```

不得把 queue ID、deployment ID、account ID、class name 或 error string 放进 metrics label。

### 3.2 adapter 能力

设计上需要以下能力，但实现不要求引入 `async_trait` 或动态 trait object。四个 workload 是已知闭集，
可以用直接方法和 typed enum dispatch：

```rust
trait SchedulerWorkload {
    type Claim;

    fn kind(&self) -> SchedulerKind;
    fn next_due(&self, now: WallTime) -> Result<Option<WallTime>>;
    fn claim_due(
        &self,
        now: WallTime,
        lease_until: WallTime,
        limit: NonZeroUsize,
    ) -> Result<Vec<Self::Claim>>;
    async fn dispatch(&self, claim: Self::Claim) -> DispatchOutcome;
    fn recover_expired(&self, now: WallTime, limit: NonZeroUsize) -> Result<usize>;
    fn summary(&self, now: WallTime) -> Result<WorkloadSummary>;
}
```

如果 Rust object safety 或异步返回类型使 trait 增加无价值复杂度，允许实现为：

```rust
enum WorkloadAdapter {
    Alarm(AlarmWorkload),
    Queue(QueueWorkload),
    Cron(CronWorkload),
    Workflow(WorkflowWorkload),
}
```

配合 exhaustive `match`。禁止以“未来可能有第五种 workload”为理由引入运行时插件发现。

### 3.3 claim 与 dispatch outcome

每种 claim 保留自己的 typed payload；kernel 只需要统一 envelope：

```rust
struct ClaimEnvelope<C> {
    kind: SchedulerKind,
    claim: C,
    claimed_at: WallTime,
    claim_until: WallTime,
}

enum DispatchOutcome {
    Completed,
    ProductRetryScheduled,
    StaleNoop,
    LeaseRetainedUnknown,
    CircuitOpen,
}
```

`LeaseRetainedUnknown` 表示 request 已离开 platformd，但 transport 没有给出可靠结果。kernel 不能把它
当作 ordinary error 立即重试；原 claim 保持到 lease 到期，再由 workload recovery 判断。

### 3.4 通用 fence 只描述形状

```rust
struct SchedulerFenceV1 {
    kind: SchedulerKind,
    source_id: StableId,
    authority_generation: u64,
    claim_token: RandomToken,
    claim_until_ms: i64,
}
```

这不是一张通用数据库表，也不取代 product token：

- Alarm 的 `authority_generation` 由 object generation、execution generation 与 `row_token` 共同解释；
- Queue 在 P2.3 使用 queue lifecycle generation、consumer generation 与 message claim token；
- Cron 使用 definition generation + logical slot；
- Workflow 使用 definition/instance generation + run token。

adapter 的 complete SQL 必须检查自己完整的 fence；kernel 不能只凭 envelope 决定删除业务状态。

## 4. Admission 与公平调度

### 4.1 两级 admission

每次 dispatch 同时持有：

1. 一个 global permit，限制 platformd 总 scheduler in-flight；
2. 一个 workload pool permit，限制该产品最大并发。

约束：

```text
in_flight_total <= scheduler.max_in_flight
in_flight[kind] <= pool[kind].max_in_flight
```

pool cap 之和可以大于 global cap。这样 Alarm 空闲时 Queue 可以借用整机容量；Queue 满载时仍不能超过
自己的 cap，也不能拿走 Alarm 为新 due work 所需的 pool permit。

claim 前先计算当前可用 permit，再把 `claim_limit` 截断为：

```text
min(
  workload claim_batch,
  global permits available,
  pool permits available,
  fairness quantum remaining
)
```

不得先 claim 大批记录再在内存等待 permit，否则 claim lease 会被排队时间无谓消耗。

### 4.2 fairness 算法

采用 work-conserving weighted deficit round-robin：

1. pool 顺序固定为 Alarm、Queue、Cron、Workflow，但起点延续上一轮 cursor；
2. 每轮给 ready pool 增加配置 weight 对应的 deficit；
3. claim 一个 dispatch unit 消耗一个 deficit；
4. pool 没有 ready work、没有 permit或 circuit-open 时跳过；
5. 所有 ready pool 都暂不可运行时，进入 WakeCoordinator；
6. 空闲 pool 的 global capacity 可被其他 ready pool 使用，不预留物理线程。

这里的 dispatch unit 是“一个 claim”，不是 message 字节。P2.3 的 Queue batch claim 是一个 dispatch
unit，但 batch size 另受 consumer 配置、payload 上限和 dispatch timeout 限制。

默认所有 pool weight 为 1。只有 capacity 测试给出证据后才调整默认权重；不为猜测中的生产流量预设
Queue 高权重。

### 4.3 pool 内排序

storage query 必须显式：

```sql
ORDER BY due_at_ms ASC, canonical_id ASC
LIMIT ?;
```

Alarm 延续当前稳定 key。Queue 后续使用 `available_at_ms, seq`；Cron 使用 `slot_at_ms, definition_id`；
Workflow 使用 `wake_at_ms, instance_id`。不能依赖 rowid 或无 `ORDER BY` 的偶然顺序。

### 4.4 starvation 验收

用 synthetic in-memory workload adapter 生成无限 Queue backlog，同时周期性注入 Alarm、Cron 和
Workflow ready claim。测试必须证明：

- 每个持续 ready 且未 circuit-open 的 pool 在一个可计算的 round bound 内至少获得一次 claim；
- pool cap 与 global cap 从未超限；
- Alarm pool 从 idle 变 ready 后不等待 Queue backlog 清空；
- 一个 pool 的 dispatch 永久 pending 时，只占用它自己的 cap 与对应 global permits；
- pool 为空时其他 workload 可以吃满剩余 global permits；
- selector 在相同 seed、ready sequence 和 clock 下产生相同选择序列。

测试断言 round/claim bound，不断言 CI 宿主上的毫秒延迟。

## 5. Batch claim 与 transaction 边界

### 5.1 claim transaction

每个 adapter 的 `claim_due` 必须：

1. 读取当前 authority/projection generation；
2. 选取不超过 limit 的 due rows；
3. 为每行生成不可预测 claim token；
4. 写入 `claim_until` 和 claimed state；
5. 在一次短 `BEGIN IMMEDIATE` transaction 内提交；
6. transaction 完成后才返回 typed claims。

SQLite connection 在 transaction 完成后归还 pool；dispatch 期间不得持有 transaction、statement 或
跨 await 的 write connection。

### 5.2 batch 大小

P2.1 保留 current Alarm `claim_batch` 语义，但把它移动到 Alarm pool 配置。每轮总 claim 仍受 global
permit 限制。后续 workload 使用自己的 batch：

- Alarm：一 claim 对应一个 object alarm；
- Queue：P2.3 一 claim 可对应一个 consumer message batch；
- Cron：一 claim 对应一个 definition logical slot；
- Workflow：一 claim 对应一个 instance 的一个 runnable transition。

不能用一个全局 batch 数同时解释这四种成本。

### 5.3 complete transaction

completion 由 adapter 自己完成，必须是 token-exact conditional update/delete。影响行数为 0 表示
claim 已过期或 authority 已变化，应记录 `stale_completion_total`，不得覆盖新状态。

## 6. Wake、deadline 与完整虚拟时间

### 6.1 WakeCoordinator

每个可能让 `next_due` 提前的已提交 mutation，在 commit 后调用：

```rust
wake.notify(kind, committed_generation);
```

WakeCoordinator 保存单调递增的 process-local generation。scheduler 进入等待前执行：

1. 读取 `observed_generation`；
2. 查询所有 registered workload 的 `next_due`；
3. 计算最早 due、基础设施 backoff、repair 和 safety reconcile deadline；
4. 再次比较 wake generation；
5. generation 已变则立即重新查询，否则等待 notify 或 deadline。

“先读 generation、后查 DB、再比 generation”用于避免 commit 发生在查 due 与注册 waiter 之间时丢
wakeup。notify 只是延迟优化，正确性仍由有界 safety reconcile 保底。

### 6.2 timer abstraction

现有 `SchedulerClock` 扩展为同时提供：

```rust
trait SchedulerClock {
    fn wall_now(&self) -> WallTime;
    fn monotonic_now(&self) -> MonotonicTime;
    async fn sleep_until(&self, deadline: MonotonicTime);
}
```

production 使用 system wall clock + Tokio monotonic timer；test clock 提供：

- `advance_wall(delta)`；
- `set_wall_backwards(delta)`；
- `advance_monotonic(delta)`；
- `advance_both(delta)`；
- 当前 pending timer 数；
- 同 deadline timer 的稳定唤醒顺序。

测试不能通过真实 sleep 等 lease、hour delay、retention 或 backoff。所有 scheduler timeout、repair
interval、circuit breaker、shutdown deadline 也必须使用同一抽象；只替换 `now()` 而让
`tokio::time::sleep` 留在主循环不算完成。

### 6.3 wall-clock floor

延续 P0.8：

```text
effective_wall_now = max(last_observed_wall_now, system_wall_now)
```

wall time 前跳会让 due work 变 ready；wall time后跳不会让已经 ready 的 work 重新等待。monotonic time
只用于进程内等待和 timeout，不能持久化为跨重启 deadline。

### 6.4 restart

重启后：

1. wall floor 从当前系统时间重新建立；
2. 所有 persisted due/lease 都按 wall timestamp 解释；
3. 首轮在接收 tenant traffic 前做 bounded expired-lease recovery；
4. 查询各 workload earliest due；
5. 再进入普通 fairness loop。

不持久化 scheduler cursor、deficit 或 process-local circuit state；它们重启后从默认值恢复，不影响
业务正确性。

## 7. Retry、backoff 与 circuit breaker

### 7.1 三层失败

| 失败层 | 例子 | 所有者 |
| --- | --- | --- |
| product outcome | alarm handler throw、未来 Queue message retry | product adapter |
| dispatch uncertainty | workerd request timeout、connection reset after write | claim lease + recovery |
| infrastructure poll/reconcile | SQLite busy、temporary workerd unavailable、repair query failed | scheduler kernel |

三层不得共用一个 retry counter。

### 7.2 保留 Alarm retry

P0.8 handler failure 的六次 retry 和 2/4/8/16/32/64 秒 schedule 完全不变。P2.1 不给 product
retry 添加 jitter，以免破坏已经冻结的 API/测试。

### 7.3 基础设施 backoff

poll、claim 或 repair 的瞬时基础设施错误使用：

```text
delay = min(base * 2^attempt, cap) + deterministic_jitter(key, attempt)
```

- jitter 由 process boot seed + workload kind + error class + attempt 计算；
- test seed 固定，因此 virtual-clock 测试可重复；
- jitter 只影响基础设施重新尝试，不改变 persisted product due time；
- 成功一次后 attempt reset；
- 永久 schema/corruption 错误直接让对应 pool circuit-open，不进行无限热循环。

不要把完整 error message 作为 jitter key 或 metric label。

### 7.4 pool circuit

一个 workload 连续遇到判定为 permanent 的存储/协议错误时：

- 该 pool 进入 degraded/circuit-open；
- 不再 claim 新 work；
- 已经 in-flight 的 claim按正常完成或 lease recovery；
- 其他 pool 继续运行；
- health 报具体 availability code；
- authenticated repair/health probe 成功后才能 half-open，再由一次 bounded claim 验证恢复。

control/scheduler 整库 corruption 仍是 platform-level failure，不伪装成单 pool degraded。

## 8. Projection generation 与 stale-work 防护

P2.1 不新增统一 projection table，只冻结以下 adapter 规则：

1. 每个 persisted projection 都必须携带其 authority generation；
2. claim 时把 generation 复制进 typed claim；
3. dispatch 前在最接近 tenant code 的边界再次验证 generation；
4. complete 时同时检查 generation + claim token；
5. repair 只允许从 authority 向 projection 收敛；
6. stale projection 可删除，不能反向覆盖 authority；
7. projection 写入发生在 authority commit 后时，必须可由 activate/read/periodic scan 补偿；
8. authority 已删除时，dispatch 必须 no-op 并清理 stale projection。

Alarm 继续使用已有 object generation、execution generation 和 random `row_token`。P2.2/P2.3 的
Queue generation 定义在 Queue 文档中，不能复用 alarm token。

## 9. Schema 与 migration

### 9.1 P2.1 不新增 schema

`scheduler.sqlite` 仍为 schema version 1，`scheduled_jobs` 不改名、不加通用列、不放未来 Queue
message。没有数据变化就不创建 `002_empty.sql`。

### 9.2 migration registry 重构

把当前单 migration include 改成与 control DB 相同的连续 registry：

```rust
const SCHEDULER_MIGRATIONS: &[Migration] = &[
    Migration::new(1, "001_scheduler", include_str!("migrations/001_scheduler.sql")),
];
```

runner 必须：

- version 从 1 连续递增，不允许 gap、duplicate 或重排；
- 对 name + SQL bytes 计算 checksum；
- 已应用 checksum 不匹配即拒绝启动；
- 每个 migration 在显式 transaction 中执行；
- `foreign_keys=ON`、WAL/FULL 和 quick_check 保持；
- production 与 offline doctor 使用同一个 registry；
- release identity 从 registry 生成 scheduler schema identity。

P2.2 再添加真实 `002_queue_producer.sql`。

## 10. 配置合同

保留现有 `[scheduler]` 字段语义，并增加带默认值的 per-pool 配置。旧 P1 配置文件必须仍可启动：

```toml
[scheduler]
max_in_flight = 64
dispatch_timeout_ms = 30000
claim_lease_ms = 60000
repair_interval_ms = 30000
shutdown_drain_ms = 30000

[scheduler.pools.alarm]
enabled = true
max_in_flight = 16
claim_batch = 32
weight = 1

[scheduler.pools.queue]
enabled = false
max_in_flight = 32
claim_batch = 16
weight = 1
```

示例数值不是容量承诺；最终默认值由 current P0.8 default 与 P1 capacity envelope 推导。实现要求：

- 所有新增字段 `serde(default)`；
- global/pool cap、weight、batch 必须大于 0 且有 hard maximum；
- production P2.1 强制 Queue/Cron/Workflow disabled，即使用户提前写 enabled 也返回稳定 config error；
- 后续 release capability 打开后才接受对应 pool；
- config hash 进入 doctor/support bundle，但不输出 secret。

## 11. Pause、shutdown 与 owner lifecycle

### 11.1 pause

保留全局 pause/resume，并允许 authenticated operator 对固定 kind 暂停：

```text
POST /v1/operator/scheduler/pause
POST /v1/operator/scheduler/pause?kind=do_alarm
POST /v1/operator/scheduler/resume
POST /v1/operator/scheduler/resume?kind=do_alarm
```

pause 只阻止新 claim：

- 已 claim work 继续完成；
- due insert、authority mutation 和 projection repair 继续持久化；
- health 显示 paused，不伪装成 unavailable；
- pause 状态是 process-local operator control，重启默认按配置恢复运行；
- P2.1 只允许 `do_alarm` kind；其他 kind 返回 `SCHEDULER_KIND_NOT_ENABLED`。

### 11.2 shutdown

1. daemon owner 触发 global stop-claim；
2. WakeCoordinator 唤醒所有 waiter；
3. 不再创建新 claim；
4. 等待每个 pool in-flight，在 global shutdown deadline 内 drain；
5. deadline 到达后取消本地等待，不伪造 complete；
6. 未知 dispatch 保留 lease，由下次启动 recovery；
7. flush metrics/health summary 后释放 scheduler store。

测试同时覆盖正常 drain、一个 pool hung、多个 pool hung 和 shutdown 时新 due insert。

## 12. 可观测性与 operator surface

### 12.1 metrics

固定低基数：

```text
open_compute_scheduler_ready{kind}
open_compute_scheduler_in_flight{kind}
open_compute_scheduler_claim_total{kind,outcome}
open_compute_scheduler_dispatch_total{kind,outcome}
open_compute_scheduler_claim_latency_seconds{kind}
open_compute_scheduler_dispatch_latency_seconds{kind}
open_compute_scheduler_oldest_due_age_seconds{kind}
open_compute_scheduler_stale_completion_total{kind}
open_compute_scheduler_lease_recovery_total{kind}
open_compute_scheduler_pool_state{kind,state}
open_compute_scheduler_wake_total{reason}
```

`outcome`、`state` 和 `reason` 都是枚举。禁止 resource/account/message/object/deployment ID label。

### 12.2 inspect JSON

`GET /v1/operator/scheduler` 返回 versioned summary：

```json
{
  "version": 2,
  "paused": false,
  "global": {
    "inFlight": 0,
    "maxInFlight": 64,
    "nextWakeAt": null
  },
  "pools": [
    {
      "kind": "do_alarm",
      "enabled": true,
      "state": "ready",
      "ready": 0,
      "claimed": 0,
      "expired": 0,
      "oldestDueAt": null,
      "nextDueAt": null,
      "inFlight": 0,
      "maxInFlight": 16
    }
  ]
}
```

P2.1 不返回 disabled Queue/Cron/Workflow 的 fake zero rows；capabilities 单独说明它们尚未启用。

### 12.3 health

- Alarm pool healthy + kernel running：scheduler healthy；
- Alarm pool paused：healthy with operator state；
- Alarm pool circuit-open：scheduler degraded；
- scheduler DB migration/checksum/corruption：unhealthy；
- Queue/Cron/Workflow 未启用：不是 degraded。

## 13. Fault injection 与 crash matrix

### 13.1 fault points

固定枚举，且只在 `cfg(test)` 或显式 test-support feature 中可用：

```rust
enum SchedulerFaultPoint {
    AfterClaimCommit,
    BeforeDispatch,
    AfterDispatchBeforeComplete,
    AfterCompleteCommit,
    DuringProjectionRefresh,
}
```

fault hook 可以 barrier/panic/abort test process，但不能：

- 编译进普通 production binary；
- 通过 tenant 或 operator HTTP 请求任意触发；
- 接受任意 SQL、path 或 shell command；
- 改变 release capability hash 而不被 Gate 发现。

### 13.2 外部 crash harness

真正的 durability Gate 使用 child process：

1. 固定 test data-dir；
2. 等待结构化 marker 表示到达 fault point；
3. 外部 harness 发送 SIGKILL；
4. 使用同一 data-dir 启动新进程；
5. 推进 virtual clock 或等待受控 deadline；
6. 验证 authority、projection、token 和 delivery count；
7. 每个 case 使用 fresh process，避免内存状态泄漏。

P2.1 先用 Alarm 覆盖所有边界；P2.2/P2.3 再复用相同 fault enum 扩 Queue case。

## 14. 实现工作包

### P2.1.0：基线锁定

- 保存 P1 aggregate、P0.8 alarm 三轮和 G0 allowlist 结果；
- 记录当前 scheduler schema/version/checksum、config default 与 metrics contract；
- 增加“P2.1 不创建 Queue/Cron/Workflow row/API”的 negative test。

### P2.1.1：migration registry

- 重构 scheduler migration registry，不改 version 1 数据；
- 增加 gap、duplicate、checksum drift、unknown future version 测试；
- 验证旧 P1 data-dir 原地启动后 schema bytes 与 alarm rows 不变。

### P2.1.2：clock 与 WakeCoordinator

- 把所有 scheduler timer/timeout/repair/drain 收口到 clock；
- 实现 generation-safe notify；
- 实现 earliest-due 与 safety reconcile deadline；
- 用 virtual clock 覆盖前跳、后跳、同 deadline 和 lost-wake race。

### P2.1.3：pool 与 fairness

- 引入 fixed kind、global/pool admission；
- 实现 deterministic weighted deficit round-robin；
- 先只接 synthetic adapter 做算法测试；
- 证明 work-conserving、bounded starvation 和 cap invariants。

### P2.1.4：Alarm adapter

- 把当前 Alarm claim/dispatch/complete/repair 接入新 kernel；
- 保留 SQL、token、lease、retry schedule 和 operator行为；
- current P0.8 tests 不改预期即可通过。

### P2.1.5：failure isolation

- 基础设施 backoff/jitter；
- pool circuit state；
- unknown dispatch lease retention；
- pool-local error 不阻断 kernel 的其他 synthetic pool。

### P2.1.6：fault harness

- 加入固定 test-only fault points；
- child-process SIGKILL/restart；
- Alarm 五个 crash 边界 + shutdown/hung dispatch；
- production feature/binary scan 确认 fault path 不存在。

### P2.1.7：operator、metrics 与 release Gate

- versioned inspect、kind pause/resume、low-cardinality metrics；
- capabilities/release identity 更新；
- doctor/support bundle 更新；
- 三轮 aggregate 和 P1/P0/G0 regression。

## 15. 测试矩阵

### 15.1 单元测试

- selector：空 pool、单 pool、四 pool、动态 ready、不同 weight、cap exhausted；
- wake：notify before wait、notify during query、notify after waiter、deadline earlier/later；
- clock：wall 前跳/后跳、monotonic 单调、多个 timer；
- backoff：cap、reset、deterministic jitter；
- config：旧配置、零值、过大值、未发布 pool enabled；
- migration：连续、checksum、future version；
- outcome：unknown 保留 lease、stale complete no-op；
- metrics enum/label cardinality。

### 15.2 SQLite/process 测试

- 旧 scheduler v1 DB 无数据变化升级；
- claim transaction 完成后 connection 不被 dispatch 持有；
- 两个 concurrent claim 不重复拿同一 alarm；
- expired lease recovery 与旧 token complete；
- pause/restart/shutdown；
- SIGKILL 五个 fault point；
- quick_check/checksum/corruption fail closed。

### 15.3 synthetic fairness 测试

| case | 输入 | 断言 |
| --- | --- | --- |
| F-01 | Queue 无限 ready，Alarm 周期 ready | Alarm 在 bounded rounds 内执行 |
| F-02 | Queue dispatch 永久 pending | 只占 Queue cap；其他 pool 仍进展 |
| F-03 | 三个 pool idle，一个 ready | ready pool 可吃满 global permits |
| F-04 | 四 pool 持续 ready | claim 比例收敛到配置 weight |
| F-05 | pool claim 返回少于 limit | 未使用 budget 在同轮给其他 pool |
| F-06 | pool SQLite temporary busy | backoff 不阻塞其他 pool |
| F-07 | pool permanent schema error | 该 pool circuit-open，其他 pool运行 |
| F-08 | 同 seed 重放 | selector 与 jitter 序列一致 |

### 15.4 Alarm regression

必须原样覆盖 P0.8：

- number/Date/invalid input、past due、overwrite、delete；
- transaction commit/rollback/coalesce；
- stale authority/projection、read/activation/scan repair；
- execution/object generation、row token 与 claim token fence；
- transport unknown、六次 retry、exhaustion；
- promotion/rollback、deleteAll；
- cold first event、workerd restart、platform restart；
- 普通 Workers/KV/R2/D1/DO 在 scheduler degraded 时不受影响。

## 16. Exit Gate

P2.1 只有满足以下条件才可进入 P2.2：

- [ ] production composition 只注册 Alarm；没有 Queue/Cron/Workflow row、API或 binding；
- [ ] `scheduler.sqlite` schema version 仍为 1，旧 checksum 和 alarm rows保持；
- [ ] migration registry 连续、checksum-exact，可接受后续真实 migration 002；
- [ ] global + pool caps 在 property/synthetic test 中从未超限；
- [ ] 四 workload synthetic backlog 下不存在 starvation；
- [ ] 空闲 pool 不浪费 global capacity；
- [ ] 所有 scheduler timers 可由 virtual clock 推进，无真实 sleep 型长测试；
- [ ] lost-wake、wall 前跳/后跳和 restart 首轮 recovery 通过；
- [ ] unknown dispatch 保留 lease，stale completion token-exact no-op；
- [ ] Alarm handler retry 仍精确为既有六次 schedule；
- [ ] test-only fault path 不存在于 production binary；
- [ ] 五个 crash boundary fresh-process recovery 通过；
- [ ] pause/resume/drain/hung dispatch/circuit isolation 通过；
- [ ] metrics label 固定低基数，inspect/health/capabilities versioned；
- [ ] P0.8 scheduler/Alarm Gate 连续三轮通过；
- [ ] P0 aggregate、P1 aggregate 与 `./poc/g0 test all` regression 通过；
- [ ] format、Clippy、unit/integration、MSRV、no-default-features、dependency boundary、
      `git diff --check` 与 coverage Gate 通过。

建议新增入口：

```bash
./scripts/test-p2-1.sh
```

该脚本负责 P2.1 fresh-process 三轮、P0.8 至 P0.2 regression、P1 aggregate 所需本地验证以及 G0。
不增加 CI、Codecov、上传或远端依赖；沿用当前仓库的本地-only 交付方式。

## 17. 完成定义

P2.1 完成不是“把 scheduler 文件拆开”或“加了一个 Queue enum”。完成时必须能够证明：

1. 现有 Alarm 通过新 kernel 运行，行为与 P0.8 一致；
2. synthetic Queue/Cron/Workflow workload 在公平性、pool isolation 和 crash harness 中可组合运行；
3. production 尚未启用这些产品；
4. P2.2 可以只增加 Queue catalog/message schema 和 Queue adapter，而无需再次重写主循环；
5. 任何 workload 的错误都能被定位、暂停、恢复，并且不会默默破坏其他 workload；
6. 所有时间与崩溃语义都有可重复、无需真实长等待的测试证据。

这样 P2.2 只需解决 Queue 自己的 authority、producer API 和 durable commit，P2.3 再解决 consumer
delivery；scheduler 基础设施不会和每个产品同时反复变化。
