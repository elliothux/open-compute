# P2.4：Workflow Core 详细设计

> 状态：已实现并完成最终验收；P2.4 为 Conditional Go（DO 内 create 按 output-gate 结论 fail closed）。最终结论与证据见 [P2.4 Gate 结果](./p2-4-gate-results.md)。
>
> 前置基线：P2.1、P2.2 已由用户确认跑通；P2.3 按
> [Queue Consumer 与 Cron 详细设计](./p2-3-queue-consumer-cron.md)完成后，Workflow 才能复用
> 已验证的 scheduler lease、custom dispatch、frozen deployment、Known/Unknown outcome 与 crash harness。
>
> 直接依赖：[P0.2：Workers Runtime](./p0-2-workers-runtime.md)、
> [P0.3：Resource 与 Binding Framework](./p0-3-resource-binding-framework.md)、
> [P1：平台加固](./p1-platform-hardening.md)、
> [P2.1：Scheduler 多 Workload 内核](./p2-1-scheduler-hardening.md)、
> [P2.3：Queue Consumer 与 Cron](./p2-3-queue-consumer-cron.md)
>
> 后续消费者：P2.5 Workflow durable waiting。P2.4 不实现 retry、sleep、event、pause/resume、
> terminate/restart、retention 或 parallel step。

P2.4 实现最小但真实 durable 的 Workflow engine：logical definition/version、Worker binding 的
`create()`/`get()`、instance `status()`、冻结的 `WorkflowEntrypoint`、从 `run()` 开始 replay、顺序
`step.do()`、step result 持久化、run-token fence 和 terminal success/error。

这里必须先接受一个架构事实：pinned stock workerd 提供 `WorkflowEntrypoint` 基类和加载/隔离能力，
但不提供可直接嵌入的完整 Cloudflare Workflows control plane、instance store 与 replay engine。后者要
像 WDL/Miniflare 一样由平台层实现。`workerLoader` 仍负责加载 immutable tenant deployment；SQLite
负责 Workflow authority；trusted facade 在 tenant isolate 内把 `step.do()` 桥接到私有 backend。

## 0. 决策摘要

| 主题 | P2.4 选择 | 原因 |
| --- | --- | --- |
| workerd 使用 | 继续复用 dynamic `workerLoader`，不为每个 instance 启新进程 | 保留 Workers/Bindings/limits 与 warm cache 一致性 |
| Workflow engine | platformd + trusted system module + `scheduler.sqlite` | stock workerd 没有完整持久化/replay 产品层 |
| Definition | logical definition + immutable version | 新 instance 使用当前 version，旧 instance 永远冻结原 version |
| Caller binding | immutable deployment Workflow binding | tenant 只能调用绑定的 definition，不能提交 trusted ID |
| Instance identity | 内部 UUID + definition 内唯一 external ID | 对齐 `create({id})` 常用语义，又不把用户字符串作为全局主键 |
| Create durability | control 先保留 live ref，再写 scheduler instance，最后 finalize | 返回前既有 durable instance，又不会让 target deployment 被 GC |
| Runtime | 每次 activation 从 `run()` 开头 replay | 不保存 V8 heap/continuation，重启后仍可恢复 |
| Step | P2.4 只支持顺序 `step.do(name, callback)` | 先把最小 replay/fence 做正确，再扩 retry/wait/parallel |
| Step identity | ordinal + name + same-name count + canonical config digest | 防止错误复用旧 result，检测非确定性分支 |
| Step output | bounded canonical JSON，1 MiB；stream/RpcSerializable 扩展暂不支持 | SQLite 可稳定存储与跨重启解码，不伪装完整 structured clone |
| Side effect | step callback at-least-once，result commit 后 replay 不再执行 callback | crash-after-side-effect-before-commit 无法给 exactly-once |
| Attempt | P2.4 product attempt 固定为 1；infra Unknown replay 不算 retry | retry/backoff 留给 P2.5，平台故障不改变 API attempt |
| Fence | instance generation + random run token + lease + step token | stale run/step commit 均 fail closed |
| Terminal | `complete` / `errored`；无自动 retry | 状态空间最小、故障可验证 |
| Instance API | `create`、`get`、`id`、`status` | 常用核心；`createBatch` 和 modifier 明确延后 |
| DO mutation | P2.4.0 独立 output-gate probe；失败则 DO 内 `create` fail closed | 不重复 P2.2 “提交早于 DO storage”问题 |
| Schema | control migration 012；scheduler migration 005 | Definition/referrer 与 high-churn run/step authority 分库 |
| Retention | P2.4 不自动删除 terminal instance | P2.5 再引入 retention；P2.4 以 local quota 限制增长 |

### 0.1 P2.4 必须守住的不变量

1. Workflow definition ID immutable；同 account live name 唯一。
2. 每个 Workflow version 冻结 ready deployment、worker、class/export、code digest 和 capability version。
3. Instance 创建时冻结 version；definition 后续更新不能迁移已有 instance。
4. Caller binding 只能引用同 account、ready/healthy definition 和精确 lifecycle generation。
5. 同一 definition 内 external instance ID 唯一；重复 `create({id})` 稳定失败。
6. `create()` 只有在 control live ref 与 scheduler instance 都 durable commit 后 resolve。
7. Live instance 为 frozen deployment 注册 typed `deployment_referrers`；GC 不能删除运行所需 artifact。
8. 同一 instance 同一时间最多一个有效 run token；所有 mutation 都验证 generation/token/lease。
9. Tenant `run()` 每次 activation 从头执行；平台不序列化 V8 heap、Promise 或 closure。
10. 已完成 step descriptor 精确匹配时只返回持久化 output，不再次执行 callback。
11. 相同 ordinal 的 name/count/config 不匹配，或 replay 提前返回，均 terminal
    `WORKFLOW_NON_DETERMINISTIC`，不能把旧 output 给新 step。
12. Step callback 执行期间不持 SQLite transaction；结果 commit 是独立短 transaction。
13. Callback side effect 后、step result commit 前 crash 时允许 callback 重复；message/HTTP/R2 等副作用
    必须由应用幂等。
14. Stale run、stale step token、旧 workerd generation 的 completion 只能 no-op。
15. Unknown dispatch/step outcome 不立即 terminal；保留 lease并在 recovery 后 replay。
16. Known callback error 在 P2.4 不自动 retry，持久化 failed step 并将 instance 收敛到 `errored`。
17. Terminal status/output/error 持久化；进程 restart、workerd restart、snapshot/restore 后相同。
18. Workflow private backend 只接受 host 注入 identity；tenant 不能覆盖 definition/version/deployment/token。
19. Payload、step result、workflow result、error 和总持久状态都有 byte/depth/step count 上限。
20. P2.4 不把未实现方法做成“成功但无效果”；统一 stable unsupported error。

### 0.2 非目标

