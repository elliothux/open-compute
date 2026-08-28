# P2.5：Workflow Durable Waiting 详细设计

状态：已实现并完成最终验收，**P2.5 Conditional Go；P2 Exit Gate PASS**。2026-08-28。
基线为 `9fe5b4a47a2136ff27a02989b4c8481c09bf412b`，加本次 worktree。
实际命令、逐轮计数、失败修正和输出路径见 [最终验证记录](./p2-5-gate-results.md)。

- Capability V2、独立 runner/controller、generator revision 3、durable retry/sleep/event、生命周期、
  retention 和有界 parallel do 已接入生产路径；V1 wrapper/facade/runner/codec 字节保持。
- Control schema 14、scheduler schema 8；本阶段追加的 control 013/014、scheduler 006/007/008
  已执行并冻结，后续只能追加 migration。
- 完整工作区 684 项测试通过，0 failed / ignored；Rust 行覆盖率 **90.16%**（56,391 / 62,547），
  门槛仍为 90.00%。格式、Clippy、无默认特性、MSRV、metadata 和依赖边界检查通过。
- P2.5 Hard/Product/snapshot、P2.4、P2.3 及递归 P2.2/P2.1/P1/P0/G0 最终验收通过。
  修正版 P2 Exit 完成三轮真实链路，并实际执行 14 点 Workflow SIGKILL 与 Queue commit crash 矩阵。
- 保留 DO 内 Workflow mutation fail-closed，以及 G0 精确 `D-abort`（三轮 `abortEvents 0 -> 0`）
  限制；未扩大 allowlist。本文其余章节继续定义已验收的支持面与不支持 API。

前置阅读：[总方案](../open-compute-workerd-platform.md)、[P2.4 设计](./p2-4-workflow-core.md)、
[P2.3 Gate 结果](./p2-3-gate-results.md)、[P2.4 Gate 结果](./p2-4-gate-results.md)、
[Gate 验证节奏](../references/testing.md)。

## 0. 结论与交付边界

P2.5 在已有 Workflow core 上增加持久等待与实例管理，不新增进程、Redis、消息中间件或数据库类型。
`platformd` 仍然拥有 SQLite authority 和调度器；唯一的 pinned stock `workerd` 子进程通过
`workerLoader` 执行冻结版本。睡眠、重试和事件等待期间，不保留一个等待到期的 JS invocation。

按下面的依赖顺序实施，每个工作包独立验收：

```text
P2.5.0 Runtime Hard Gate
  -> P2.5.1 Capability V2 / forward migration / replay identity
  -> P2.5.2 Durable yield / wake / recovery
  -> P2.5.3 Retry / backoff / attempt timeout
  -> P2.5.4 sleep / sleepUntil
  -> P2.5.5 waitForEvent / sendEvent / event timeout
  -> P2.5.6 pause / resume / terminate / restart
  -> P2.5.7 Retention / referrer cleanup
  -> P2.5.8 Bounded parallel step.do
  -> P2.5.9 Aggregate Gate / P2 end-to-end
```

先做公共的 yield/wake 协议，再接 retry 和 sleep，避免为每种等待各写一套 lease recovery。
Parallel 放到最后，只支持同一同步 fan-out 批次的 `step.do`，不在本阶段实现任意 Promise DAG。

### 0.1 已验证基线

| 证据 | 已验证内容 | 对 P2.5 的约束 |
|---|---|---|
| `95c389e`、P2.3 Gate | Queue consumer 与 Cron；Known/Unknown 分类、lease、真实 custom-event dispatch；Go | 复用 scheduler，不改变 Queue/Cron delivery 语义 |
| `9fe5b4a`、P2.4 Gate | Workflow create saga、冻结版本、顺序 step persistence/replay、crash、snapshot；Conditional Go | 继续使用真实生产 loader/SQLite 路径 |
| P2.4 token probe | Tenant 修改 Promise intrinsics 能观察 tenant realm 中的私有异步返回值 | raw run/step token 必须留在 system isolate 的 request-scoped `WorkflowRunController` |
| P2.4 DO probe | DO 内 Workflow `create` 不满足 output gate；`get/status` 可用 | 新的 `sendEvent` 和 modifier 默认也在 DO 中 fail closed |
| P2.4 transport 回归 | Body-bearing dispatch 使用独立 no-pool HTTP client 后稳定 | 保留 transport 修复，不以自动重试 mutation 掩盖 Unknown |
| 当前 pin | `runtime/workerd.lock.json`：`v1.20260826.1` | 本阶段不升级 workerd；G0 只接受原有精确 `D-abort` 观察 |
| P2.4 最终覆盖率 | Rust line coverage 90.12%，三轮最终 Gate 已完成 | 是前阶段证据，不是 P2.5 的覆盖率或验收结果 |

### 0.2 本阶段承诺

- V2 `step.do` 的静态 retry/backoff、per-attempt timeout、`NonRetryableError`；
- `step.sleep`、`step.sleepUntil`，重启后按已持久化 deadline 恢复；
- `step.waitForEvent`、`instance.sendEvent`、event-before-wait buffering 和 timeout；
- `pause/resume/terminate/restart`，其中 restart 只从头执行；
- V2 instance retention、可恢复的跨库清理，以及清理后的 external ID 重用；
- 最多 4 个同批并行 `step.do` 的默认本地配置；每个结果独立提交；
- 已完成 step 不重跑，未知结果允许重跑，旧 generation/token 不能提交。

不承诺 callback exactly-once、外部副作用回滚、强制取消任意用户 JS、完整 RpcSerializable、
rollback hooks、动态 delay function、restart-from-step、`createBatch`、并行 wait/`Promise.race`、
Workflow 原生 cron 配置或完整 Cloudflare REST/Wrangler 兼容。这些不偷偷变成本阶段的前置依赖。

### 0.3 必须守住的不变量

1. SQLite 是状态 authority；JS timer、内存 ready queue 和 replay cache 都可以丢弃。
2. `waiting/paused` 不持有 run token、heartbeat、执行 permit 或长时间存活的 dispatch RPC。
3. 等待注册、结果提交、事件消费、quota 记账和 due projection 在同一 scheduler transaction 中完成。
4. Yield 与事件到达可以交错，但不能丢 wakeup；唤醒后仍从 `run()` 开头 replay。
5. Replay identity 包括 generation、ordinal、kind、name/count、规范化配置和已冻结的依赖。
6. 只有一个有效 run lease；有并行 callback 也不能发放第二个有效 run token。
7. 业务 retry 与基础设施 Unknown recovery 分开；丢响应不直接消耗一次业务 retry。
8. V2 的最终业务错误可被 `run()` catch；内部 suspension、stale、corruption 不能伪装成业务成功。
9. 已提交的事件最多消费到一个 wait；`sendEvent` 本身不承诺对重复 HTTP 调用去重。
10. Paused instance 不执行用户代码；暂停不冻结 wall-clock deadline。
11. Restart 保留 internal instance ID、external ID、input 和 frozen target，只递增执行 generation。
12. V2 terminal instance 在 retention 内保留 frozen target 引用；GC 不能破坏 restart 所需代码。
13. 清理没有完成前不能复用 external ID；旧 handle 不得操作复用该 ID 的新 instance。
14. raw token、creation nonce、私有 backend、payload 和原始异常不得进入 operator 日志/metrics。

## 1. 参考实现与兼容边界

### 1.1 Cloudflare 外部 API

本次核对的官方文档：