- `step.do` retry/backoff、timeout config、`NonRetryableError`；
- rollback handler；
- `step.sleep()`、`sleepUntil()`、`waitForEvent()` 或 Workflow cron schedule；
- `sendEvent()`、pause、resume、terminate、restart、delete；
- parallel/fan-out step 或 dependency DAG；
- `createBatch()`；
- ReadableStream、Map/Set/Date/ArrayBuffer 等完整 RpcSerializable 持久化；
- terminal retention、instance reuse 或 per-instance delete；
- exactly-once callback 或 callback 外部副作用 transaction；
- Cloudflare dashboard/REST API/Wrangler 完整兼容；
- 全球 Workflow、多节点 continuation 迁移、跨 region replication；
- 在 Workflow 中隐式启动 Queue consumer 或 Cron；最终链路留到 P2 Exit Gate。

## 1. 外部 API 与参考实现

### 1.1 Cloudflare compatibility target

以当前官方 [Workers API](https://developers.cloudflare.com/workflows/build/workers-api/) 和
[Workflows limits](https://developers.cloudflare.com/workflows/reference/limits/) 为外部基线。当前 API
包含 `WorkflowEntrypoint.run(event, step)`、多种 `step.do` overload、sleep/event、rollback、
`createBatch` 和完整 instance modifier。P2.4 只宣布下面明确列出的 capability V1 子集。

Cloudflare 当前非 stream step result 和 event payload 上限均为 1 MiB，instance ID 最长 100 字符。
P2.4 对齐这两个常用边界；instance 总状态和并发使用本地更保守的 operator 配置，不复制 plan 容量。

### 1.2 WDL 与 Miniflare

重点参考：

- `references/wdl/runtime/bindings/workflow.js`：binding-scoped trusted identity；
- `references/wdl/runtime/dispatch/workflow-json.js`：bounded JSON 与错误边界；
- `references/wdl/runtime/dispatch/workflow-replay-cache.js`：replay page/cache 不是 authority；
- `references/wdl/runtime/dispatch/workflow-step.js`：ordinal、name count、claim/commit bridge；
- `references/workers-sdk/packages/workflows-shared/src/binding.ts`：Miniflare binding/handle surface；
- `references/workers-sdk/packages/miniflare/src/plugins/workflows/`：本地 engine 组合方式；
- `references/workerd/src/workerd/server/tests/python/workflow-entrypoint/`：pinned workerd
  `WorkflowEntrypoint` 基类支持证据。

可复用的是边界和测试思路，不是存储实现：WDL 的 mesh/backend、Miniflare 的 Durable Object engine
都不直接进入单 SQLite 产品。Replay cache 只能是性能优化，不能成为已完成 step 的唯一来源。

## 2. Capability V1

### 2.1 Workflow definition

```ts
import { WorkflowEntrypoint } from "cloudflare:workers";

export class OrderWorkflow extends WorkflowEntrypoint<Env, OrderParams> {
  async run(event: WorkflowEvent<OrderParams>, step: WorkflowStep) {
    const order = await step.do("load-order", async (ctx) => {
      return await this.env.DB.prepare("SELECT ...").first();
    });

    return await step.do("write-result", async (ctx) => {
      await this.env.BUCKET.put(`orders/${order.id}.json`, JSON.stringify(order));
      return { id: order.id };
    });
  }
}
```

`WorkflowEvent<T>`：

```ts
interface WorkflowEvent<T = unknown> {
  readonly payload: Readonly<T>;
  readonly timestamp: Date;
  readonly instanceId: string;
  readonly workflowName: string;
}
```

P2.4 的 `schedule` 永远不存在；Workflow Cron 后续实现。

`WorkflowStepContext`：

```ts
interface WorkflowStepContext {
  readonly step: { readonly name: string; readonly count: number };
  readonly attempt: 1;
  readonly config: null;
}
```

### 2.2 caller binding

```ts
interface Workflow<Params = unknown> {
  create(options?: { id?: string; params?: Params }): Promise<WorkflowInstance>;
  get(id: string): Promise<WorkflowInstance>;
}

interface WorkflowInstance {
  readonly id: string;
  status(): Promise<WorkflowInstanceStatus>;
}

type WorkflowInstanceStatus =
  | { status: "queued" }
  | { status: "running" }
  | { status: "complete"; output: unknown }
  | { status: "errored"; error: { name: string; message: string } };
```

`create()` 未提供 ID 时由 host 生成 UUIDv7 external ID；提供时要求 1..100 UTF-8 bytes，字符集采用
当前 Cloudflare instance validator fixture。ID 只在 definition 内唯一。

`createBatch`、`pause`、`resume`、`terminate`、`restart`、`sendEvent`、`delete` 若出现在完整 Cloudflare
typing 中，runtime 统一抛 `WORKFLOW_METHOD_UNSUPPORTED`；本项目 capability typings 不宣布这些方法。

### 2.3 step.do 支持矩阵

| surface | P2.4 | 说明 |
| --- | --- | --- |
| `step.do(name, callback)` | 支持 | 顺序执行，name 1..256 bytes |
| callback context | 支持 | `attempt=1`、name/count、config null |
| JSON primitive/array/object result | 支持 | canonical JSON，≤1 MiB |
| `undefined` result | 支持为 `null` | 固定 capability 规则 |
| config overload | 不支持 | P2.5 retry/timeout 时加入 |
| rollback options | 不支持 | stable reject |
| stream/binary/Map/Set/Date result | 不支持 | stable serialization error |
| parallel `Promise.all(step.do...)` | 不支持 | 第二个 concurrent step stable reject |
| step outside active run | 不支持 | private facade 不可构造 |

## 3. P2.4.0：Runtime Hard Gate

任何 schema 实现前，先在 pinned stock workerd 上验证最小链路。这个 Gate 决定 Workflow engine 是否
能继续复用 `workerLoader`，不能用普通 fetch 模拟结果。

### 3.1 dynamic entrypoint Gate

Probe 使用真正的 deployment artifact 与 RuntimeSource：

1. `WorkflowEntrypoint` named export 可由 immutable loader key 定位；
2. trusted dispatcher 与 tenant class 在同一 loaded isolate 中调用 `run(event, stepFacade)`；
3. `this.env` 拥有该 frozen deployment 的 KV/D1/R2/DO/Queue/vars/secrets bindings；
4. event `timestamp` 是 `Date`，payload 是隔离后的 readonly-compatible value；
5. tenant 无法取得 private backend binding、run token 或 deployment authority；
6. warm/cold loader 结果一致；换 deployment ID 一定走不同 loader key；
7. workerd restart 后能重新加载同一 frozen version；
8. `run()` return/throw、workerd abort、transport timeout 可分类 Known/Unknown；
9. `this.ctx.waitUntil` 若可用，dispatch completion 必须等待其 settlement；若不能可靠观察，capability
   文档明确不允许依赖 WorkflowEntrypoint `waitUntil`；
10. named class 不存在或没有 `run` 时 deployment/version validation fail closed。

### 3.2 step bridge Gate

Trusted facade 需要在 callback await 前后调用 private persistence service。Probe 必须证明：

1. `step.do()` 可先查询 replay row，再决定是否调用 callback；
2. callback 成功结果能在返回 tenant code 前 durable commit；
3. backend 已有 complete row 时 callback 确实不执行；
4. callback throw 能先持久化 sanitized failure，再把等价 Error 抛回 `run()`；
5. crash-after-callback-before-commit 会重跑 callback，ID/token 不被 tenant 观察；
6. stale run/step token 的 commit 被 backend 拒绝；
7. 两个并发 `step.do()` 可被 facade deterministic 拒绝；
8. result JSON size/depth/cycle/BigInt 在 facade 与 backend 双重验证；
9. private service 不可由普通 tenant fetch/service binding 访问；
10. replay 1000 个小 step 时不会把全部状态无界塞进 single RPC。

### 3.3 Durable Object output-gate Gate

Workflow binding `create()` 是外部 durable mutation。必须复刻 P2.2 的 probe：DO transaction abort
时，已 resolve 的 `create()` 是否会一起 rollback/阻止提交。若 facade/service RPC 不能继承 native DO
output gate：

- 普通 Worker `create()` 开放；
- DO 内 `create()` 抛 `WORKFLOW_DO_OUTPUT_GATE_UNSUPPORTED`，scheduler 零新增；
- `get()`/`status()` 作为只读操作可在单独 probe 后开放；
- 不因用户“通常会 await”而接受语义缺口。

### 3.4 Gate verdict

| 结果 | 结论 |
| --- | --- |
| dynamic class、step bridge、token fence 都成立 | Go |
| 普通 Worker成立、DO create output gate 不成立 | Conditional Go，精确 fail closed DO mutation |
| 无法在 loaded isolate 内提供 callback-aware step facade | No-Go |
| 只能每个 step 重新启动不同 tenant realm | No-Go，closure/env/control-flow 语义不成立 |
| stale token 可提交或 replay 会调用 completed callback | No-Go |

结果写入 `p2-4-gate-results.md`，保存 lock digest、generated config、probe artifact、stderr 与 verdict。

## 4. 总体架构

```text
Caller Worker
  env.ORDER_WORKFLOW.create/get
        │ trusted WorkflowBinding facade
        ▼
platformd Workflow control backend
  ├── control.sqlite
  │   ├── workflow_definitions / workflow_versions
  │   ├── workflow_bindings
  │   └── workflow_instance_referrers
  └── scheduler.sqlite
      ├── workflow_instances
      └── workflow_steps
                 │ P2.1 Workflow pool claim
                 ▼
workerd dynamic workerLoader
  frozen deployment + trusted dispatcher
      └── WorkflowEntrypoint.run(event, stepFacade)
             └── private claim/replay/commit step RPC
```

`platformd` 不执行 tenant JavaScript。Workerd 不拥有 durable Workflow authority。Trusted dispatcher
只做 realm 内 API facade、callback 调用与 bounded serialization；所有状态转换由 Rust repository +
SQLite trigger 决定。

## 5. Control schema：migration 012

建议新增 `crates/storage/migrations/012_workflows.sql`。

### 5.1 logical definition

```sql
CREATE TABLE workflow_definitions (
  id                    TEXT PRIMARY KEY,
  account_id            TEXT NOT NULL REFERENCES accounts(id),
  name                  TEXT NOT NULL CHECK(length(name) BETWEEN 1 AND 64),
  state                 TEXT NOT NULL CHECK(state IN (
                          'creating', 'ready', 'deleting', 'tombstoned'
                        )),
  availability          TEXT NOT NULL CHECK(availability IN (
                          'healthy', 'degraded', 'unavailable'
                        )),
  availability_code     TEXT,
  lifecycle_generation  INTEGER NOT NULL CHECK(lifecycle_generation >= 1),
  current_version_id    TEXT,
  created_at_ms         INTEGER NOT NULL,
  updated_at_ms         INTEGER NOT NULL,
  deleted_at_ms         INTEGER,
  CHECK((state = 'tombstoned') = (deleted_at_ms IS NOT NULL)),
  CHECK((availability = 'healthy') = (availability_code IS NULL))
) STRICT;

CREATE UNIQUE INDEX workflow_definitions_live_name
ON workflow_definitions(account_id, name)
WHERE state != 'tombstoned';
```

`current_version_id` 使用 deferred FK 或在 version 表创建后补 trigger 验证：version 属于同 definition
且 `ready`。Definition rename 只改 display identity，不改变 ID/lifecycle generation。

### 5.2 immutable version

```sql
CREATE TABLE workflow_versions (
  id                    TEXT PRIMARY KEY,
  definition_id         TEXT NOT NULL REFERENCES workflow_definitions(id),
  version_number        INTEGER NOT NULL CHECK(version_number > 0),
  state                 TEXT NOT NULL CHECK(state IN (
                          'staging', 'validating', 'ready', 'rejected',
                          'deleting', 'tombstoned'
                        )),
  worker_id             TEXT NOT NULL REFERENCES workers(id),
  deployment_id         TEXT NOT NULL REFERENCES worker_deployments(id),
  class_name            TEXT NOT NULL,
  worker_code_sha256    BLOB NOT NULL CHECK(length(worker_code_sha256) = 32),
  loader_schema_version INTEGER NOT NULL,
  capability_version    INTEGER NOT NULL CHECK(capability_version = 1),
  descriptor_sha256     BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms         INTEGER NOT NULL,
  ready_at_ms           INTEGER,
  rejected_at_ms        INTEGER,
  rejection_code        TEXT,
  deleted_at_ms         INTEGER,
  UNIQUE(definition_id, version_number)
) STRICT;
```

Version validation 要求：

- definition、Worker、deployment 同 account；
- deployment `ready`；
- class name 合法且 probe 可加载 `WorkflowEntrypoint`；
- exact deployment code digest/loader schema 匹配；
- capability version 受支持；
- descriptor hash 覆盖全部 frozen field。

Version ready 后除 lifecycle transition 外 immutable。Normal deploy pipeline 可在 Worker deployment
validation 时 stage 对应 Workflow version，并在 promotion 的同一 control transaction 切
`current_version_id`；旧 instance 不受影响。

当前 version 通过 `deployment_referrers(kind='workflow_version')` 保护 artifact。切换 current version
后，旧 version 只有在不再 current、没有 live instance referrer且没有 pending activation时才能释放
该 version referrer并进入 deleting。Live instance 自己的 `workflow_instance` referrer继续保护其冻结
deployment，避免切新版后旧实例正在replay但artifact被GC。

### 5.3 caller binding

```sql
CREATE TABLE workflow_bindings (
  id                            TEXT PRIMARY KEY,
  deployment_id                 TEXT NOT NULL REFERENCES worker_deployments(id),
  name                          TEXT NOT NULL,
  definition_id                 TEXT NOT NULL REFERENCES workflow_definitions(id),
  definition_lifecycle_generation INTEGER NOT NULL,
  capability_version            INTEGER NOT NULL CHECK(capability_version = 1),
  descriptor_sha256             BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  created_at_ms                  INTEGER NOT NULL,
  UNIQUE(deployment_id, name)
) STRICT;
```

Binding 与 vars/secrets/resource bindings/Queue producer 名称双向冲突检查；只可在 staging deployment
插入，之后 immutable。Definition 的统一 referrer registry 为：

```sql
CREATE TABLE workflow_referrers (
  definition_id  TEXT NOT NULL REFERENCES workflow_definitions(id),
  referrer_kind  TEXT NOT NULL CHECK(referrer_kind IN ('binding', 'instance')),
  referrer_id    TEXT NOT NULL,
  created_at_ms  INTEGER NOT NULL,
  PRIMARY KEY(definition_id, referrer_kind, referrer_id)
) WITHOUT ROWID, STRICT;
```

Binding insert/delete trigger分别建立和移除 `kind='binding'` 的 row，definition delete只查询这一份
registry。这样不会在后续增加 Workflow trigger 时漏查引用。

RuntimeSource 将 `workflow_bindings` 与现有 deployment snapshot 合并，生成 facade descriptor：

```json
{
  "kind": "workflow",
  "bindingName": "ORDER_WORKFLOW",
  "definitionId": "uuid",
  "definitionLifecycleGeneration": 1,
  "capabilityVersion": 1,
  "descriptorSha256": "..."
}
```

Tenant 只能选择 public method/options；account、definition、caller deployment、generation 和 internal
auth 由 facade props 注入。

### 5.4 live instance referrer

```sql
CREATE TABLE workflow_instance_referrers (
  instance_id           TEXT PRIMARY KEY,
  definition_id         TEXT NOT NULL REFERENCES workflow_definitions(id),
  external_instance_id  TEXT NOT NULL,
  version_id            TEXT NOT NULL REFERENCES workflow_versions(id),
  deployment_id         TEXT NOT NULL REFERENCES worker_deployments(id),
  instance_generation   INTEGER NOT NULL CHECK(instance_generation >= 1),
  creation_nonce        BLOB NOT NULL CHECK(length(creation_nonce) = 32),
  state                 TEXT NOT NULL CHECK(state IN (
                          'creating', 'live', 'releasing', 'released'
                        )),
  created_at_ms         INTEGER NOT NULL,
  updated_at_ms         INTEGER NOT NULL,
  released_at_ms        INTEGER,
  UNIQUE(definition_id, external_instance_id)
) STRICT;
```

`creating/live/releasing` 向 `deployment_referrers` 注册：

```text
kind   = workflow_instance
ref_id = internal instance UUID
```

同时向 `workflow_referrers(kind='instance')` 注册。进入 `released` 时删除两个 live referrer，但保留
`workflow_instance_referrers` 历史 row供create/get/status与后续retention使用。

Terminal completion后 instance 不再需要执行 artifact，进入 `releasing` 并删除 deployment referrer，
最终 `released`。Scheduler 仍保留 version/deployment ID 作为历史字段，因此 `status()` 不依赖 artifact。
Definition delete 被 bindings 和非 terminal instance 阻止；terminal history可在 definition tombstone 后读。

## 6. Scheduler schema：migration 005

建议新增 `crates/storage/scheduler-migrations/005_workflow_core.sql`。P2.4 不创建 events/sleep/retry 表。

### 6.1 workflow_instances

```sql
CREATE TABLE workflow_instances (
  id                     TEXT PRIMARY KEY,
  account_id             TEXT NOT NULL,
  definition_id          TEXT NOT NULL,
  definition_name        TEXT NOT NULL,
  external_instance_id   TEXT NOT NULL,
  version_id             TEXT NOT NULL,
  worker_id              TEXT NOT NULL,
  deployment_id          TEXT NOT NULL,
  worker_code_sha256     BLOB NOT NULL CHECK(length(worker_code_sha256) = 32),
  class_name             TEXT NOT NULL,
  instance_generation    INTEGER NOT NULL CHECK(instance_generation >= 1),
  state                  TEXT NOT NULL CHECK(state IN (
                           'queued', 'running', 'complete', 'errored'
                         )),
  input_json             BLOB NOT NULL,
  output_json            BLOB,
  error_json             BLOB,
  next_run_at_ms         INTEGER,
  run_token              BLOB,
  run_claimed_at_ms      INTEGER,
  run_lease_until_ms     INTEGER,
  completed_step_count   INTEGER NOT NULL DEFAULT 0 CHECK(completed_step_count >= 0),
  state_bytes            INTEGER NOT NULL CHECK(state_bytes >= 0),
  created_at_ms          INTEGER NOT NULL,
  updated_at_ms          INTEGER NOT NULL,
  terminal_at_ms         INTEGER,
  UNIQUE(definition_id, external_instance_id),
  CHECK(
    (state = 'queued' AND next_run_at_ms IS NOT NULL AND
      run_token IS NULL AND run_claimed_at_ms IS NULL AND run_lease_until_ms IS NULL)
    OR
    (state = 'running' AND next_run_at_ms IS NULL AND length(run_token) = 32 AND
      run_claimed_at_ms IS NOT NULL AND run_lease_until_ms IS NOT NULL)
    OR
    (state IN ('complete', 'errored') AND next_run_at_ms IS NULL AND
      run_token IS NULL AND run_claimed_at_ms IS NULL AND run_lease_until_ms IS NULL AND
      terminal_at_ms IS NOT NULL)
  ),
  CHECK((state = 'complete') = (output_json IS NOT NULL)),
  CHECK((state = 'errored') = (error_json IS NOT NULL))
) STRICT;

CREATE INDEX workflow_instances_due
ON workflow_instances(next_run_at_ms, created_at_ms, id)
WHERE state = 'queued';

CREATE INDEX workflow_instances_expired
ON workflow_instances(run_lease_until_ms, id)
WHERE state = 'running';
```

`state_bytes` 包含 input、step descriptor/output/error、workflow output/error 的持久化 bytes，由同一
transaction 维护，用于 per-instance quota。P2.4 默认上限建议 32 MiB、最多 1,024 step；均通过
local capability 暴露，不能硬写成 Cloudflare plan 承诺。

### 6.2 workflow_steps

```sql
CREATE TABLE workflow_steps (
  instance_id          TEXT NOT NULL REFERENCES workflow_instances(id),
  instance_generation  INTEGER NOT NULL CHECK(instance_generation >= 1),
  ordinal              INTEGER NOT NULL CHECK(ordinal >= 0),
  name                 TEXT NOT NULL,
  name_count           INTEGER NOT NULL CHECK(name_count > 0),
  kind                 TEXT NOT NULL CHECK(kind = 'do'),
  config_json          BLOB NOT NULL,
  descriptor_sha256    BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  state                TEXT NOT NULL CHECK(state IN (
                         'pending', 'running', 'complete', 'failed'
                       )),
  attempt              INTEGER NOT NULL CHECK(attempt = 1),
  run_token            BLOB,
  step_token           BLOB,
  output_json          BLOB,
  error_json           BLOB,
  started_at_ms        INTEGER NOT NULL,
  updated_at_ms        INTEGER NOT NULL,
  completed_at_ms      INTEGER,
  PRIMARY KEY(instance_id, instance_generation, ordinal),
  UNIQUE(instance_id, instance_generation, name, name_count),
  CHECK(
    (state = 'pending' AND run_token IS NULL AND step_token IS NULL AND
      output_json IS NULL AND error_json IS NULL AND completed_at_ms IS NULL)
    OR
    (state = 'running' AND length(run_token) = 32 AND length(step_token) = 32 AND
      output_json IS NULL AND error_json IS NULL AND completed_at_ms IS NULL)
    OR
    (state = 'complete' AND run_token IS NULL AND step_token IS NULL AND
      output_json IS NOT NULL AND error_json IS NULL AND completed_at_ms IS NOT NULL)
    OR
    (state = 'failed' AND run_token IS NULL AND step_token IS NULL AND
      output_json IS NULL AND error_json IS NOT NULL AND completed_at_ms IS NOT NULL)
  )
) WITHOUT ROWID, STRICT;
```

`config_json` 在 P2.4 固定 canonical `null`，但仍参与 descriptor，给 P2.5 forward migration 留下
稳定 replay identity。Step output 不与 input 重复存储。

### 6.3 trigger

DB trigger 至少保证：

- identity/frozen fields immutable；
- queued→running 只设置 fresh token/lease；
- running→queued 只由 exact generation/token recovery，清 token；
- running→terminal 只由 exact generation/token completion；
- terminal immutable；
- step insert/update 只在 parent instance exact running token 下发生；
- running step 在 run lease recovery 时只能转为 `pending`，保留 descriptor、清除旧 token；
- complete/failed step immutable；
- ordinal 不能越过 `completed_step_count` 多于 1；
- quota counter 与 step row 同 transaction；
- instance delete P2.4 禁止，除测试 fixture/migration teardown。

Trigger 不能替代 repository token predicate；两层都要有。

## 7. Definition/version 生命周期

### 7.1 create/update

```text
definition creating
  -> version staging
  -> probe validating
  -> version ready
  -> definition ready/healthy + current_version
```

更新只创建新 version，不修改旧 version。切 `current_version_id` 是 control transaction 的唯一
linearization point。Instance create 在同一 read transaction 冻结 current version 与 deployment digest。

如果新版 probe 失败：version `rejected`，definition 继续使用旧 current version。不能因坏 deployment
让已有 definition unavailable。

### 7.2 delete

Definition 删除前要求：

- 没有 workflow binding referrer；
- 没有 creating/live/releasing instance referrer；
- 没有 pending version activation；
- terminal scheduler history允许保留，但 definition先 tombstone，名字之后可被新 ID 重建。

Version artifact 删除受 deployment referrer registry 保护。Old version 即使不是 current，只要仍有 live
instance 就不可删除。

## 8. Instance create/get/status

### 8.1 create validation

Binding backend 顺序验证：

1. private auth/startup generation/caller deployment；
2. binding descriptor hash 与 lifecycle generation；
3. definition `ready/healthy`，current version `ready`；
4. ID 规则与 params canonical JSON ≤1 MiB；
5. account/workflow queued/running/total-state quota；
6. disk admission；
7. DO mutation capability verdict。

Tenant 提交的 `definitionId`、version、class、deployment、generation 字段一律忽略/拒绝；public body只
允许 `id` 与 `params`。

### 8.2 cross-database create protocol

Control 与 scheduler 无跨文件 transaction，使用可恢复 saga：

1. 生成 internal instance UUID、external ID、32-byte creation nonce；
2. control transaction 插入 `workflow_instance_referrers(state='creating')`，冻结 version/deployment，
   也建立 deployment referrer；这一步保留 external ID；
3. scheduler transaction 插入 `workflow_instances(state='queued', generation=1)`；
4. control transaction compare nonce 将 referrer 改 `live`；
5. scheduler wake；
6. durable response 返回 handle。

Crash recovery：

- 只存在 creating ref：若 scheduler 有 exact instance/nonce-derived identity，finalize live；否则超过
  bounded grace 后删除 reservation/referrer；
- scheduler 有 instance且 control ref仍 creating：finalize；
- scheduler 有 instance但 control ref缺失：实例不可 claim，reconciler恢复 ref或标 unavailable；
- response 丢失后 user-provided ID retry得到 duplicate error；调用方可 `get(id)`；
- host-generated ID 在 response 丢失时调用方不知道 ID，这是 Cloudflare `create()` 无 idempotency key 的
  固有限制；建议业务传 stable ID。

`create()` 不在 scheduler insert 前返回。WDL/Miniflare 可异步初始化的行为不能覆盖本项目 durability
合同。

### 8.3 get/status

`get(id)` 只验证格式与 definition scope；不存在抛 stable `WORKFLOW_INSTANCE_NOT_FOUND`。返回 handle
不读取完整 step history。

`status()` 从 scheduler authority 返回：

| DB state | tenant status | 额外字段 |
| --- | --- | --- |
| queued | queued | 无 |
| running | running | 无 token/lease |
| complete | complete | parsed output |
| errored | errored | sanitized `{name,message}` |

不暴露 internal UUID、deployment ID、version ID、run token、step token、SQL error、absolute path 或
tenant stack。Operator surface可查看 frozen target 与低基数 error code。

## 9. Workflow scheduler claim

P2.1 Workflow pool 在 global/pool admission 后按 `next_run_at_ms, created_at_ms, id` claim。一个
`BEGIN IMMEDIATE`：

1. bounded recover expired runs；
2. 验证 control-ref projection cache/definition availability；
3. 选择 due queued instance；
4. 生成 random 32-byte run token；
5. state→running，冻结 lease；
6. commit 后 dispatch。

Frozen deployment/class已在 instance row，不能在每次 replay重读 definition current version。

### 9.1 lease heartbeat

Workflow run可能比 Queue handler长。使用 60 秒默认 lease、20 秒 heartbeat：

- transport task仍活跃时，platformd用 exact generation/run token续租；
- 每次 step backend call也顺带续租；
- heartbeat transaction短且不修改 step语义；
- process crash后无 heartbeat，lease最终过期；
- transport Unknown时停止主动 completion，等 lease；
- late heartbeat/completion因 token stale no-op。

配置必须满足 `heartbeat < lease / 2 < dispatch_timeout`，启动时校验。Virtual clock覆盖 lease边界。

### 9.2 recovery

Expired running instance：

1. exact token将 state改 queued、清 run token，`next_run_at` 使用 infra backoff；
2. 属于该 token 的 running step转为 `pending`，保留 descriptor但清 token，允许新 run重新 claim；
3. complete/failed step不变；
4. product attempt不增加；
5. stale workerd late commit被拒绝。

Repeated infra failure进入 Workflow pool circuit/operator告警，但不自动把 tenant instance标 errored。
只有明确的 tenant result、quota/serialization violation或可证明的永久 frozen deployment错误才 terminal。

## 10. Trusted dispatcher

### 10.1 run envelope

Private envelope：

```json
{
  "instanceId": "internal-uuid",
  "externalInstanceId": "order-2026-001",
  "instanceGeneration": 1,
  "runToken": "opaque private bytes",
  "definitionName": "order-workflow",
  "versionId": "uuid",
  "deploymentId": "uuid",
  "className": "OrderWorkflow",
  "createdAtMs": 1787700000000,
  "payloadJson": "{...}"
}
```

Run token 只存在 system isolate 的 trusted dispatcher/controller，不传给 tenant `event` 或 `step context`。
step token 同样不跨入 tenant realm；controller 持有当前顺序 step 的 grant，替 tenant facade 注入
commit token，只返回无令牌的执行/replay verdict。不能仅依赖闭包隐藏异步返回值：恶意 tenant 可以
修改 Promise intrinsics 观察它。controller 是 request-scoped native `RpcTarget`，调用结束显式销毁，
丢失后只能依靠 SQLite lease recovery，不形成内存 authority。

### 10.2 realm 内流程

```text
load frozen deployment/class
  -> construct WorkflowEntrypoint with frozen env/context
  -> create trusted StepController
  -> call run(event, stepFacade)
  -> await run + required in-flight checks
  -> verify replay frontier
  -> serialize bounded output
  -> token-exact terminal commit
```

Tenant 不能 import StepController 实现或直接构造 facade。System module name/private binding name使用保留
prefix，并在 deployment module validation 阶段拒绝冲突。

### 10.3 dispatch result

Workerd private response使用有限 enum：

```text
complete(outputJson)
errored(sanitizedError, errorCode)
non_deterministic(detailsDigest)
unsupported(code)
unknown  # transport层，不由tenant构造
```

Tenant throw只保存安全 name/message 与内部低基数 code；完整 stack只进入受限、bounded runtime log，
不写 status DB。

## 11. Step identity 与 replay

### 11.1 deterministic identity

每次 run activation 初始化：

```text
ordinal = 0
nameCounts = {}
activeStep = none
```

每次 `step.do(name, callback)`：

1. 验证 name 非空、≤256 UTF-8 bytes；
2. 同名 count + 1；
3. config canonical JSON = `null`；
4. descriptor = hash(kind, ordinal, name, nameCount, configJson)；
5. 查询 exact instance/generation/ordinal row。

结果：

| persisted row | descriptor | 动作 |
| --- | --- | --- |
| 不存在 | — | claim new step，执行 callback |
| pending | exact | 以当前 run/新 step token claim，执行 callback |
| complete | exact | parse output，callback不执行 |
| failed | exact | 重建 sanitized Error并抛出，callback不执行 |
| running current token | exact | 幂等返回同一执行 grant |
| running old token | exact | 当前 run不得抢占，先由lease recovery转pending |
| 任意 | mismatch | terminal `WORKFLOW_NON_DETERMINISTIC` |

只用 name作为 key不够：循环中相同 name合法出现多次；只用 ordinal也会在分支变化时错误复用结果。

### 11.2 replay frontier

`run()`返回前，dispatcher报告本次看到的 final ordinal。Backend 要求：

- 至少一个 step；
- 所有 `0..completed_step_count-1` persisted row都已被 exact replay；
- 没有 active step promise；
- final ordinal不小于 persisted frontier；
- 所有新 step均 terminal complete，不能仍 running。

若旧运行完成三个 step，新 replay因 `Date.now()` 分支只访问两个就返回，必须报 non-deterministic，
不能把 workflow标 complete。

### 11.3 允许的 JavaScript control flow

`if`、loop、try/catch可以使用，但稳定性由用户负责：

- 基于 event payload或已持久 step output分支是可 replay 的；
- 在 step外调用 `Date.now()`、`Math.random()`、fetch或写外部系统会在每次 replay重复；
- 若它改变 step序列，平台检测并 terminal；
- 若它产生副作用但序列未变，平台无法撤销；文档要求副作用放进 step callback并幂等。

## 12. Step claim/execute/commit

### 12.1 claim-step transaction

Trusted facade调用 private backend，backend验证 internal auth、instance generation/run token、lease、
ordinal/descriptor与quota：

- complete/failed exact row：直接返回 replay result；
- 不存在：插入 running row，生成 random 32-byte step token，返回 `run` grant；
- pending exact row：改为 running并写当前 run/新 step token，返回 `run` grant；
- running同 token：幂等返回同 grant；
- running旧 token：当前 run不得抢占；只有run lease recovery先将它转为pending后才可新claim；
- mismatch：返回 non-deterministic，不执行 callback。

Callback只有拿到 `run` grant后执行。Tenant看不到 grant token。

### 12.2 callback execution

P2.4只允许一个 active callback：

```js
const first = step.do("a", async () => 1);
const second = step.do("b", async () => 2); // stable concurrent-step error
await Promise.all([first, second]);
```

单纯忘记 await也会触发同一保护。这样先验证顺序 replay；P2.5最后再加入 parallel dependency frontier。

Callback可以使用 frozen `this.env` bindings，但不同产品之间没有原子 transaction：

- `KV.put()` commit后 platform crash、step result未 commit → replay会再次 put；
- Queue `send()` 可能产生重复 message；
- R2/HTTP/DO side effect同理；
- D1只在其单资源 transaction内原子，不能与 workflow step row原子。

推荐业务使用 `{instanceId, stepName, nameCount}` 作为外部 idempotency key。

### 12.3 success commit

Facade先在 tenant realm序列化并检查，再把 exact JSON发给 backend。Backend重新解析/规范化、检查
≤1 MiB、depth、state quota后：

```sql
UPDATE workflow_steps
SET state='complete', output_json=:json,
    run_token=NULL, step_token=NULL, completed_at_ms=:now
WHERE instance_id=:id
  AND instance_generation=:generation
  AND ordinal=:ordinal
  AND state='running'
  AND run_token=:run_token
  AND step_token=:step_token
  AND EXISTS (
    SELECT 1 FROM workflow_instances
    WHERE id=:id AND state='running'
      AND instance_generation=:generation
      AND run_token=:run_token
      AND run_lease_until_ms>:now
  );
```

同一 transaction递增 `completed_step_count/state_bytes`。Commit成功后 facade才把 value返回 tenant
`run()`。Response丢失时同一/下次 replay读 complete row，不重跑 callback。

### 12.4 failure commit

Callback known throw：sanitize `{name,message}`，token-exact将 step设 failed。P2.4无 product retry，
facade重新抛 sanitized-equivalent Error；`run()`可以 catch它，但 failed step仍是 durable failure。

为避免用户 catch后把 workflow伪装成功，StepController记住 terminal step failure：即使 `run()`返回，
dispatcher仍提交 instance `errored`。这与 WDL 的 `terminalStepFailure` 思路一致。

如果 commit-step-error response Unknown，保留 running instance/step lease；recovery后 callback可能重跑。

## 13. JSON codec 与 quota

### 13.1 canonical subset

P2.4 payload/output支持：

- `null`、boolean、finite number、string；
- array；
- plain object或 null-prototype object；
- 对象 key按 UTF-8 byte排序生成 canonical JSON；
- object中 `undefined`/function/symbol字段忽略，array中变 `null`；top-level `undefined`变 `null`；
- `NaN`/Infinity按 JSON规则变 `null`。

拒绝：BigInt、cycle、unpaired surrogate、超深 nesting、非 plain exotic object、stream、locked body、
Map/Set/Date/ArrayBuffer（payload中的 Date经调用方 JSON序列化后为string不算 Date round-trip）。

Facade与Rust backend必须共享fixture，不能各自实现略有差异的“JSON-like”。可参考WDL的bounded
stringifier，但Rust是最终authority。

### 13.2 limits

建议默认：

| 项目 | 上限 |
| --- | --- |
| params | 1 MiB UTF-8 canonical JSON |
| 单 step result | 1 MiB |
| workflow final result | 1 MiB |
| error name | 128 bytes |
| error message | 4 KiB |
| JSON container depth | 127 |
| step name | 256 bytes |
| steps per instance | 1,024 |
| total persisted state per instance | 32 MiB local default |
| queued/running instances | operator-configured local quota |

Cloudflare current total instance state可更大，但本地 SQLite SMB default必须以容量测试为准。返回
capability时明确“local limit”，不借用Cloudflare plan名称。

## 14. Terminal completion 与 referrer release

### 14.1 success

`run()`正常返回且StepController检查通过：

1. serialize final output；
2. scheduler transaction exact generation/run token：state→complete、写 output/terminal time、清 lease；
3. control referrer进入 releasing；
4. 删除 deployment referrer；
5. control row进入 released；
6. `status()`可立即读 complete，即使 referrer release稍后完成。

Crash在2后3前不会重复运行，因为scheduler已terminal；reconciler只补release。

### 14.2 error

以下是 known terminal error：

- callback known throw；
- `run()`在step外throw；
- non-deterministic replay；
- unsupported step/config/concurrency；
- payload/result永久serialization/quota错误；
- frozen deployment artifact经integrity验证永久损坏。

Runtime unavailable、process crash、response timeout、temporary disk busy不是tenant terminal error；这些保留
lease/requeue或进入platform availability/circuit。

### 14.3 no automatic retention

P2.4 terminal row和step永久保留，直到P2.5 retention上线。为防无限增长：

- account/definition total instance count与state bytes quota；
- nearing quota metrics/doctor；
- create在disk admission失败时fail before reservation；
- operator不能手工删单row绕过referrer；紧急离线repair必须通过受审计工具。

## 15. Definition update、restart 与 snapshot

### 15.1 version freeze

Instance A创建于version 1后，definition切version 2：

- A继续loader key `(account, worker, deployment-v1)`；
- B冻结version 2；
- V1 artifact由A live ref保护；
- A terminal后可release ref并参与artifact GC；
- status history不重新加载V1。

这是 Workflow 正确性比“始终执行active Worker”更重要的地方。

### 15.2 process/workerd restart

- replay cache全部丢失不影响authority；
- running instance等lease recovery后queued；
- complete step从SQLite读取；
- running step callback可能重跑；
- startup/execution generation使旧workerd private response认证失败；
- loader从immutable artifact重建相同class/env。

### 15.3 snapshot/restore

P1 maintenance snapshot先停止新create/claim，bounded drain，再一起snapshot control/scheduler。备份按既有
决定不考虑保密性，但manifest/checksum/runtime lock/schema version仍必须验证。

Fresh-host restore后：

1. creating instance saga reconcile；
2. running lease recovery；
3. live deployment referrer与scheduler非terminal instance对账；
4. complete step replay fixture验证；
5. old startup generation response不可提交；
6. S3中的frozen artifact按digest重新materialize；
7. external KV/D1/DO/R2 side effect不随Workflow回滚，文档明确这一点。

## 16. Reconciler

每轮bounded并可从operator触发：

1. creating ref ↔ scheduler instance；
2. live/releasing ref ↔ instance terminal state；
3. deployment referrer存在性；
4. expired running instance/step；
5. step ordinal连续性、descriptor与completed count；
6. state_bytes counter抽样重算；
7. queued next_run与Workflow pool wake；
8. frozen artifact availability/digest；
9. stale execution generation；
10. terminal row不可变检查。

Repair规则：

- 能从authority确定的projection/referrer可重建；
- complete/failed step output/error不可猜测；
- descriptor mismatch标unavailable/terminal corruption，不删除后重跑；
- orphan scheduler instance若能证明从未对外finalize且无live ref，可隔离后清理；否则fail closed并告警。

## 17. Error、metrics 与 operator surface

### 17.1 stable errors

至少定义：

```text
WORKFLOW_NOT_FOUND
WORKFLOW_NOT_READY
WORKFLOW_VERSION_NOT_READY
WORKFLOW_BINDING_STALE
WORKFLOW_INSTANCE_ID_INVALID
WORKFLOW_INSTANCE_ALREADY_EXISTS
WORKFLOW_INSTANCE_NOT_FOUND
WORKFLOW_PAYLOAD_TOO_LARGE
WORKFLOW_RESULT_TOO_LARGE
WORKFLOW_SERIALIZATION_UNSUPPORTED
WORKFLOW_STATE_QUOTA_EXCEEDED
WORKFLOW_STEP_LIMIT_EXCEEDED
WORKFLOW_STEP_CONFIG_UNSUPPORTED
WORKFLOW_PARALLEL_STEP_UNSUPPORTED
WORKFLOW_METHOD_UNSUPPORTED
WORKFLOW_NON_DETERMINISTIC
WORKFLOW_RUN_STALE
WORKFLOW_STEP_STALE
WORKFLOW_DO_OUTPUT_GATE_UNSUPPORTED
WORKFLOW_RUNTIME_UNAVAILABLE
```

Tenant error message不带internal UUID/token/SQL/path/private URL；operator日志可用request ID关联。

### 17.2 metrics

```text
open_compute_workflow_instances_total{outcome}
open_compute_workflow_instance_status{status}
open_compute_workflow_runs_total{outcome}
open_compute_workflow_run_seconds{outcome}
open_compute_workflow_steps_total{outcome}
open_compute_workflow_step_seconds{outcome}
open_compute_workflow_replay_steps_total{outcome}
open_compute_workflow_stale_commits_total{kind}
open_compute_workflow_in_flight
open_compute_workflow_queue_lag_seconds
open_compute_workflow_state_bytes
open_compute_workflow_reconcile_total{outcome}
```

Definition/instance/version/deployment/class/error字符串都不作为metrics label。

### 17.3 operator API

Authenticated inspect返回：

- definition/version state、frozen deployment/class、referrer count；
- instance internal/external identity、generation/status、step count/state bytes、lease age；
- step ordinal/name/count/status/output bytes/error code，不返回output/payload内容；
- Workflow pool admission、oldest queued age、expired lease、circuit；
- reconcile/repair结果与request ID。

不提供任意“把instance改complete”或SQL endpoint。Pause/terminate属于P2.5产品状态机，不用operator
直接改row替代。

## 18. Crash matrix

| Crash point | 恢复后要求 |
| --- | --- |
| control creating ref commit前 | 无ID reservation、无scheduler instance |
| creating ref commit后、scheduler insert前 | reconciler完成或安全释放，target deployment不被GC |
| scheduler insert commit后、ref finalize前 | finalize live，instance只存在一次 |
| create response前 | user stable ID可get；不重复instance |
| run claim transaction中 | queued或running二选一，无半token |
| run claim commit后、dispatch前 | lease后replay，无step丢失 |
| tenant run开始、首step claim前 | replay run，side effect outside step可能重复并有文档 |
| step claim commit后、callback前 | recovery重跑callback |
| callback side effect后、result commit前 | callback可能重复，external idempotency测试 |
| step result commit后、facade response前 | replay直接读result，callback不再执行 |
| failed step commit后、instance error commit前 | replay failed row并收敛errored |
| last step commit后、run output commit前 | replay全部step，callback零执行，再commit output |
| terminal scheduler commit后、ref release前 | status terminal，reconciler补release |
| lease过期后旧workerd late result | stale no-op，不覆盖新run |
| definition切version时 | old/new instance各自冻结正确deployment |
| snapshot时 | maintenance fence后control/scheduler一致，restore可replay |

每个fault point要分别测Known与Unknown transport，不只测函数返回Error。

## 19. 工作包

### P2.4.0 Runtime Hard Gate

- dynamic WorkflowEntrypoint probe；
- trusted step bridge、completed replay、stale token；
- JSON boundary、warm/cold/restart；
- DO create output-gate verdict；
- 固化results文档。

### P2.4.1 Definition/version/binding

- migration 012；
- lifecycle、version validation、current switch；
- caller binding/env conflict/RuntimeSource descriptor；
- deployment/definition referrer与isolation。

### P2.4.2 Instance create/get/status

- migration 005 instance table；
- cross-DB creation saga/reconciler；
- ID/payload/quota/disk admission；
- ordinary Worker与DO capability behavior；
- status codec。

### P2.4.3 Run dispatch 与 step.do

- Workflow pool claim/heartbeat/recovery；
- trusted dispatcher；
- step table/claim/execute/success/failure；
- frozen env bindings；
- sequential/concurrent rejection。

### P2.4.4 Replay 与 fencing

- ordinal/nameCount/config descriptor；
- complete/failed replay；
- frontier/non-determinism；
- run/step stale token、old execution generation；
- paged replay/cache-as-optimization。

### P2.4.5 Terminal/referrer/reconcile

- complete/error transaction；
- deployment ref release；
- restart/snapshot/restore；
- quota/counter/repair；
- operator/doctor/metrics。

### P2.4.6 Aggregate Gate

- full crash matrix；
- version switch/GC；
- Queue/Cron/Alarm/Workflow fairness；
- P0/P1/P2 regression；
- coverage与results文档。

## 20. 测试矩阵

开发、审查与修复期间每次只跑一轮相关 Gate：`OPEN_COMPUTE_GATE_ROUNDS=1 ./test/test-p2-4.sh`。
实现收尾、源码冻结后才运行最终三轮及相关 aggregate/coverage；若还需改代码，先回到单轮反馈，
不要在中间迭代重复整条历史回归链。具体命令和证据口径见 [Gate 验证节奏](./testing.md)。

### 20.1 Definition/binding

- create/update/rejected version/current switch；
- missing class、not WorkflowEntrypoint、missing run；
- binding warm/cold descriptor一致；
- binding/var/secret/resource/Queue name冲突；
- cross-account definition/version/deployment spoof；
- live instance阻止artifact deletion；terminal release后可GC；
- rollback promotion仍冻结正确version。

### 20.2 Instance API

- generated ID、stable user ID、empty/101-byte/invalid ID；
- duplicate create、response loss后get；
- params absent/null/1MiB/beyond/cycle/BigInt/depth；
- create saga三个commit边界；
- get missing/invalid/cross-definition；
- queued/running/complete/errored status；
- unsupported createBatch/modifier；
- ordinary Worker与DO output-gate结果。

### 20.3 run/step/replay

- one step、multiple sequential、loop same name count；
- callback context attempt/name/count/config；
- callback return每种JSON值；
- complete replay callback count保持1；
- callback throw、run throw、caught step error仍terminal；
- crash before/after step commit、before terminal commit；
- step name/config/ordinal mismatch；
- replay提前return/额外step；
- `Date.now()`/random分支non-deterministic fixture；
- two concurrent steps stable reject；
- stale run/step token、lease heartbeat/expiry；
- 1,024 steps、1,025 reject、state byte quota；
- 1 MiB result与边界surrogate/depth/cycle。

### 20.4 integration

- Workflow step读取KV/D1/R2、调用DO、Queue send；
- external side effect幂等key重复fixture；
- Workflow backlog不饿死Queue/Cron/Alarm；
- workerd warm/cold/restart、platformd SIGKILL；
- snapshot/fresh-host restore；
- upgrade from P2.3 populated fixture；
- artifact cache eviction后从S3按digest恢复；
- G0 exact `D-abort` allowlist不扩大；
- P2.2 DO producer fail-closed维持。

## 21. Exit Gate

P2.4 只有满足以下条件才为 Go：

1. Hard Gate证明dynamic WorkflowEntrypoint与callback-aware step bridge可在生产loader路径运行；
2. `create()`返回前control ref与scheduler instance均durable；
3. complete step在所有restart/crash fixture中不再执行callback；
4. crash-after-side-effect-before-commit明确产生at-least-once，并有stable instance/step幂等identity；
5. non-deterministic replay不会错误复用output；
6. 所有instance/step terminal mutation有generation/run-token/step-token fence；
7. definition version切换时已有instance始终使用frozen deployment/class；
8. stale workerd completion、expired lease、snapshot前旧generation均无法提交；
9. create saga、referrer release、reconciler在每个crash boundary收敛；
10. payload/result/state quota与disk admission fail closed；
11. Workflow backlog下Queue/Cron/Alarm仍满足P2.1 fairness；
12. upgrade、restart、fresh-host restore、aggregate regression与coverage gate通过。

允许的 Conditional Go 只能是精确surface，例如“普通 Worker create可用，DO内create因output gate
fail closed”。以下缺口不能 Conditional Go：completed step会重复执行、stale token可提交、version未冻结、
create返回后instance可能不存在、non-determinism错误复用result。

P2.4通过后，P2.5按依赖顺序扩展同一状态机：先step retry/backoff，再sleep/sleepUntil，再event，
随后instance modifier与retention，最后parallel step。不得在P2.4 Gate未稳定时一次性加入所有等待状态。

P2.4 的实际实现结论见 [Gate 结果](./p2-4-gate-results.md)。后续已在
[P2.5：Workflow Durable Waiting 详细设计](./p2-5-workflow-durable-waiting.md) 中细化：先验证
system-isolate controller 的 suspension/timeout，再实现 capability V2 与公共 yield/wake；其后按上述
顺序逐项接入。P2.4 V1 history、failure latch、descriptor/hash 和 DO output-gate 限制不隐式改写。