- [Workers API](https://developers.cloudflare.com/workflows/build/workers-api/)：方法形状、retention
  object、instance status；当前还包括 rollback 与 restart-from-step，本文不将它们列为已支持。
- [Sleeping and retrying](https://developers.cloudflare.com/workflows/build/sleeping-and-retrying/)：
  静态 retry/backoff 和睡眠用法。CF 默认 timeout 为 10 分钟，本地采用更短的执行上限，见第 4 节。
- [Step context](https://developers.cloudflare.com/workflows/build/step-context/)：`attempt` 从 1 开始；
  callback 获得已解析配置。
- [Events and parameters](https://developers.cloudflare.com/workflows/build/events-and-parameters/)：
  实例创建后、到达 wait 前可以发送事件。
- [Rules of Workflows](https://developers.cloudflare.com/workflows/build/rules-of-workflows/)：
  side effect 放在 step 内，跨 activation 依靠 replay，而不是局部变量持久化。
- [Limits](https://developers.cloudflare.com/workflows/reference/limits/)：用来核对外部形状和常见上限；
  不复制 CF plan 容量。

本文中的 FIFO、deadline 裁决、暂停计时、restart target、quota 和 retention 默认值，都是
open-compute 明确选择的本地契约，不声称与 CF 的所有内部行为一致。

### 1.2 WDL / Miniflare

本地参考快照是 WDL `cf4e63e50d5f74ea0f75ed62dda21589cfc9be59`、workers-sdk
`296a1a7c97e027a308740e1eaaa6d904dec8f102`。实现时优先看以下文件：

| 参考 | 可借鉴的部分 | 不直接移植的部分 |
|---|---|---|
| `references/wdl/runtime/dispatch/workflow-step.js` | 顺序 ordinal、同步 fan-out、独占 sleep/wait、replay 与 suspension | Tenant realm 携带 token 的方案不能覆盖本项目的 system-isolate 安全结论 |
| `references/wdl/rust/workflows/src/api/` | lease、due、event、lifecycle、retention 的状态机拆分 | Valkey key/Lua、独立 workflows 服务和 mesh |
| `references/wdl/docs/modules/workflows.md` | ready hint 不作 authority；并行 join 和冻结版本引用 | WDL restart 选 active version；本项目选择保留原 frozen target |
| `references/workers-sdk/packages/workflows-shared/src/context.ts` | step config、event 返回值、错误与 replay 测试 | 将 Miniflare DO engine 变成新的产品 runtime |
| `references/workers-sdk/packages/workflows-shared/src/lib/retries.ts` | constant/linear/exponential 公式 | 动态 delay function 与 stream/rollback 分支 |
| `references/workers-sdk/packages/workflows-shared/src/engine.ts` | modifier、事件恢复、测试案例 | 依赖 DO abort 的取消能力；暂停时移动 timer deadline 的本地实现 |
| `references/workers-sdk/packages/miniflare/src/plugins/workflows/` | binding/engine 的装配边界 | Miniflare 的开发服务器生命周期 |

同一份参考代码也可能与当前 CF 文档存在差异。API 以官方文档为参考，执行语义以本文的本地契约
和 pinned-workerd Gate 为准，不能用“Miniflare 能跑”替代产品验证。

## 2. Capability V2 与旧数据

### 2.1 显式选择，不就地升级旧实例

新增 Workflow execution capability 2 和 caller binding capability 2：

```json
{
  "deploymentId": "<ready deployment UUID>",
  "className": "OrderWorkflow",
  "capabilityVersion": 2
}
```

这是现有 version create API 的新增可选字段；省略仍为 1。新的 caller declaration 使用
`{ "type": "workflow", "id": "<definition UUID>", "capabilityVersion": 2 }`，省略仍为 1。
沿用部署声明现有的 `type` 字段；持久 descriptor/runtime-source 使用其既有 `kind` 字段。
这两个字段都必须经过 schema、descriptor digest、runtime-source 和 loader validation，而不是仅由
facade 分支决定。V2 version 通过实际 V2 runner 的 class probe 后才能 ready。

- capability 1 与 capability 2 保持各自的执行契约；系统源码统一使用当前 TS 生成产物，不保留历史 descriptor/hash。
- V1 仍是顺序 `do`、attempt=1、无 retry、无 retention；已 catch 的 step error 仍按原 failure latch 失败。
- V2 支持本文能力；最终 step error 可被 catch，失败 step 也可成为合法 replay 历史。
- `create` 要求 caller capability 与 definition 当前 version capability 一致；不一致返回
  `WORKFLOW_CAPABILITY_MISMATCH`。不能让旧 caller 静默创建改变错误/retention 行为的新实例。
- V1 caller 的 get/status 只访问 V1 instance；不能经旧 name-only handle 访问有 ID 重用能力的 V2 instance。
- V2 caller 可只读查询 V1 instance，但新增 mutation 对 V1 返回 `WORKFLOW_METHOD_UNSUPPORTED`。
- V1 history 不自动采纳 V2 retention，也不尝试重新取得已经释放的旧 artifact 引用。

升级现有 definition 的 current version 时，需要一起发布 V2 caller；无中断迁移可先建新 definition。
这是一次显式能力切换，不修改历史 deployment。原 bundle 若兼容 V2，可用于新的 V2 version，无需
仅为递增 capability 重传相同 artifact。

### 2.2 Wrapper 与 loader 身份

系统源码按 day1 使用 `runtime/src/loader/wrappers/` 中的一套 TS wrapper。generator 只生成
模块导入、已验证配置与导出；执行逻辑位于可严格检查的 runtime、DO、Workflow 模块。
capability 1/2 分别选择对应 runner，不再选择历史 generator revision 或旧 JS 资产。

每个 WorkerCode 都包含完整生成源码清单的摘要，包含无产品绑定的 Worker。
内部模块使用保留的 `__open_compute__/` 命名空间，不允许租户 bundle 覆盖。
Workflow loader cache key 区分 execution capability 与 frozen version descriptor；binding 与
execution 分开校验，host 不接受 tenant 传入的 capability/version 作为 authority。

冻结 Workflow version 不改变其他产品的调用契约。DO 仍执行 P0.7 的 active Worker deployment
校验；已退役 Worker deployment 的 DO 调用返回 `DO_DEPLOYMENT_STALE`，不能通过 Workflow replay
回退已升级的 DO facet。P2 Exit 切换的是 Workflow current version，保留 DO 所属 Worker 的 active
deployment，并单独验证原 Workflow target 不变。

`NonRetryableError` 优先验证 pinned runtime 对 `cloudflare:workflows` 的原生导出。若缺少导出，
使用受控模块 shim 和真正的 module-specifier resolver；不能对 tenant source 做全局字符串替换。
Shim 仅影响 V2 code path，保留普通字符串、注释、模板和 `import.meta` 的含义。

## 3. Public API 子集

```ts
type LocalStepConfig = {
  retries?: {
    limit: number;
    delay: number | string;
    backoff?: "constant" | "linear" | "exponential";
  };
  timeout?: number | string;
};

type LocalWorkflowStepEvent<T> = {
  type: string;
  payload: T;
  timestamp: Date;
};

// Interface sketch; not a replacement for generated runtime types.
interface LocalWorkflowStep {
  do<T>(name: string, callback: (ctx: StepContext) => T | Promise<T>): Promise<T>;
  do<T>(name: string, config: LocalStepConfig,
        callback: (ctx: StepContext) => T | Promise<T>): Promise<T>;
  sleep(name: string, duration: number | string): Promise<void>;
  sleepUntil(name: string, timestamp: Date | number): Promise<void>;
  waitForEvent<T>(name: string, options: {
    type: string;
    timeout?: number | string;
  }): Promise<LocalWorkflowStepEvent<T>>;
}

interface StepContext {
  step: { name: string; count: number };
  attempt: number;
  config: {
    retries: {
      limit: number;
      delay: number;
      backoff: "constant" | "linear" | "exponential";
    };
    timeout: number;
  };
}
```

`ctx.config` 返回完整 resolved retries，包括 `backoff`；时间以规范化数值毫秒为准。此处类型是说明，
生产类型应与 core 的 resolved config 保持一致。

Instance 新增 `sendEvent({type,payload})`、`pause()`、`resume()`、`terminate()`、`restart()`；
create 新增 `retention: { successRetention?, errorRetention? }`。`terminate/restart` 暂不接受非空 options。
`status()` 的 V2 集合是 `queued/running/waiting/waitingForPause/paused/complete/errored/terminated`，
并返回 `rollback: null`；只有 complete 带 output，errored 带 sanitized error。

Payload、step/final output、event payload 延续 canonical JSON 子集；传输层恢复 `timestamp: Date`
不表示支持将任意 Date/Map/stream 持久化为 step result。`sleep*` 的 replay 结果返回 `undefined`。
事件 type 使用 ASCII `^[A-Za-z0-9_][A-Za-z0-9_-]*$`、1–100 bytes；不接受含点的 type。

Event payload 上限不扣除平台 envelope：`wait_event` 的内部 output 允许额外最多 1 KiB 的 type/timestamp
envelope，仍按完整 bytes 计入 state quota；其他 do/final output 的 1 MiB 上限不放宽。

未知字段、动态 retry function、rollback overload、restart-from、并行 wait 必须稳定拒绝，不能忽略后
宣称执行成功。非空 `{ rollback: false }` 也不作为本阶段已支持的 overload。

## 4. 本地配置、计时与预算

### 4.1 默认策略

| 项 | P2.5 本地默认/上限 | 说明 |
|---|---|---|
| retry | limit=5、delay=10,000 ms、backoff=exponential；limit 0–100 | limit 是额外重试数，最多 `1 + limit` 次业务 attempt |
| attempt timeout | 默认 60,000 ms；范围 1–240,000 ms | 有意不同于 CF 默认 10 分钟；不得超过 dispatch deadline 减去 drain margin |
| 单 activation | 沿用 dispatch timeout=300,000 ms；预留 30,000 ms drain margin | 长 Workflow 在安全 step 边界主动 yield，不把整条 Workflow 限制为 5 分钟 |
| sleep/event timeout | wait 默认 24 小时；每个时长最大 365 天 | 只写 deadline，不占用 activation |
| retry delay | 最终 delay 最大 24 小时 | checked arithmetic 后 clamp；backoff 不无限溢出 |
| retention | success=7 天，error/terminated=30 天；每项 1 小时–365 天 | 创建时冻结，后续 operator 改默认值不追溯改旧实例 |
| parallel `do` | 默认最大 4，配置范围 1–16 | P2.5.8 前强制为 1；不新增独立 scheduler 进程 |
| workflow execution pool | 沿用 16 | 默认并行 callback 上界为 `16 × 4`；不是 64 个独立 run lease |
| durable descriptor | 沿用 `max_steps=1024` | 本地将 sleep/wait 也计入，区别于 CF 对 sleep 的计数 |
| canonical JSON | 每个 input/result/event payload 1 MiB、depth 127 | 沿用已有 JS/Rust parity fixtures |
| retained state | 每实例默认 32 MiB、每 account 默认 1 GiB | 包含所有 step、buffered event、结果和已定义的逻辑 metadata |
| event inbox | 每实例最多 128 条、合计最多 8 MiB 未消费事件 | 还必须同时满足 instance/account state quota |
| registry/GC page | 每轮默认最多 100 条，受 maintenance 时间预算约束 | 不能以一次无界扫描阻塞 Queue/Cron |

重试默认值是 V2 capability 的固定默认，不是每次 replay 读取 operator 当前值。Capacity 是 admission
上限：降低配置不能截断已存 output、改变已注册 deadline，或把健康的旧行判断为 corrupt。
冻结的 retry/timeout/retention 配置仍有效；新增写入不能超过新的容量策略。

配置新增字段沿用现有 `WorkflowsConfig` 的严格解析。新增默认字段不能改变旧 snapshot 的默认 policy
fingerprint；必须给默认配置、非默认配置与旧 snapshot fixtures 分别测试。不要仅靠 serde 新默认值
静默重算旧 manifest policy hash。

当前配置入口为 `[workflows.default_retention]`，使用规范化毫秒字段 `successRetentionMs` 和
`errorRetentionMs`。`create` 只对缺省项采用当时配置；显式 API override 仍使用 duration grammar。
这两个默认值以及 `max_parallel_steps/max_buffered_events/max_event_bytes` 的默认序列化值会省略，
非默认值纳入 policy fingerprint。示例见 `share/default-config.toml`。

### 4.2 Duration grammar

先支持 finite 非负数值毫秒，以及 `<number> <unit>`：millisecond/second/minute/hour/day/week，允许
复数和已列明的 `ms/s/m/h/d/w`。大小写不敏感、首尾空白可 trim，小数向上取整到毫秒。
不支持复合表达式、月份、年份、日期字符串或隐式 parseFloat；`sleepUntil` 只接受 valid Date/数值毫秒。
`sleep(0)` 和过去的 sleepUntil 同事务完成；`waitForEvent(timeout=0)` 先检查已提交事件，否则立即超时。
Retry delay=0 仍经 durable yield 和 scheduler 公平调度，不能在同一个 JS tick 内无限自旋。

JS/Rust 共用 parity fixtures：单位、空白、小数、NaN/Infinity、负数、溢出、safe integer、边界值。
使用 checked integer arithmetic；deadline 不接受超过 JS safe integer 或存储 i64 范围的数值。

### 4.3 三种时间不能混为一谈

- Durable deadline 使用 host 的 observed wall-clock 毫秒，与 P2.1 clock policy 一致，供重启后判断 due。
- 当前进程的 callback/watchdog 使用 monotonic clock；时钟回拨不延长已运行 callback 的本地预算。
- run lease 只代表当前 activation 的提交权；不等于 step timeout，也不等于 sleep/event deadline。

一次长 sleep 不持续续租。Wall-clock 前跳后，due work 以有界批次追赶；回拨按现有时钟策略处理，
不重新计算相对 sleep 的起点。Snapshot restore 保留绝对 deadline，不能再加一遍 duration。

## 5. Runtime Hard Gate

先新增 `p2_5_workflow_hard_gate` 测试目标，走生产 system host、动态 loader、真实 SQLite 和当前 pin。
本节的通过条件是计划，不是新的已接受 limitation。

| Probe | 必须证明 |
|---|---|
| Durable suspension | wait commit 后，普通 `await step.sleep` 路径迅速结束 dispatch；执行 permit/heartbeat/RPC 不随睡眠时长占用 |
| Catch/forged signal | 用户捕获或伪造名为 suspension 的 Error，不能让未满足的 wait 被当作完成，不能继续取得新 grant 或写 terminal success |
| Prototype adversary | 改写 Promise constructor/then、thenable、Error getter、JSON hooks 时，不泄漏 run/step token；覆盖 retry、wait、parallel 分支 |
| Bounded timeout | Callback 不 resolve、延迟 resolve/reject、transport 断开时，可信 host 能作逻辑 timeout/fence；迟到结果不可提交 |
| Parallel | 同步登记多个 descriptor，独立 token/commit，乱序完成不把 sibling grant 串用 |
| NonRetryableError | 验证原生导出或受控 shim；catch/replay 获得相同稳定错误类别；不会混淆内部控制信号 |
| Frozen loading | 同 bundle 的 V1/V2 run、旧 ready deployment 和没有 Workflow binding 的 Worker 同时正确 |
| DO mutation | create/sendEvent/modifier 在 tenant facade、trusted transport、backend 三处 fail closed；只读继续可用 |

不能把 `Promise.race` 胜出、RPC dispose 或 client disconnect 当成“用户 callback 已停止”。本阶段保证
逻辑失效和拒绝迟到提交，不保证撤销已经发出的 HTTP/D1/R2/DO 操作。普通 suspension 必须释放实际
执行资源；仅观察 scheduler 的 waiting 行、忽略未结束的 workerd invocation，不算通过。

若普通等待无法释放调用资源，或可信 timeout 不能隔离迟到提交，P2.5 停在 No-Go；不能退化为
长 `setTimeout`。不协作的用户代码沿用 runtime limit/transport watchdog 与 Unknown recovery，不能
无限累积不计数的后台调用，也不能为了终止单个 Workflow 随意杀掉所有租户共用的 workerd。
必须在此 Gate 固定资源上界和异常 drain 行为，再写后面的产品实现。

## 6. 存储设计与 migration

### 6.1 仍然只有两个平台 authority 文件

`control.sqlite` 放 definition/version/binding、instance reachability 和跨库操作 intent；
`scheduler.sqlite` 放 instance、step、deadline、event inbox 和 GC receipt。不为每个 Workflow、step
或 wait 创建 SQLite 文件，不把 Workflow state 写入 D1。业务通过 bindings 使用的 KV/D1/DO 数据库保持原布局。

下一组 migration 预定为 control `013_workflow_durable_waiting.sql`、scheduler
`006_workflow_durable_waiting.sql`。编号以实施时的 registry 为准；migration 一旦执行，不论是否形成
release，后续 schema 变化都继续追加下一 migration，绝不回改已执行的 013/006。

当前 scheduler 005 的 CHECK/trigger 限制了 state、`kind='do'`、`config_json='null'`、attempt=1、
generation immutable、step frontier 和禁止删除。Control 012 也限制 capability=1 和 referrer 状态。
因此不能只 ADD 几列：需要一次可回滚的 Workflow 子图重建，再安装 V1/V2 分支约束。

### 6.2 Control delta

| 表 | 变更 |
|---|---|
| `workflow_versions` | capability CHECK 改为 1/2；descriptor 对 V2 使用明确的编码分支；旧 bytes 不重算 |
| `workflow_bindings` | capability 1/2；create 授权验证 caller/target capability；name conflict、lifecycle 与 digest guard 保留 |
| `workflow_instance_referrers` | 增加 `retained/restarting`；允许精确 restart intent 下 generation+1；将 active quota 与 artifact reference 生命期分开 |
| `workflow_instance_operations`（新） | 每 instance 最多一个 restart/purge intent；保存 operation UUID、instance UUID、creation identity、expected/target generation、kind、创建时间；不存 payload |
| typed referrers/guards | `live/retained/restarting` 都保留 deployment/definition 引用；只有有证据的 purge/release 才可删除 |

`max_active_per_account` 是非 terminal instance 数，不是 live reference 数。加入 `retained` 后，不能继续
使用当前 `state != 'released'` 计算 active quota；creating/live/restarting 计 active，retained 不计，
但 retained 仍占 total-instance/state-byte/artifact 容量。Restart 在 intent prepare 时重新预留 active quota。
Retention policy/expiry 的 authority 只在 scheduler；control prepare 可以读取其快照，但 apply 必须再次
核对，不在两库维护两套可分别修改的 TTL。

### 6.3 Scheduler instance delta

保留 `workflow_instances` 作为唯一 instance authority；generation 是当前执行代际，不新增通用任务库。

| 字段组 | 设计 |
|---|---|
| immutable identity | 现有 account/definition/external ID/internal UUID、version/deployment/class、digest、capability、creation nonce、input；restart 均不修改 |
| generation | 初始 1；仅精确 restart saga 可 +1，不回绕、不复用 |
| state | V1 保持四态；V2 增加 `waiting/paused/terminated` |
| leased state | `running` 才有 run token/claim/lease；`pause_requested`、`yield_requested` 仅控制 draining，不授予第二条 lease |
| ready/due | `next_run_at_ms` 用于 queued；`next_wake_at_ms` 是当前未决 step 的最早 durable deadline projection |
| frontier/counters | 保留 V1 completed count；V2 使用 `registered_step_count`、`settled_step_count`、`completed_step_count`，不能把三者混用 |
| retention | 创建时冻结 success/error retention ms；terminal 时写 `expires_at_ms`，非 terminal 必须为 NULL |
| operation fence | `last_restart_operation_id` 用于证明 scheduler 已执行某个 restart |
| accounting | `state_bytes` 继续作为 exact logical retained-byte 计数；增加 inbox count/bytes |

V2 `waiting/paused` 的 token/lease/claim/next-run 全为 NULL；queued 有 next-run，无 lease；terminal 有
terminal time/expiry，无 runnable/due projection。`pause_requested=1 AND state='running'` 对外映射
`waitingForPause`。Yield draining 期间仍是 running，不能出现 waiting 行持有有效 run token。

### 6.4 Step delta

继续使用 `workflow_steps`，PK 为 `(instance_id, instance_generation, ordinal)`：

| 字段 | 约束/用途 |
|---|---|
| kind | V2 为 `do/sleep/sleep_until/wait_event`；V1 仍只能 do |
| name/count | name 1–256 UTF-8 bytes；count 按 `(kind,name)` 从 1 计；ordinal 在整条 run 的 API 调用顺序中从 0 计 |
| config_json/hash | Rust 规范化后的 resolved config；长度最多 4 KiB；不能只比较 hash 而跳过 schema 校验 |
| dependencies | 有序、去重且小于 ordinal 的 predecessor 集合；V2 并行前只能是上一 settled step；V1 不新增 edges；最大 fan-in 跟随 parallel 上限 |
| batch | `batch_first_ordinal/batch_size`；同批共享 predecessor；并行前 size=1；replay 比较完整批次形状 |
| state | `pending/running/retry_wait/waiting/complete/failed/cancelled`；kind/state 组合必须检查 |
| attempt | do 的已开始业务 attempt 数；初始 claim 为 1；非 do 为 0；Unknown recovery 不直接递增 |
| grant | 仅 running do 持有当前 run/step token；retry_wait/waiting/settled 不持有 step token |
| attempt timing | started/deadline；Unknown recovery 保留未结束 attempt 的原 deadline，不能无限重置 timeout |
| due_at_ms | sleep、retry、event-timeout 的绝对 deadline；event 的无匹配等待也一定有 deadline |
| result/error | complete 的 JSON output 或 wait event envelope；failed 的稳定 error；do 可保存最近一次 attempt error，不保存无界异常历史 |
| settled_at_ms | complete/failed 都计 settled；sleep void 用显式 kind/状态表示，不与 JSON null 混淆 |
| cancelled_at_ms | terminate 或不可恢复的 run failure 关闭未完成 step；不计 settled，不可 replay 为成功 |

`workflow_step_dependencies` 新表以 child/parent ordinal 构成复合 FK，限制同 instance/generation，
`parent < child`；并行前也采用同一表达，避免以后把 completed count 偷偷解释成 DAG。
这只是 replay descriptor 的依赖，不新增独立 DAG planner 或对外 DAG API。

Step `complete/failed/cancelled` 在本 generation 内 immutable。只有持久 restart/purge intent 才允许删除旧历史。
V2 run 可以在捕获 failed step 后 complete；所有已登记 step 都必须 settled，不能只要求所有 step complete。

### 6.5 Event inbox 与 GC receipt

`workflow_events`：

- PK `(instance_id, instance_generation, event_seq)`；同库 FK 到 instance；
- `type`、canonical `payload_json`、host `accepted_at_ms`、logical byte size；
- event_seq 来自 instance 内单调计数器，不依赖客户端 ID 或 wall-clock 唯一性；
- index `(instance_id, instance_generation, type, event_seq)`；
- 只保留未消费事件；消费时将完整稳定 envelope 写入 step result，再在同事务删除 inbox row。

`workflow_gc_receipts`：保存 purge operation ID、internal instance UUID、creation identity、generation
和删除完成时间。它证明 scheduler 的缺行是已授权清理，不是 corruption。Control finalize 后再删除 receipt；
不能仅按 TTL 丢弃尚未对账的 receipt。

### 6.6 Index、accounting 与约束

至少需要 queued-due、running-lease-expiry、waiting-next-wake、terminal-expiry、event-type-FIFO、
operation-reconcile 索引。Due/retention 查询使用 keyset pagination，不逐 tick 扫全部历史。
`next_wake_at_ms` 是可核对 projection；与 step 表不一致按 invariant error 处理，修复只允许显式 reconciler
从完整、有效的 step authority 重建，不能在普通读取中猜测。

逻辑 state bytes 包含：input/output、descriptor/name/config/dependencies、step output/error、event
type/payload/envelope 和固定计价 metadata；新增列的固定计价写入一份 JS/Rust/SQL 共用规范。
消费事件时算净增量，不能同一 payload 长期重复计在 inbox 和 step；每次 mutation 同事务校验
instance/account cap。SQLite 物理页/WAL 大小是另一类 operator 容量，不能用逻辑 bytes 冒充磁盘用量。
V1 保留原有计价规则和 state_bytes；V2 使用新的 metadata 计价。不能仅因为添加 schema 列，就在 migration
中增加旧实例的已用容量，导致原本合法的历史越过 quota。

当前 V2 基础实现的固定计价契约见 [`share/workflow-accounting-v2.json`](../../share/workflow-accounting-v2.json)：
instance 为 256 bytes、step 为 160 bytes、每条 dependency 为 16 bytes、inbox event 为 32 bytes。
这些是固定逻辑计价单位，不代表 SQLite 行或物理页大小。Instance 另计 definition name、external ID、
class name 与 input/output/error；step 另计 name/config/result/error；event 另计 type/payload。
字符串按 UTF-8 bytes 计，消费后的完整 envelope 只保留在 step output；临时 run/step token 不计入。

### 6.7 FK 始终开启的重建步骤

在停止 admission、取得现有 migration owner 后，每个数据库各用一个 migration transaction：

1. 校验旧 checksum、完整性和 V1 history；以临时 staging 表保存需重建的行和列，包含所有 token/digest。
2. 只移除本次涉及的 Workflow triggers；外部表上的 Workflow referrer/name-conflict triggers 也需精确列入。
3. Control 先保存 definition 的 current-version pointer，再临时置空；清理需要重建的 child tables 后
   重建 versions、bindings、instance refs。Definitions 与其他产品 authority 不重建，typed referrer 行不重插。
4. Scheduler 先保存 steps，再移除 child/dependency 表，重建 instances/steps。按父后子顺序恢复数据；
   V1 的 config `null`、attempt、run token、deadline、bytes 与 terminal output 原样保留。
5. 恢复 control current pointer，安装新版全部 CHECK/index/trigger；在安装 INSERT guard 前搬运已验证历史，
   避免把旧 terminal 行误判成新建请求，也避免触发重复 add-ref。
6. 执行 `foreign_key_check`、行数/digest/counter 对账和 registry/checksum 更新，一并提交。

全过程 `foreign_keys=ON`，不用 writable_schema、关闭 FK、无条件 `INSERT OR IGNORE` 或 runtime 自愈。
精确 DROP/CREATE 顺序必须由含 current version、creating/live/released、running step 的 migration fixtures
验证；失败回滚到完整旧 schema。两库没有原子升级：任一库未到本 release 要求的版本，启动不得 ready；
重启继续完成另一库的 forward migration，不自动降级第一库。

## 7. Durable yield、wake 与 replay

### 7.1 两阶段 yield

不能让 sleep 注册一完成就释放 run token：稍后需要支持同批 sibling commit，也要处理事件在 yield 前到达。
统一协议如下：

1. 在有效 run fence 下注册/更新 step；写 due 或 event wait，设置 `yield_requested=1`。Instance 暂仍 running。
2. System controller 记录 token-free 的 suspension verdict。Tenant runner 停止开始新的 step，drain 已获
   grant 的 sibling，向 host 返回 control outcome，而不是业务错误结果。
3. Host 发 `yield`；scheduler 同事务核对 run fence、没有未处理的 running step、已登记历史与 pause flag，
   决定转 queued、waiting 或 paused，并清空 token/lease。
4. Dispatch 正常结束，释放 permit/heartbeat/RPC。Scheduler 后续依据 SQLite 重新 claim。

运行中 event 已满足 wait，或最早 deadline 在 drain 时已到期，则第 3 步转 queued，不会先 sleep 再丢通知。
Registration 不能自行启动新 run；单个 host request 的 in-memory notification 仅用于减少延迟。

### 7.2 可执行状态转换

| 原状态 | 事件 | 结果 |
|---|---|---|
| queued | 取得执行 permit 后，SQLite claim 成功 | running + 新 run token |
| running | 登记 sleep/event/retry | running + yield_requested，禁止新批次 |
| running + yield | 现有 callback 已处理，无可立即恢复工作 | waiting，无 token |
| running + yield | 等待已满足或只是 activation budget 到期 | queued，无 token |
| running | 收到 pause | waitingForPause；阻止新 grant，允许有效已发 grant settle |
| running + pause | drain 完成 | paused，无 token |
| waiting | deadline/event 完成 | queued，或仍等待另一未决 retry |
| paused | deadline/event 到达 | 可更新持久 step/inbox，不执行用户代码，仍 paused |
| running | `run()` 正常返回且所有已登记 step settled | complete |
| running | 未捕获业务异常/确定性失败 | errored |
| 任意非 terminal | terminate | terminated，失效所有 grant |

`waiting` 的 retry 唤醒可以只有一个 sibling 到期：下一 activation replay 所有已登记 sibling，完成的
只读 output，到期的可执行，未到期的保持等待；不能要求整个 batch 同时到期才运行。

### 7.3 Crash 与 Unknown

- Register commit 前失败：旧行未变，等待 run lease recovery。
- Register commit 后、yield 前 crash：保留 lease；到期后恢复未完成 do 为可重领，保留已注册 wait/deadline。
- Yield 已 commit、响应丢失：scheduler 的 waiting/queued/paused 已是事实，不能覆盖回 running/errored。
- Callback success/failure commit 响应丢失：Unknown；停止该 activation 后续业务，不重复发送相同 mutation
  来猜结果。下一次 replay 从 persisted step 裁决。
- Recovery 遇到尚未超时的未知 do，使用同一业务 attempt、新 step token；已到 durable attempt deadline
  才能记录 timeout 并按 retry policy 处理。Unknown 本身不等于 timeout。

Driver 对 yield 和 terminal 使用不同 result enum。当前 service 在已知 run 结束后调用 `release_instance`
的路径必须拆分：waiting、paused、retry、budget yield 都不能释放 frozen deployment referrer。

### 7.4 Replay identity 与提前返回

API 调用时同步分配 ordinal/count，先规范化 config，再请求 host。每次 replay 从 0 开始，既比较已完成
记录，也比较 failed/waiting/retry 的 descriptor。不能只对成功 cache hit 校验。

相对 duration 存 descriptor，首次注册的绝对 due 单独存 authority；replay 不重算起点。
`sleepUntil` 的绝对 timestamp 本身属于 descriptor；在 step 外用 `Date.now()` 重算不同 timestamp 会得到
`WORKFLOW_NON_DETERMINISTIC`。先用 `step.do` 持久化目标时间，或使用相对 sleep。

V2 用 registered/settled count 判定历史遍历完整性。提前 return、遗漏旧 descriptor、return 时仍有未完成
step、新 descriptor 依赖不匹配都不能 terminal success。平台检查可观察的未完成工作，不声称能静态
证明用户对每一个已经 settled 的 Promise 都写了 await。完成 step 的返回值每次重新解码，tenant 修改
对象不能污染 cache 或后续 replay。

### 7.5 控制信号不是普通 Error

System controller 自己持有 `yield/unknown/closed` 状态和 raw grant；tenant 可见的只是 verdict。
Runner 使用私有的 process-local suspension 标记向外层返回控制结果，不能凭 Error.name 识别成功 yield。
Host 最终还需核对 SQLite 状态，不能只信任 tenant 的 `{outcome:"suspended"}`。

用户 catch 了 suspension，也不能再取得新 step grant 或完成 instance；再入 controller 稳定失败，后续
late result 被 fence 拒绝。不能承诺 catch/finally 里的任意外部 side effect 被取消，应用必须把这些操作
放入正常 `step.do` 并做业务幂等。不能把 suspension 持久化为可由用户构造的普通失败记录。

## 8. Retry、backoff 与 attempt timeout

### 8.1 Retry transaction

第 n 次业务 attempt 的 callback throw，或可信 host 确认 attempt deadline 到期：

1. 检查 instance UUID/generation、run token/lease、ordinal、step token、attempt。
2. 将错误映射为稳定类别；不保存原始 message/stack、cause 或任意 getter 的输出。
3. 若是 NonRetryableError，或 `n > retry.limit`，写 failed 并清 step token，向 run 传播可 catch 的错误。
4. 否则写 retry_wait、最后错误和 `due_at_ms = decision_time + delay(n)`，清 step token，申请 durable yield。
5. 同事务更新 accounting 与 due projection；callback 在收到已 commit verdict 前不能继续下一业务 step。

```text
constant:    delay(n) = base
linear:      delay(n) = base * n
exponential: delay(n) = base * 2^(n - 1)
persisted:   min(checked delay(n), 24 hours)
```

`limit=0` 不重试；limit=5 对应最多 6 次业务 callback attempt。只在下一次实际 claim 到期 retry 时
递增 attempt。Overflow 使用饱和到已声明 cap 的明确算法；不是浮点溢出后再存 Infinity。
不加随机 jitter，避免 replay 配置和测试隐式变化；多实例 due 风暴由 admission/fairness 控制。

### 8.2 Timeout 与长流程

System host 为每个有效 step grant 建立可信 timeout tracker；tenant 的 timer、修改过的 Promise 或
callback 提供的时间都不是 authority。Success commit 同时检查 durable deadline；到期边界之后的
success 与 timeout transaction 只有一方能赢。已超时 step 的迟到 callback 不能覆盖 retry/final error。

Per-attempt timeout 不涵盖之前的 sleep/event/backoff，也不等于整次 Workflow timeout。准备开始一个
fresh do 时，若 activation 剩余预算不足以容纳该 attempt timeout 加 drain margin，则先 budget-yield
到 queued，下一 activation replay 后再 claim；不先消耗 attempt 再主动抛弃它。

无法确认 callback outcome 的 transport 超时仍是 Unknown。已有 durable attempt deadline 可在恢复时
判定 timeout；不能通过“每次 Unknown 都延长 deadline”无限运行，也不能把平台 outage 直接记为业务 throw。
逻辑 timeout 后外部请求可能完成；下一 attempt 也可能重复副作用，应用幂等不因 retries 而变成可选项。

### 8.3 错误可捕获与不可捕获的边界

V2 对 retry exhausted、NonRetryableError、event timeout，先持久化 settled failure，再 reject 给 `run()`。
用户可 catch 并执行 fallback step；replay 必须 reject 同一种 sanitized error，进入相同分支。
没有 catch 时，run 进入 errored。

Serialization/quota/descriptor mismatch、stale authority、Unknown 和 suspension 属于平台协议失败或
控制状态，不能沿用“业务 catch 后任意完成”的分支。Unknown 保留恢复机会；确定的 nondeterminism
和非法 step 记录为 terminal platform error。V1 failure latch 保持原样，不在 migration 时重解释旧 failed row。

## 9. sleep 与 sleepUntil

`sleep` 第一次登记时，Rust 计算并冻结 `due_at = host_now + normalized_duration`；`sleepUntil` 使用
已校验的绝对 timestamp。到期时间已过去则同事务写 complete 并返回 void，否则登记 waiting 并走 yield。

Due maintenance 在短 transaction 中把到期 sleep 标为 complete，更新 settled count，再将非 paused
instance 转 queued。唤醒只表示可争取 permit，不保证准点执行；event/callback latency 不在 sleep API 的承诺内。
到期任务按 `(due_at, instance_id, ordinal)` 有界处理，必须与 Queue/Cron 新工作交错，避免整批 sleep
同时到期后阻塞其他产品。

同名、不同 duration 的 replay 必须失败。重复消费同一 due hint 无效果，不能多加 settled count。
sleep/wait 不会执行 callback，也不消耗 retry attempt；同样计入本地 descriptor/state quota。

## 10. Event inbox、wait 与 timeout

### 10.1 sendEvent

V2 handle 的 system-isolate transport 在 binding scope 中解析精确 internal UUID，获得当前 generation，
然后在 scheduler transaction 中重新核对 instance identity/state。Tenant 只能提供 type/payload。

1. 拒绝不存在、已过 retention、terminal 或处于跨库操作中的 instance。
2. 校验 JSON/type，检查 inbox count/bytes 和 instance/account quotas。
3. 分配 event_seq、记录 host accepted_at，插入 inbox。
4. 若存在同 type wait，使用下节同一个 match/timeout helper 裁决；可以立即消费，不要求先进入 waiting。
5. 原子更新 step/inbox/counters/due/ready；durable commit 后 `sendEvent` 才 resolve。

允许 queued/running/waiting/paused 接收；pause 不丢外部事件，但不启动用户代码。Inbox 满返回
`WORKFLOW_EVENT_QUEUE_FULL`，不驱逐旧事件。没有 event TTL，未匹配事件保留到消费、restart 或实例 retention。
不新增公开 event ID/idempotency key；调用方重试未知 sendEvent 可能插入两条，业务 payload 应包含业务去重 ID。

### 10.2 waitForEvent

按 descriptor 创建 wait；先找同 generation/type 的最小 event_seq，消费成功则写完整
`{type,payload,timestampMs}` 到 step output，在 facade 中恢复 Date。Replay 固定该 envelope，不能重新取
“最新事件”。没有匹配时写首次 deadline，默认 24 小时，并申请 yield。

同一事件只能完成一个 wait。同 type 的顺序两个 wait 消费两条 FIFO 事件；P2.5 不支持多个并行 wait，
因此不实现广播或多个 wait 竞争的额外规则。不同 type 各自 FIFO，不承诺跨 type 的应用处理顺序。

### 10.3 事件与超时的唯一裁决

采用一条本地规则：已在 authority 中记录、且 `accepted_at_ms < deadline_ms` 的匹配事件可满足 wait；
相等时 timeout 赢。首次注册前已提交的匹配事件先被消费，包含 timeout=0 的情况。

`sendEvent`、register、due tick、resume/recovery 都调用同一 transaction helper：

```text
BEGIN IMMEDIATE
  verify exact instance / generation / step identity
  if step already settled: return recorded verdict
  pick oldest matching committed event eligible for this wait
  if found: copy envelope to step; delete inbox row; settle once
  else if deadline <= authority_now: persist event-timeout failure once
  else: retain waiting state
  update counters and due projection
  if instance waiting and now has runnable continuation: mark queued
  if instance paused: keep paused
COMMIT
```

晚于 deadline 的新事件不能因为 timer tick 延迟而“抢救”已超时 wait；先裁决 timeout，再将新事件
保存在 inbox，供后续同 type wait 使用。Timeout 与 event 不能一边消费 event、一边写 failed step。
Timer tick 晚到时，必须先看 deadline 前已持久化的匹配事件，不能直接把健康事件丢成 timeout。

HTTP 响应丢失不影响已提交事件；event commit 与 result commit 是同一事务，不存在“已删除 inbox、
step 没存 payload”的窗口。所有竞争都用真实 SQLite 的事务测试，不用进程内锁替代持久化裁决。

## 11. pause、resume 与 terminate

### 11.1 生命周期契约

| 操作 | 接受状态 | 持久效果/返回语义 |
|---|---|---|
| pause | queued、waiting、running | 前两者直接 paused；running 写 pause_requested，返回表示请求已提交，不表示 callback 已结束 |
| pause 重复 | paused、waitingForPause | 无效果成功，不增加 generation |
| resume | paused | 检查已持久化 wait/retry 是否满足；转 queued 或 waiting |
| resume 其他状态 | 包括 waitingForPause | `WORKFLOW_INSTANCE_STATE_CONFLICT`，不隐式取消尚未 drain 的 pause |
| terminate | queued、running、waiting、waitingForPause、paused | 立即逻辑 terminal，清除所有 run/step grant 与 due/ready projection |
| terminate 已 terminal | complete、errored、terminated | state conflict；不能改写已提交 output/error |

Pause 不取消已发出去的 side effect。已持有效 grant 的 callback 可在原 deadline/lease 内提交；不发新
grant，不开始下一 batch。全部结束后转 paused。当前 attempt 超时则先按 retry policy 写 retry_wait/failed，
再 paused；resume 时才向 `run()` 传播结果。进程 crash 后按 durable pause flag 收敛，不能恢复成正常执行。

Pause 与 run terminal 竞争以 scheduler transaction 顺序为准：terminal 先提交，pause 返回 conflict；
pause flag 先提交，业务 complete/errored 延迟到 resume 后 replay 收尾，已完成 steps 保留。
不可恢复的平台 invariant failure 仍可 fail closed，不被 pause 隐藏。不要丢弃已知 step output，只为凑出 paused 状态。

### 11.2 暂停时 timer 继续走

本地采用 wall-clock 语义：sleepUntil 的目标时间、相对 sleep 首次得到的 due、retry due 和 event deadline
都不因 pause/resume 改期。Paused 时可以消费及时事件、记录 timeout/sleep complete，但不能执行 callback。
Resume 看到已满足的等待就排队恢复。这个选择与参考快照中 Miniflare 调整 timer 的做法不同，必须写入
兼容性文档和测试，不能把不一致隐藏在实现里。

Pausing 的 runtime drain 有界，沿用 attempt/dispatch watchdog；无法确认结果时保留 lease 到期恢复。
`waitingForPause` 不能永远只存在内存中，也不能在 crash 后丢掉暂停请求。

### 11.3 Terminate 的承诺限于平台状态

Terminate 在单个 scheduler transaction 中设置 terminated、terminal time/expiry，清 token，将未完成
step 转 cancelled、清除 step grant，留作不可再执行的诊断历史，停止后续 claim。不可恢复的 run failure
也需关闭仍未完成的 step。System controller 下次 RPC 得到 stale/closed，late success/failure 不可覆盖 terminal。

不递增 execution generation，也不删除已完成 output；只有 restart 会产生新执行 generation。
不尝试回滚已写入的 D1/R2、已投递的 Queue 消息或外部 HTTP 请求。正在执行的用户代码可能继续产生
外部效果，特别是当前 pin 的断连不能当作可靠取消；UI/status 的 terminated 只表示平台不再推进该代执行。

## 12. Restart 与跨库一致性

### 12.1 从头执行，仍然使用原版本

`restart()` 接受 V2 的 queued/running/waiting/paused/terminal，前提是仍在 retention 内、definition 和
frozen version/deployment 可用，且没有另一个 lifecycle operation。它：

- 保留 internal UUID、external ID、input、creation timestamp、version/class/descriptor；
- generation 精确 +1，清除旧 generation 的 steps、event inbox、output/error、timer 和 terminal expiry；
- 创建新的 queued generation，重新执行 callback；原来的业务副作用不回滚；
- 不使用 definition 当前 version；若要使用新版本，显式 create 新 instance。

这是唯一允许打破“terminal 行不再执行”的产品操作，必须由持久 operation intent 授权，不开放通用
`UPDATE state='queued'` 或 SQL 管理入口。generation 达上限时稳定失败，不能 wrap。

V1 instance 不支持 restart。其 deployment ref 可能已释放，不能为了“兼容”静默选当前部署替代原版本。
V2 retained ref 损坏或 artifact digest 错误也必须 fail closed，不能从 cache 里碰运气恢复。

### 12.2 Restart saga

| 阶段 | 数据库与操作 | crash 后处理 |
|---|---|---|
| R1 prepare | Control：验证 identity/capability/target；预留 active quota；写唯一 restart intent，expected=g、target=g+1；ref 状态变 restarting，仍保留引用 | 没有 scheduler apply 证据时，按同一 intent 继续，不产生第二个 generation |
| R2 apply | Scheduler：验证 UUID/creation identity/g、terminal expiry 和 intent；清旧 steps/events；generation+1、queued；记录 last_restart_operation_id | 重复同一 operation 只读确认，不能再次清空新 generation |
| R3 finalize | Control：看到精确 operation/g+1，更新 ref generation、状态 live，删除 intent | R2 已提交而 R3 丢失，reconciler 完成 finalize 后才允许 claim |

R2 对旧 run/step token 是原子失效点。R1 后 controller/backend admission 阻止新的用户 mutation 和
run claim；R1 前已经 admitted 的操作如果先于 R2 commit，属于旧 generation，其结果随后被 restart
清掉；若晚于 R2，generation predicate 使其失败。不能宣称两个 SQLite 文件之间存在原子锁。

R3 前新 queued generation 不具备 live matching control reservation，禁止 dispatch。状态查询遇到未完成
saga 返回稳定 `WORKFLOW_INSTANCE_BUSY`；operator 可以看到 operation phase，不能呈现虚假的新结果。
`restart()` 只在 R3 commit 后 resolve；收到 Unknown 后客户端不要盲目自动重试非幂等 restart。

每个 facade 调用由 system host 产生 operation UUID。同一内部操作的恢复使用同一 ID；外部再次调用
restart 是新请求，不自动当成同一次。Control operation 表只保留进行中的记录，不形成无界操作日志。

### 12.3 Restart、event、quota 与 retention 竞争

- Event admission 绑定当时的 generation；R2 后旧 generation 的已 admitted event 不能进入新 inbox。
  尚未到达平台 admission 的新请求按当时的 current generation 处理；业务 webhook 应携带业务轮次 ID。
- Restart 和 purge 争同一个 control operation slot，并在 scheduler 再核对 generation/expiry；不能仅看
  prepare 时的 terminal snapshot。
- 已过期 instance 拒绝 restart，即使物理 GC 尚未执行。Prepare 后、apply 前到期也拒绝，不延长 retention。
- R2 确定未执行且因 state/expiry 拒绝时，撤销 R1 的 active reservation，恢复原 ref 分类；R2 outcome
  Unknown 时不得回滚 prepare，必须读取 last-operation evidence。
- R1 对原非 terminal instance 不重复计 active；从 retained 重启则先获得 active quota。R2 清理历史后
  按净变化更新 account state bytes，不能先减两次再加一次。

已实现的拒绝恢复采用单实例单调 `operation_sequence`，独立于 execution generation 和 wall clock。
Control prepare 分配序号，scheduler 的 `workflow_operation_progress` 每个 internal UUID 只保存最近一次
已提交的 applied/rejected 决定。重放较早序号失败，同一序号只能确认同一 operation；因此 R1 撤销、
时钟回退或延迟到达的内部请求都不能重新执行旧 restart。确定拒绝也是持久证据，不能用“尚未看到
restart marker”代替。新操作覆盖旧结果，purge 的结果与 receipt 一同保留到 P4，记录数受实例/receipt
数量约束，不形成追加式操作日志。

## 13. Retention、artifact ref 与 ID 重用

### 13.1 Terminal 不再立即释放 V2 引用

`create({retention:{successRetention,errorRetention}})` 的两个时长在创建时规范化并冻结。
Complete 使用 success retention；errored/terminated 使用 error retention。Terminal transaction 写
`expires_at = terminal_at + retention`；status/read 不滚动延长 expiry，暂停中的实例不自动过期。

P2.4 的 terminal -> `release_instance` 对 V1 保持原样；V2 改为 control `live -> retained`：保留 typed
deployment/definition ref，使 retention 内 restart 有可用原版本。Terminal 与 control retained 之间 crash，
只会保守地多占 active quota/ref；reconciler 从 scheduler terminal evidence 收敛，不能提前放行 artifact GC。

因此，retention 也限制旧代码的保留期，而不仅是结果行的 TTL。Definition/version/Worker 删除需要先
处理这些 retained references。状态检查不能只统计 running instance 而忽略 retained/restarting。
R2/S3 业务对象不属于 Workflow retention；GC 只释放平台 deployment ref，由原 artifact GC 决定是否可删。

### 13.2 Purge saga

沿用一个 control operation slot；不用跨库 transaction，也不一次 `DELETE WHERE expires_at < now`：

1. **P1 prepare**：control 验证 V2 retained identity，写 purge intent。引用继续保留，external ID 继续占用。
2. **P2 delete state**：scheduler 验证 instance/generation/expiry，无 run lease；同事务删除 inbox、dependency、
   step 和 instance 行，准确减少 quotas，并写入 `workflow_gc_receipts`。没有 receipt 的缺行不是成功清理。
3. **P3 release**：control 验证精确 receipt，走 releasing/released，释放 typed refs；在同事务删除 operation
   与 instance reservation，最后才释放 external ID。Trigger 只允许这种有 intent 的删除。
4. **P4 sweep receipt**：确认旧 internal UUID 的 control reservation/operation 已不存在，删除 receipt。

P2 的删除权限来自有界、已校验的 purge context/receipt 约束，不是 SQL 的全局 `allow_delete` 开关。
若物理行多，单实例删除仍受 descriptor/inbox 上限控制；一轮清理最多处理配置页数/时间预算内的实例。
不在正常 request path 上执行 VACUUM，也不通过删 SQLite/WAL 文件回收空间。

Restart 清旧 generation 与 retention purge 是两个不同授权入口：前者 instance 行必须留下并 +1，后者
必须留下跨库 receipt。任何普通 terminal/replay/doctor read 都无权删除历史。

### 13.3 逻辑过期与旧 handle

Expiry 后 get/status/mutation 返回 not-found/expired 类稳定错误；物理清理异步完成。`create` 遇到同 ID
已过期但仍有 purge intent/旧 reservation 时返回 `WORKFLOW_INSTANCE_CLEANUP_PENDING`，或完成一次有界
清理后重试；不能在旧 reservation 仍存在时强行复用名称。

V2 `get/create` 返回的 handle 必须持有 **system-isolate 内的 instance-scoped RpcTarget**，固定 internal
UUID 和 binding identity。公开对象只暴露 external `id`；不能只保存 id 后每次重新按名字解析。
Handle 不固定 execution generation：同一个 instance restart 后仍可使用；每次方法 admission 再固定
generation，防止正在执行的旧请求越过 R2。Retain 结束后，新 create 获得新的 internal UUID，即使
external ID 一样，旧 handle 也只能得到 not found，不能向新实例发事件或 terminate。

这个新 handle 不承载 raw run/step token，也不向 tenant 返回 creation nonce。V1 无自动 retention/ID
重用；facade 与 wrapper 统一从当前 TS 构建，不承诺历史 deployment 的 wrapper bytes。
RpcTarget 的资源生命期由当前 RPC/request 管理，不能跨请求永久缓存；需要长期保存的是 external ID，
后续请求再 `get(id)`，不能序列化或持久化私有 handle。

## 14. 最后实现：有界并行 step.do

### 14.1 只接受显式可恢复的批次

支持的常用写法：

```ts
const [customer, inventory] = await Promise.all([
  step.do("load-customer", async () => readCustomer()),
  step.do("reserve-inventory", async () => reserveInventory()),
]);
await step.do("confirm-order", async () => confirm(customer, inventory));
await step.sleep("cooldown", "10 seconds");
```

只接受从 `run()` 同一个同步 fan-out 段发起、在下一 durable step 前整体 join 的 `do`；
`Promise.allSettled` 可采用同一批次规则。Callback 内调用任何 step（包括 await 之后）、并行 sleep/event、
`Promise.race/any` 的提前继续语义、不完整 join 的动态 DAG，均不在支持范围。
平台拒绝可观察的非法嵌套、重叠或 descriptor 变化，不试图拦截全局 Promise 或静态识别所有 combinator。

### 14.2 批次 identity 与执行协议

仅靠“哪个 Promise 先完成”更新 frontier 会在 replay 时改变依赖，必须固定登记边界：

1. Runner 在 API 调用时同步分配 ordinal/name count，把同一同步段的 do descriptor 收集成有界 batch。
   在第一次 callback 执行前封口，走 `claim-batch`；不是等某个 callback 成功后才确定 batch membership。
2. Step descriptor 增加 `batch_first_ordinal/batch_size`；同批共享上一个 batch 的 settled frontier。
   Backend 同事务验证/登记整个 batch、parent edges 和 quota，任何一个不合法都不发新 grant。
3. System controller 用 `Map<ordinal, grant>` 保管私有 token；向 tenant 返回 token-free 的逐 step verdict。
   Completed/failed 只 replay；到期 do 才发 grant；retry 未到期保持等待。
4. 每个 callback 完成就独立 durable commit；公开给 run body 的本批 Promise 结果经 batch barrier 交付，
   避免 first rejection 后用户过早开始下一 batch。内部持久化不必等最慢 sibling。
5. 若一个 sibling 进入 retry_wait，先 drain 已发 grant 的其他 siblings，再 yield 整个 activation。
   下一次 replay 核对同样 batch；成功 sibling 不再执行，仅对到期的未决 sibling 运行 callback。

这是比任意 JS Promise 组合更窄的本地规则：不会提供 first-settled 提前继续。合法的整体 join 得到正常
结果；依赖“只 await 第一个后就开下一步”的代码必须改成 batch join，不能把未 join 的 sibling 留在后台。

`claim-batch` 只返回有界 metadata/grant，不在同一 HTTP 响应中塞入 16 个 1 MiB replay output。已有
结果按 ordinal 单独读取或使用带 byte cap 的分页，仍验证完整 run fence；单条私有 envelope 保持在现有
2 MiB 级别上限内。System controller 的 serialized replay working set 最多 16 MiB，不能仅限 batch 个数；
已交付给用户代码的对象另受 workerd 的 heap/runtime limits 约束。

所有批次声明必须可重复。Replay 中缺少某个 sibling、拆成不同批次、换名字/config/依赖，都返回
`WORKFLOW_NON_DETERMINISTIC`。需要至少一个 crash fixture 覆盖“先完成 B、A retry；恢复后只有 A 执行”。

### 14.3 并行与控制状态

- Retry/suspension flag 出现后，不允许新 batch；同 run 已获 grant 的 sibling 可提交。
- Pause drain 所有已获 grant；terminate/restart 原子失效整个 grant map，所有 late callback 都不能提交。
- Unhandled error 不可在 sibling 仍可能提交时直接 terminal success；先 drain/fence，再决定 terminal。
- V2 complete 条件是 registered history 全部被本次 replay 遍历、全部 settled、没有 pending grant、run
  真正返回。Failed-but-caught step 是 settled，不强行要求 complete count 等于 registered count。
- Batch permit 来自当前 Workflow execution admission；callback 上界受 pool × max_parallel 限制。
  不给每个 sibling 再 claim 独立 run，也不把公共 backend request semaphore 当作 callback 并发上限。

若 P2.5.8 未通过，前面工作包可以单独标记完成，但 aggregate 只能记 Conditional Go（parallel 暂不开放），
不能把方法存在、内部仍串行或悄悄忽略 sibling 的实现称为完整 P2.5。

## 15. 调度、资源隔离与背压

继续使用 P2.1 的 SchedulerKernel 和独立 Workflow execution pool；不把 Workflow 的 due 写进
Queue message 表，也不借 DO alarm 充当这个 engine 的唯一 scheduler。

每轮先做有界 lease recovery / due resolution / lifecycle reconcile，再执行 bounded claim。Claim 前先取得
execution permit，claim 失败立刻释放。Resume/due 和新 create 两类 ready work 都有保底份额；默认最多
连续选 3 个恢复项后检查新项，account 之间轮转，防止 retry=0 的热实例长期占满池子。
Fairness cursor 是内存优化，丢失不丢工作；SQLite queued/due index 仍可发现全部任务。

Waiting/paused 不占 execution concurrency，但计入 nonterminal quota 和 state bytes；retained 不计 active，
仍计 total/storage。配置下降、磁盘接近满、事件 buffer 满、maintenance admission 关闭，都返回明确
backpressure，不能丢事件、提前 ACK 或把未完成实例当 terminal 以释放容量。

数据库损坏影响隔离沿用 P2.1：scheduler authority corrupt 时停止该 scheduler admission，Worker 的独立
KV/D1/DO 数据不能被误删。`recover-corrupt` 只要存在 Queue/Cron/Workflow authority/ref/history/intent，
就不能重建空 scheduler；新增 retained/operations/receipts 也纳入检查。

## 16. Snapshot、关机与恢复

继续采用 P1 的 maintenance window，不引入在线跨 SQLite/S3 原子备份，也不新增备份保密性要求。
这一阶段增加的是一致性、完整性和可恢复性验证，不是加密工程。

- Snapshot 前关闭 create/sendEvent/modifier/dispatch admission，按 P1 drain 当前 mutation。到 grace
  deadline 仍未确认的 invocation 按既有 Unknown/fence 策略处理，不能假定 HTTP 断开撤销了请求。
- 对 restart/purge intent，要么在窗口内收敛，要么把两库的 intent/marker/receipt 作为完整恢复状态一起复制。
  Snapshot manifest 记录两库 schema release 和 workflow policy，不能漏掉 scheduler events/GC receipts。
- 等待中的实例没有长 run lease，无需等 sleep 到期。Paused/retained 也属于必须纳入的 authority。
- Restore 先按 P1 exact-source-release 流程恢复、核验，再 forward upgrade。恢复后的旧 run token 不再拥有
  当前 workerd generation 的提交权；due 按原绝对时间处理，旧 buffered event 不重复消费。
- 恢复不会回滚 snapshot 之后已发生的 Queue、D1、KV、DO 或外部 S3/HTTP 副作用；replay/restart 都要求
  应用幂等，跨产品 exactly-once 不在 snapshot 的承诺内。

Shutdown 不等待几天的 sleep。停止 admission，drain 短事务/已执行 callback，保留 due/intent，再交给已有
supervisor 停止唯一 workerd 子进程。不得新增一个不受 supervisor 管理的 Workflow runner。

## 17. 模块归属与 API 接入

按现有归属扩展；新增文件名是实现建议，不是已存在的 production source：

| Owner / 现有入口 | 新职责 |
|---|---|
| `crates/core/src/workflow.rs` | capability/config、duration、resolved retry policy、稳定错误和纯状态类型；按职责拆文件 |
| `crates/storage/src/workflows/` | control ref/operation、capability、retained quota、restart/purge prepare/finalize |
| `crates/storage/src/scheduler/workflow/` | step/batch、wait/event、due、retry/timeout、lifecycle、GC receipt 的短 transaction |
| `crates/workers/src/workflows.rs` | 跨库 create/restart/purge orchestrator、immutable target 校验、operation reconcile；不依赖 runtime crate |
| `crates/service/src/workflow_backend.rs` | 严格 public/private wire decode、trusted scope、admission；不直接堆 SQL |
| `crates/service/src/scheduler/workflow.rs` | dispatch、heartbeat、yield/terminal 分类、due/timeout composition |
| `crates/service/src/runtime_bridge/workflow.rs` | V2 request/result protocol、frozen capability dispatch；保留 body-bearing no-pool 修复 |
| `runtime/system-workers/workflows/host.js` | request-scoped run controller、instance-scoped handle、可信 timeout、私有 grant map |
| `runtime/system-workers/workflows/runner-v2.js`（新） | token-free API、replay、batch barrier、control signal；不拥有 SQLite authority |
| `crates/service/src/doctor_workflow.rs` / metrics | 新状态/operations/inbox/due 的低基数诊断 |

新增 private run protocol 使用 `/internal/workflows/v2/runs/`，包括 claim、claim-batch、success、failure、
register-sleep、register-wait、yield；timeout 只能由 trusted host/kernel 提交。V1 endpoint 保留到没有对应
历史合同的未来版本，不把 V1 请求解释成 V2。私有 run identity 总在 tenant fields 后由 host 注入覆盖。

Binding endpoints 可继续沿用既有路由 prefix，由已签入 descriptor 的 capability 决定方法；新增
send-event/pause/resume/terminate/restart，不能仅凭 URL 声称 V2 权限。DO mutation 防护必须同时存在于
tenant facade、trusted transport 和 backend；用户修改 prototype 也不能绕开。

Control API 在现有 account/definition scope 下增加 instance detail 与 modifier/event routes。路径中的
`{instance}` 继续表示 internal UUID，与现有 steps/list API 对齐；external ID 由 binding 的 `get(id)` 解析。
Admin mutation 也使用同一个 lifecycle controller，不能绕过 generation、quota 和 operation slot。
不要求本阶段新增完整 CLI/UI；operator GET 只显示 metadata，payload read 走已有授权的数据面契约。

已实现的 control 路径以 `/v1/accounts/{account}/workflows/{definition}/instances/{instance}` 为前缀：
`GET` 查询 metadata，`POST /pause`、`/resume`、`/terminate`、`/restart` 接受空 JSON object，
`POST /events` 接受 `{ "type": "...", "payload": ... }`。不接收调用者提供的 operation ID、generation
或私有 token。维护窗口拒绝 mutation；detail/steps 对逻辑过期实例返回 not found。

## 18. Error、metrics 与 doctor

### 18.1 错误分类

沿用已有 `WORKFLOW_RUN_STALE/STEP_STALE/NON_DETERMINISTIC/RUNTIME_UNAVAILABLE`、serialization、quota、
binding 和 output-gate 错误。计划新增或补充固定 error code：

| 分类 | 代表代码 |
|---|---|
| capability/config | `WORKFLOW_CAPABILITY_MISMATCH`、`WORKFLOW_DURATION_INVALID`、`WORKFLOW_STEP_CONFIG_UNSUPPORTED` |
| settled 业务错误 | `WORKFLOW_STEP_TIMEOUT`、`WORKFLOW_STEP_RETRIES_EXHAUSTED`、`WORKFLOW_NON_RETRYABLE`、`WORKFLOW_EVENT_TIMEOUT` |
| event intake | `WORKFLOW_EVENT_TYPE_INVALID`、`WORKFLOW_EVENT_QUEUE_FULL` |
| lifecycle | `WORKFLOW_INSTANCE_STATE_CONFLICT`、`WORKFLOW_INSTANCE_BUSY`、`WORKFLOW_INSTANCE_CLEANUP_PENDING` |
| scope/history | 既有 not found/unsupported/invariant 类错误，不泄漏跨 account 是否存在 |

Status error 使用固定 name/message；不输出 callback 原始异常、token、definition 输入或 SQL。
Suspension 是 private protocol outcome，不对外报告为 errored。

### 18.2 观测与修复

增加固定 label 集合的状态 gauge、wait reason、retry outcome、event accepted/consumed/full、timeout、
lifecycle result、purge phase、stale commit 计数，以及 oldest-due/operation-age/inbox-bytes 指标。
Instance ID、step name、event type、definition name 不能作为 metric label。日志可以用既有经过校验的
request/operation correlation ID，但不记录 payload、raw exception 或私有 token。

P2.4 的固定 series 基线为 517；当前 V2 新增状态、等待、retry/timeout、operation、inbox 和 lifecycle
指标后，精确 render 计数测试为 548，默认预算仍为 1024。paused/retained 不计 running。
`workflow_retry_results`、`workflow_timeout_results`、`workflow_consumed_events` 表示保留 generation
中的结果数量，是会随 restart/purge 减少的 gauge；`workflow_event_intake_total` 和
`workflow_lifecycle_total` 表示本进程观察到的已授权调用结果，不冒充跨崩溃的永久累计计数。

Doctor/reconciler 需要检查：

- control/scheduler UUID、creation identity、generation、capability、frozen target 对齐；
- operations 与 restart marker/GC receipt 的合法组合；retained refs 完整；
- registered/settled/completed counts、batch/edge identity、event byte accounting；
- waiting/paused 无 lease，running grant 与 deadline 对齐，terminal 无 ready/due；
- inbox 不含其他 generation、event 不重复消费，due/expiry projection 有对应 authority。

正常读和 dispatch 不自愈。显式 reconcile 只能按已证明的 authority 完成 saga、清理旧 hint/receipt、恢复
合法 projection；不能凭 ready hint 补写缺失 step/output、猜 frozen deployment，或把 absent scheduler
instance 当作 retention 已完成。

## 19. 工作包与逐段验收

### P2.5.0 Runtime Hard Gate

完成第 5 节 probes，明确 suspension 资源释放、timeout/迟到 callback、NonRetryableError 导出、token
隔离与 wrapper 兼容。失败即停在对应能力之前；不能边修改 runtime pin 边假设 probe 已通过。

验收：真实 workerd 上正常长等待不占执行池；prototype probe 看不到任何 grant；晚到结果不能越 fence。

### P2.5.1 Capability、schema 与 replay identity

完成 control 013/scheduler 006、checksum registry、config、V1/V2 wrapper/descriptor、counter/edge、
事件与操作表基础。先用顺序 do 验证新 schema，未上线方法仍 unsupported，不提前开放未验收的 mutation。

验收：P2.4 数据升级后 hash/output/token/config 不变；V1 原 Gate 仍通过；capability 不匹配稳定拒绝；
真实 migration crash/rollback、FK、恶意直接 SQL mutation guards 通过。

### P2.5.2 Durable yield、wake 与 recovery

实现 private yield protocol、due projection、expired-run recovery、budget yield、driver result 分类。
先用 test-support 注册等待，不往生产加测试端点或固定 tick 结果。

验收：register/yield/claim 每个 crash 点都能恢复；事件式唤醒在 register 与 yield 之间不丢；
waiting/paused 没有 heartbeat/permit；旧 run response 不覆盖新状态；Queue/Cron 不被 maintenance 饿死。

### P2.5.3 Retry 与 timeout

实现静态 config canonicalization、attempt/backoff、可信 timeout、NonRetryableError、V2 catch/replay。
暂不并行；run controller 仍按单 grant 验证，接口为后续 bounded batch 保留明确边界。

验收：limit=0/1/default、三种 backoff、timeout-success 竞争、Unknown 不立即耗 retry、deadline 不重置、
已捕获最终错误可继续、未捕获则 errored；同实例重启后 ctx.attempt/config 正确。

### P2.5.4 sleep / sleepUntil

接入公共 waiting protocol，冻结相对 due、绝对 timestamp 和 void replay。

验收：0/过去时间/未来时间、duration overflow、同名 descriptor mismatch、时钟前后跳、process/workerd
restart、snapshot restore；长 sleep 不占 execution pool，先前 do output 不重算。

### P2.5.5 waitForEvent / sendEvent

实现 scoped V2 instance handle、inbox/FIFO、event-before-wait、event-timeout 同事务裁决、byte caps。

验收：wait 前/后到达、同/异 type、timeout 临界点、并发发送、丢响应、duplicate payload、quota full、
snapshot 恢复、跨 account/definition 拒绝、DO 内 mutation fail closed；不能以 mock 证明事件不丢。

### P2.5.6 Instance modifier

依次实现 pause/resume、terminate、restart saga；先测试单库逻辑停止，再加跨库 generation handoff。
即使 retention GC 尚未上线，V2 terminal 也必须转 retained，不能提前释放 restart 需要的引用。

验收：paused timer/event 语义、pause-terminal race、无限等待不阻塞 pause、late callback、重复请求、
restart 清 inbox、active quota、版本冻结、两个数据库的每个 operation crash 点、artifact GC 竞态。

### P2.5.7 Retention 与清理

实现 frozen retention、logical expiry、purge intent/receipt、typed ref release、external ID reuse 与旧 handle
隔离。先 dry-run inspect candidate，再用真实事务测试验证，不提供跳过 fence 的 force-delete。

验收：success/error/terminated expiry、未过期引用不能 GC、pause 无 TTL、restart-purge race、P1–P4 crash、
receipt 缺失 fail closed、旧 handle 不能操作新 incarnation、cleanup 后 quota 准确恢复；V1 history 原样保留。

### P2.5.8 Bounded parallel

实现同步 fan-out sealing、batch identity、per-ordinal system grant map、独立 commit、barrier 和 retry drain。
只扩展 do；不顺手开放 parallel wait、callback 嵌套 step 或 arbitrary DAG。

验收：4 个 sibling 乱序完成、部分已成功后 crash/retry、批次缺项/重排、first failure 后 drain、pause/
terminate/restart 与迟到 sibling、quota/并发上限、Promise intrinsics；两条 run token 从未同时有效。

### P2.5.9 Aggregate 与 P2 Exit

新增 `p2_5_workflow_product_gate`、`test/test-p2-5.sh`、机器可核对的 Gate 结果模板。
覆盖完整 HTTP -> Queue -> Consumer -> Workflow -> KV/D1/R2/DO 路径；此路径可以由普通 Queue consumer
create Workflow，Workflow 调用 DO，不要求 DO 反向 create/sendEvent，以免绕过已接受 output-gate 限制。

## 20. 必测 crash / race 矩阵

| 故障/交错点 | 必须观察到的结果 |
|---|---|
| retry failure commit 前/后 SIGKILL | 未提交则 Unknown；已提交则按原 due/attempt 恢复，不双增 attempt |
| attempt success 与 timeout 同时 commit | 只出现一个 settled verdict，late token 无效 |
| register-wait 后、yield 前到达事件 | 结果 durable；下一状态 queued，不永久 waiting |
| yield commit 后丢响应 | 不变 errored，不长期保留执行 permit |
| sleep 到期，tick promotion 前 SIGKILL | restart 重新发现 due，不重置到期时间 |
| event insert/consume commit 前后 SIGKILL | 已接受事件不丢；同 event 不完成两个 wait |
| event deadline 同毫秒到达 | 按严格小于规则裁决，不因线程调度改变契约 |
| late tick 遇到 deadline 前 buffered event | 先匹配事件，不误记 timeout |
| timeout 被 run catch，fallback commit 后 crash | replay 仍进同一 fallback，先前结果不重跑 |
| pause 与最后一步/run terminal 竞争 | SQL commit 顺序唯一裁决；不伪造 paused success |
| paused 时 event/sleep/retry 到期 | 不执行 callback；resume 读取原 deadline/持久结果 |
| terminate/restart 后旧 callback 完成 | 所有 stale commit 被拒绝，不能污染新 generation |
| restart R1/R2/R3 任一点 crash | intent 与 marker 收敛到一个 generation，target 不变 |
| purge P1/P2/P3/P4 任一点 crash | 不释放运行所需 artifact；有证据清理，不造成孤儿/误复用 ID |
| terminal expiry 与 restart 竞争 | 仅一条合法操作链；不能重建已过期 history |
| 同 external ID 再 create 后使用旧 handle | not found，不向新 instance 发事件或终止新 instance |
| parallel B 完成、A retry 后 crash | B 只 replay，A 按原 policy 继续；join descriptor 不变 |
| batch 登记/提交中断，下一次形状变化 | stable nondeterminism，不把某个旧 output 当成另一 sibling |
| snapshot 包含 waiting/paused/restarting/purging | exact-release restore 后可 reconcile，再 forward upgrade |
| clock jump / quota full / busy DB | bounded retry/backpressure，无 silent drop、无 busy-loop、其他产品仍有调度机会 |
| token/prototype/header tamper | 私有 authority 不可观察/覆盖；DO mutation 和跨 scope 请求拒绝 |

## 21. 验证节奏与 Exit Gate

### 21.1 开发验证

只跑本工作包的一轮 focused tests。P2.5 与 P2 Exit runner 已支持 `OPEN_COMPUTE_GATE_ROUNDS=1`；
脚本存在不代表完整验收已经通过：

```sh
OPEN_COMPUTE_TEST_WORKERD="$PWD/.temp/runtime-cache/v1.20260826.1/workerd" \
  OPEN_COMPUTE_GATE_ROUNDS=1 ./test/test-p2-5.sh

OPEN_COMPUTE_TEST_WORKERD="$PWD/.temp/runtime-cache/v1.20260826.1/workerd" \
  OPEN_COMPUTE_GATE_ROUNDS=1 ./test/test-p2-exit.sh
```

Runtime 使用现有校验过的 binary，不自动下载。Unit/property tests 覆盖纯 state/config/JSON，storage
tests 使用真实 SQLite，Hard/Product Gate 使用真实进程/workerd。Fault hook 只在 tests/test-support 中，
production 不识别 case ID、fixture URL 或测试账号。

### 21.2 最终验收

源码修复完成并冻结后，P2.5 runner 默认三轮 fresh-process Gate；按变更触达范围一次性回归
P2.4/P2.3/P2.2 以及有关 G0/P0/P1 路径，不在开发迭代中递归重复整条历史 aggregate。
必须通过 AGENTS 规定的 format、clippy、workspace tests、no-default-features、metadata、dependency
boundary、coverage，Rust line coverage 仍不得低于 90.00%。实际 Gate case/count/coverage 由执行结果写入，
不能复制 P2.4 的数字。

Exit checklist：

1. V1 已有数据、descriptors、wrapper/hash 和行为保持；V2 显式选择、无隐式能力升级。
2. Retry/sleep/event 等待 durable，正常等待释放执行资源，重启后不丢 due work。
3. Attempt/context/backoff/timeout 正确；Unknown 与业务 retry 分离，completed step 不重跑。
4. Event-before-wait、FIFO、timeout race、quota、Date envelope 和 replay 全部通过。
5. Pause/resume/terminate/restart 的状态、generation、late commit 与跨库 crash 矩阵通过。
6. Retention 保留/释放引用正确；旧 handle 不跨 incarnation，cleanup 可恢复，V1 不被自动删除。
7. Bounded parallel do 独立提交、整体 join/replay 与异常 drain 通过；没有伪装支持的 parallel wait。
8. System-isolate token 边界、DO mutation fail-closed、frozen loader、transport 修复和精确 G0 D-abort 口径保持。
9. Snapshot/restore、doctor、metrics budget、recover-corrupt 与 shutdown 回归通过。
10. 完整 P2 黑盒链路通过，真实 crash 注入后 Queue 不丢已接受消息、Workflow 不接受 stale commit。

P2.5 最终报告单独记录 Go / Conditional Go / No-Go、真实 pin/schema、执行命令、case 清单、输出路径与
剩余不支持 API。既有 DO output-gate 限制可以保留，但任何新增限制都必须单独说明，不能混入原 allowlist。

## 22. 交付物

- 本文对应的 production 实现、forward migrations、config/types/runtime assets；
- Hard/Product Gate、duration/JSON parity、migration/clock/crash/race fixtures；
- `test/test-p2-5.sh`、`test/test-p2-exit.sh` 及 `docs/p2-5-gate-results.md`（实际验收后生成/填写）；
- 总方案、API 兼容性、operator/config/snapshot/testing 文档同步；
- 明确说明默认本地限制：JSON only、60 秒 step timeout、暂停不冻结 deadline、restart 保留原版本、
  V2 retention 保留代码引用、有界 batch parallel、DO 内 Workflow mutation 暂不支持。

完整验收前不创建 P2.5 通过报告，不改写历史 results；migration 一旦应用，继续遵守只追加的规则。
