# W2：Workers Standard ResourceLimits 与运行时自恢复

状态：**待实施**（2026-09-13 重写）。W1 已完成；W2 负责关闭
[`#67`](https://github.com/elliothux/open-compute/issues/67) 及同一故障族，并完成
`OC-WKR-LIMIT-001`。实施统一基于 [`third_party/workerd/`](../../third_party/workerd/) 中的用户 fork，
不等待 upstream 合并。源码身份、upstream base、formal pin 与更新流程由[本目录 README](README.md)统一记录，
本文不复制会过期的 revision 或 digest。

本文取代此前的 W2 草案。旧草案把结构性 upload 边界、产品配额、原生执行限制和 supervisor 恢复混在一张
矩阵中，也只把“请求返回超时”当作最后一道防线，无法彻底解决“子 Worker 忙循环后整个 workerd 仍存活但
不再服务”的问题。本设计按 Day1 当前模型直接建立一套实现，不保留旧 limits 拒绝模式、双协议、兼容开关
或第二套 supervisor。

## 1. 交付结论

W2 必须同时交付三层保护；缺少任何一层都不能关闭 `#67`：

1. **invocation 限额**：workerd 原生按请求执行 CPU 与 subrequest budget。触发请求返回稳定的 limits
   outcome，故障范围不越过所属 Dynamic Worker isolate；同 isolate 的其他在途请求可以随 condemnation
   一并失败，但邻居 tenant 不受影响。I/O 等待不计为 CPU，HTTP wall time 不冒充 CPU time。
2. **isolate 摘除**：CPU termination 或 isolate 级故障把对应 Dynamic Worker isolate 标记为
   condemned，从 loader cache 摘除并结算其全部在途请求。后续调用从 immutable Version 重新创建 isolate，
   不复用可能被终止在任意位置的模块状态；邻居 Worker 和 workerd 进程继续运行。
3. **进程自恢复**：如果 workerd 退出，或进程仍在但受信任的内部数据面不再推进，现有 supervisor 对准确的
   runtime generation 进行确认、撤销凭据、停止并 reap 进程，再按既有 backoff/restart budget 启动新
   generation。`ocd` 不需要重启，SQLite 与 immutable deployment authority 不被重写。

这三层分别处理正常配额、isolate 损坏和执行器损坏。ResourceLimits 不是主机安全 sandbox；进程或 VM 的
外层内存、CPU、文件系统和网络隔离仍由部署环境负责，不能替代 Worker 可观察的 Standard 合同。

## 2. Day1 范围与非目标

### 2.1 本阶段范围

W2 的原生执行范围是 Cloudflare 已公开给 Worker Loader 的 `ResourceLimits` 及其直接相关的 isolate 限制：

| 限制 | Standard 目标 | authority | 执行位置 |
| --- | ---: | --- | --- |
| invocation CPU | 默认 30,000 ms；可配置上限 300,000 ms | immutable Version 的 effective limits | workerd request enforcer |
| invocation subrequests | 默认 10,000；可配置上限 10,000,000 | immutable Version 的 effective limits | workerd request enforcer |
| isolate memory | 128 MiB | Standard profile 常量 | workerd isolate enforcer |
| startup CPU | 1,000 ms | Standard profile 常量 | workerd isolate startup enforcer |
| simultaneous outbound connections | 6 / invocation | Standard profile 常量 | workerd request accounting |

数值来自 Cloudflare 的 [Workers limits](https://developers.cloudflare.com/workers/platform/limits/) 和
[Dynamic Workers custom resource limits](https://developers.cloudflare.com/dynamic-workers/usage/limits/)。固定
Wrangler schema、workers-types snapshot 和 fork source 才是 release qualification 输入；网页变化不能自动
改变产品常量。

CPU 与 subrequest 是首先落地、关闭 `#67` 主路径的 request limits。memory 与 startup 使用独立 isolate
模块；simultaneous connections 留在 request accounting。它们不得迫使 CPU watchdog、supervisor 或管理面
共用一个巨型 enforcer。

### 2.2 已有结构性限制不在 W2 重做

Worker code、multipart metadata、vars/secrets、request body、URL/header 和各产品 binding 的确定性输入边界，
继续由其现有 authority 执行。W2 只审计一次“哪些 tenant 可观察的调用消耗 subrequest”，不复制 KV、D1、
R2、Queues、Vectorize 或 AI Search 的产品限额，也不把内部 fan-out 重复计数。

部署方的全局并发、队列、磁盘水位、数据库连接、进程 RSS/CPU ceiling 和 restart budget 属于
`operator_capacity`。它们可以返回 overload/runtime-unavailable，但不得伪装成 Worker CPU、memory 或
subrequest outcome，也不得进入 `/client/v4` 的 `limits` 字段。

### 2.3 明确非目标

- 不实现 Cloudflare Free/Paid 计费或 account plan；Day1 只有一个 Standard runtime profile。
- 不用请求 wall timeout、进程 RSS 采样、容器 limit 或外部 wrapper 近似原生 CPU/isolate limit。
- 不给每个 tenant 启动一个 workerd；保持一个 `ocd`、一个受监督 workerd、多个隔离 tenant Worker。
- 不为旧 open-compute 配置、旧 runtime snapshot 或曾经拒绝 `limits` 的行为保留兼容路径。
- 不把 `/health/ready` 或内部 startup readiness 当作周期性重启信号。
- 不要求先向 upstream 提交或合并 PR；但补丁按可独立移植的边界组织。

## 3. 故障模型与责任边界

| 故障 | 第一责任层 | 预期结果 | 是否重启 workerd |
| --- | --- | --- | --- |
| JS/Wasm 忙循环耗尽 CPU | request enforcer | 终止 invocation，condemn 对应 isolate | 否 |
| subrequest 第 N+1 次调用 | request enforcer | 在发送前拒绝该调用，返回 limits outcome | 否 |
| isolate heap/startup 超限 | isolate enforcer | 终止并摘除该 isolate，失败其在途调用 | 否 |
| invocation 打开第 7 个 outbound connection | request enforcer | 按 Standard 行为排队，释放 slot 后继续 | 否 |
| 单个请求 header timeout，但内部 liveness 成功 | bridge + supervisor confirmation | 当前请求失败，保留 generation | 否 |
| workerd 退出或 control fd 损坏 | supervisor | 清理、reap、backoff、重启 | 是 |
| PID/control fd 仍正常，但内部事件循环卡死 | functional watchdog | generation-fenced 确认后重启 | 是 |
| 旧 generation 的迟到失败报告 | generation fencing | 丢弃，不影响新 child | 否 |
| 连续真实故障超过 restart budget | supervisor | fail closed，进入现有 Failed/invalid 状态 | 不再自动重试 |
| 外部依赖使平台 readiness 降级 | readiness authority | 停止 admission 或报告 not-ready | 否 |

关键区分是“tenant 被限制”与“runtime 不健康”。正常的 CPU/subrequest 超限永远不能报告 supervisor
unhealthy；否则一个 tenant 可以通过持续触发预算来重启所有邻居。反过来，单个 bridge timeout 也只是
suspicion，只有对同一 generation 的功能性探活失败后才允许重启。

## 4. limits authority 与传递

`limits` 只有一条权威链：

```text
Wrangler / v4 strict decoder
          │
          ▼
immutable Version effective limits
          │
          ▼
signed runtime-source snapshot
          │
          ▼
trusted W1 Worker Loader adapter
          │
          ▼
workerd ResourceLimits → native request/isolate enforcers
```

规则如下：

1. API 边界只接受固定 schema 中存在的字段；拒绝非整数、负数、零、超出 Standard 上限和未知字段。
2. Version 创建时物化最终默认值。runtime、loader 和 workerd 不再各自补一遍默认值。
3. limits 属于 immutable Version；promotion/rollback 只切换 active pointer，不修改历史 Version。
4. tenant 不能通过 WorkerCode、entrypoint 或请求输入提高 Version ceiling。若 Dynamic Worker API 在多个层级
   提供 limits，每一维取已声明值的最小值；省略下层值继承父级 ceiling，而不是 unlimited。
5. trusted loader 把持久化的内部类型转换成 workerd 已有的 `ResourceLimits` DTO。tenant source 和 env 不得
   获得 runtime snapshot、签名材料或内部 fetcher。
6. W2 完成后删除显式 `limits` 的临时 fail-closed 分支及其错误；仓库只保留支持后的单一行为。

W1 当前的 tenant 路径必须全部经过 Dynamic Worker Loader。实施前先用路由清单和 real-runtime Gate 证明
这一点；发现旁路时直接接入同一 loader authority，不为 static 与 dynamic tenant 建两套预算系统。平台
system Workers 是受信任基础设施，可继续使用显式 unlimited/no-op enforcer，但该选择必须在创建点可见，
不能由 tenant 输入决定。

## 5. workerd：隔离的原生实现

### 5.1 文件边界

新增的主要实现放在独立文件中：

```text
third_party/workerd/src/workerd/server/
  standalone-resource-limits.h
  standalone-resource-limits.c++
  standalone-resource-limits-test.c++
  standalone-isolate-limits.h
  standalone-isolate-limits.c++
  standalone-isolate-limits-test.c++
  thread-cpu-clock.h
  thread-cpu-clock.c++
```

允许修改现有 workerd 文件的范围仅为接线：

- `server.c++`：接收 source/WorkerCode/entrypoint limits，计算每维最小值，为每个 `IoContext` 创建 request
  enforcer，在 abort 回调中标记/摘除 condemned isolate；
- 对应 `BUILD.bazel`：加入新 source 和 tests；
- 只有现有接口无法表达 correctness 时，才对 `limit-enforcer.h`、`io-context.c++` 或 Worker Loader API 做
  最小补充，并在 commit message 说明为何不能留在 standalone 模块。

默认不改 Cap'n Proto 配置格式、JSG 公共 API、worker-entrypoint 调度和通用 isolate 实现。上游
[`#6399`](https://github.com/cloudflare/workerd/pull/6399) 已提供的 ResourceLimits 传递接口直接复用；不复制
一套 Loader 或另造公开协议。上游背景与固定源码证据见
[workerd upstream references](../references/workerd-upstream.md)。

### 5.2 request enforcer 的生命周期

实现保持两个不同生命周期：

```cpp
struct EffectiveResourceLimits {
  kj::Maybe<kj::Duration> cpu;
  kj::Maybe<uint32_t> subrequests;
};

class StandaloneIsolateLimitState;

kj::Own<workerd::LimitEnforcer> newRequestLimitEnforcer(
    kj::Rc<StandaloneIsolateLimitState> isolateState,
    EffectiveResourceLimits limits);
```

- `EffectiveResourceLimits` 是内部普通类型。JSG/Loader DTO 在入口立即校验并转换，原生计数器不持有 JS 值。
- `RequestLimitEnforcer` 每个 `IoContext` 一个，拥有本 invocation 的 CPU 与 subrequest 计数。
- `StandaloneIsolateLimitState` 每个 Dynamic Worker isolate 一个，拥有 condemned 状态和共享 failure promise；
  它不保存某个请求的 counter。
- `WorkerService` 不再充当共享的 request counter；否则并发请求会互相消耗预算。

### 5.3 CPU 计量与安全终止

CPU budget 以执行 tenant JS/Wasm 的 OS thread CPU clock 为 authority，而不是 wall clock：

1. `enterJs()` 的 RAII scope 注册当前 isolate、请求、deadline epoch 和执行线程；离开 JS 时注销并累计本次
   CPU delta。I/O await、排队和外部服务等待不消耗 CPU。
2. 一个进程级 watchdog thread 按固定、有界的 cadence 采样已注册执行线程的 CPU clock，并用 invocation
   已累计 CPU 加当前 JS turn delta 判定超限；不得用 busy wait。Linux 使用 pthread/thread CPU clock，macOS
   使用 Mach thread accounting，平台适配只放在 `thread-cpu-clock.*`。采样 cadence 与最大 overshoot 作为
   固定实现常量并进入边界测试。
3. watchdog 线程只做原子状态转换并调用 workerd 已有、允许跨线程使用的 V8 termination primitive。KJ
   promise、cache 和 isolate 清理全部回到 workerd 线程执行。
4. 每次进入 JS 都使用单调递增 epoch。离开、完成或重用线程时先 disarm；迟到 watchdog event 必须同时
   匹配 isolate、invocation 与 epoch，不能终止后续请求。
5. `requireLimitsNotExceeded()` 把 V8 termination 转成稳定的 CPU-exceeded 结果，并触发 isolate condemnation。
   原始 V8 异常、source、loader key 或内部 topology 不进入 tenant response/log。
6. 无法被 V8 interrupt 的长时间 native 操作必须单独列入限制清单和测试证据；在没有可中断点前，不得把它
   宣称为已受 CPU limit 保护。

### 5.4 subrequest 计数

`newSubrequest()` 在实际发送前执行 check-then-increment。预算为 N 时前 N 次允许，第 N+1 次不产生网络或
binding side effect。计数覆盖 tenant 可观察的 outbound fetch/connect 和已声明消耗 subrequest 的产品
binding 调用；产品后端内部 retry、fan-out、embedding 或 storage 操作不重复扣 tenant invocation。

实现必须审计 workerd 的 `isInHouse`/internal channel 语义和 open-compute binding adapters，形成一张由测试
冻结的调用清单。内部 gateway、loader、liveness probe 和 supervisor traffic 永远不计入 tenant budget。

### 5.5 isolate condemnation 与重新加载

CPU termination 可能发生在任意模块状态变更之间，因此“请求失败后继续复用 isolate”不是安全恢复：

1. 首次 fatal limit 把 `StandaloneIsolateLimitState` 原子标记为 condemned，并只 resolve/reject 一次共享
   failure promise。
2. Worker Loader cache 在 workerd event-loop 线程删除该 isolate 的 key。已经持有旧 stub 的调用也必须检查
   condemned 状态并失败，不能重新进入 isolate。
3. 该 isolate 的其他在途 `IoContext` 通过现有 `onLimitsExceeded()`/`abortWhen()` 路径有界结算。
4. 后续相同 immutable Version key 创建全新 isolate；模块初始化重新执行。旧 isolate 只在所有引用释放后
   销毁，不需要强制同步析构。
5. condemnation 只影响该 loader key。邻居 Worker、system Worker 和 workerd PID 保持不变。

memory 与 startup 限制复用这套 condemnation 结果，其计量放在 `standalone-isolate-limits.*`。simultaneous
connection accounting 属于 `standalone-resource-limits.*`，只复用 invocation 生命周期，不触发
condemnation。两者都不能把 heap observer、startup phase 和 request CPU watchdog 耦合。

## 6. supervisor：存活进程之外的功能性恢复

### 6.1 单一状态机

继续由 `crates/runtime` 的现有 supervisor actor 独占 child lifecycle。不得增加旁路 kill task、第二个 restart
loop 或由 HTTP handler 直接发 signal。W2 只向现有状态机加入 generation-fenced evidence 与功能性探活。

建议把新逻辑隔离为：

```text
crates/runtime/src/supervisor/watchdog.rs
crates/service/src/runtime_bridge/health_report.rs
```

现有 `actor.rs`、`mod.rs`、`probe.rs`、bridge transport/dispatch 和 gateway ingress 只做类型接线与状态转移。

### 6.2 独立的内部 liveness endpoint

在 system gateway Worker 增加认证的 `GET /internal/live`：

- 它只证明当前 generation 的 workerd event loop、system Worker dispatch 和 generation credential 能完成一次
  最小 request/response；
- 它不读取 SQLite、S3 或外部依赖，也不复用平台 `/health/ready`；
- 它只在 loopback internal listener 可达，要求当前 generation credential，response 不含 token、PID 或配置；
- startup 仍以 control-fd listen evidence 加现有 `/internal/ready` probe 为准；进入 Running 后才启动 periodic
  liveness probe。

这样 readiness 表示“是否允许 admission”，liveness 表示“执行器是否还能推进”，不会因外部依赖降级制造
workerd restart loop。

### 6.3 generation fencing

当前无 generation 参数的 `report_unhealthy()` 必须替换为显式命令，例如：

```rust
SuspectUnhealthy {
    startup_id: StartupId,
    evidence: RuntimeFailureEvidence,
}

FunctionalProbeResult {
    startup_id: StartupId,
    result: FunctionalProbeResult,
}
```

bridge 取得的 endpoint snapshot 必须同时包含 port、generation credential 和 `StartupId`。actor 只接受与
当前 Running child 完全匹配的命令；A generation 的超时或迟到 probe 不能终止 B。仅在 handler 入队前调用
`with_current()` 不足以保证这一点，因为命令可能在新 child 启动后才被消费。

evidence 使用低基数、无秘密的 enum，例如 response-header-timeout、connect-failed、malformed-internal-response、
periodic-probe-failed、control-channel-failed。不得记录 URL、authorization、loader key、secret、response body
或 raw upstream exception。

### 6.4 suspicion、确认与重启

Running 状态执行两种探活：

- **周期探活**：固定产品常量的 interval/timeout，连续失败达到小阈值后确认 unhealthy。第一版不增加 operator
  tuning surface；测试可通过 `test-support` 注入时钟和更短周期。
- **按需确认**：bridge 遇到 response-header timeout、connection reset/refused 或内部协议损坏时发送
  suspicion。supervisor 对该 generation 立即触发一次 `/internal/live`，而不是直接 kill。

每个 generation 同时最多一个 probe；并发 suspicion 合并。probe 成功则当前请求仍按原错误失败，但不重启；
probe 失败或超时才进入现有 teardown/restart 路径。PID 退出、validated process identity 丢失或 control fd
不可恢复错误属于直接证据，无需再用 HTTP 确认。

确认 unhealthy 后的顺序固定为：

1. 从 Running 撤销 admission，并清除当前 generation auth；
2. 使旧 token 立即失效，结算/中断 bridge、WebSocket、scheduler 和 loader watcher 的在途工作；
3. 按现有 graceful deadline 停止 process group，必要时强制停止，并验证 identity 后 reap；
4. 每次确认故障只消耗一次 restart budget，按既有 backoff 启动新 generation；
5. 从 SQLite authority 和 immutable runtime snapshot 重建，不修复或改写持久化数据；
6. 新 generation 使用新的 startup id 和 credential，通过 startup readiness 后才重新 admission。

证据风暴不重复 teardown 或扣 budget；stale evidence 不扣 budget。若无法验证进程身份或完成安全 reap，保持
fail closed 并进入现有 terminal failure，而不是向可能已复用的 PID 发信号。

### 6.5 transport 边界

`runtime_bridge` 的 response-header timeout 保留为单请求有界等待，但它不是 CPU limit。transport 在以下失败
后上报 suspicion：连接建立失败、header timeout、连接异常终止、认证过的内部响应无法解析。明确的 tenant
limit outcome、普通 4xx/5xx、client disconnect、SSE/response-body idle 和 WebSocket 长连接不得自动报告
unhealthy。

W2 不给整个 response body 增加 wall deadline；合法的 streaming Worker 可以长期保持连接。无流量时的
workerd wedge 由 periodic `/internal/live` 检出。

## 7. 对 `#67` 的完整验收

同一个 real-runtime 场景必须连续证明两道防线，而不是分别用 mock 宣称通过：

1. 部署 tenant A（`while (true) {}`）与正常 tenant B。
2. 调用 A；在配置的 CPU budget 内得到稳定 CPU-exceeded，不等待 bridge 的 30 秒 header timeout。
3. 断言 A 的旧 isolate 被摘除，A 的下次请求创建新 isolate；B 立即成功；workerd PID/startup id 未变化。
4. 通过仅在 `test-support` 可达的通用 runtime-stall fault，模拟 limiter bug/native deadlock：PID 与 control fd
   保持存在，但 `/internal/live` 不再响应。
5. 断言 supervisor 自动确认并重启该 generation，旧 credential 失效、新 credential 不同、无旧 listener
   或 orphan process；无需重启 `ocd`。
6. B 在新 generation ready 后成功，immutable Version、route pin、Durable Object facet 和持久化数据保持正确。

第 2–3 步证明 ResourceLimits 会保护邻居；第 4–6 步证明即使原生限制器或 workerd 自身失效，平台仍可自恢复。
只验证其中一条路径不能关闭 issue。

## 8. 测试与故障注入

### 8.1 workerd 单元与集成测试

- 注入 fake CPU clock/watchdog，确定性覆盖边界前、边界、超限、disarm 和 stale epoch；生产实现仍使用真实
  OS thread CPU clock。
- JS loop、Wasm loop、microtask loop 均可终止；I/O await 不收费；并发 invocation 预算互不影响。
- budget N 只允许 N 次 subrequest，第 N+1 次在 side effect 前失败；覆盖 fetch/connect 和所有纳入清单的
  product bindings。
- Version、WorkerCode、entrypoint 每维 min 规则；省略下层不产生 unlimited。
- CPU termination 后 `finally` 未完成或 module state 半更新时，旧 stub 不能再进入；新 isolate 正常，邻居
  正常，PID 不变。
- 多个在途请求有界结算；condemn/abort 竞态只执行一次；无悬挂 promise、UAF 或迟到 watchdog 误杀。
- memory 覆盖 V8 heap、ArrayBuffer/Wasm backing store 的已支持归属；startup 和 simultaneous connections
  分别覆盖 success/failure boundary。第 7 个连接必须排队而不是提前产生 side effect。无法精确归属的共享
  内存必须记录限制，不能用 RSS 伪装。
- Linux arm64/x64 与 macOS arm64/x64 都执行 CPU clock adapter tests，对齐 formal platform targets。

### 8.2 supervisor 与平台测试

- PID/control fd 存活但 functional endpoint 卡死时自动重启；无请求流量也能由 periodic probe 发现。
- 单个 request header timeout 后 probe 成功：不重启，不增加 restart budget。
- generation A 的迟到 suspicion/probe result 到达 B：B 不受影响。
- 同 generation 大量并发 evidence：单个 probe、单次 teardown、单次 budget consumption。
- process exit/control-channel failure 使用直接恢复路径；反复失败按 backoff/budget 精确进入 terminal state。
- teardown 期间 HTTP、streaming、WebSocket、scheduler、loader watcher 全部在有界时间结算；新 generation 不
  继承旧 credential、port、cache handle 或 in-flight registry。
- 日志、metrics、status 和错误不泄漏内部 token、source、URL、headers、loader key 或 raw exception。
- 成功和失败后无 orphan workerd、遗留 listener 或 secret-bearing temp artifact。

故障注入必须是通用行为（stall event loop、drop control fd、exit child、delay internal response），只放在
tests 或 `#[cfg(any(test, feature = "test-support"))]` 路径。production 不得出现 issue ID、fixture Worker 名或
场景分支。

## 9. 实施顺序

### W2-R0：authority 收口

- 固定 Standard 常量、strict decoder、immutable Version 字段和 runtime snapshot schema。
- 证明所有 tenant execution 都经过 W1 Loader；列出 subrequest 消耗点。
- 删除与当前 Day1 模型冲突的旧 limits 默认或临时配置，不保留双读写。

### W2-R1：request ResourceLimits

- 增加 `thread-cpu-clock.*` 与 `standalone-resource-limits.*`。
- 接通 `enterJs()`、`newSubrequest()`、limits outcome 和 Dynamic Worker 每层 min。
- 完成 JS/Wasm/microtask/I/O、并发和 stale watchdog tests。

### W2-R2：isolate condemnation

- 增加 per-isolate state、cache 摘除、old-stub 拒绝和全部在途调用结算。
- 证明 tenant A 超限不影响 B，workerd 不退出，A 可从 immutable Version 干净重建。

### W2-S1：generation-fenced evidence

- endpoint snapshot 加入 `StartupId`；替换无 generation 的 `report_unhealthy()`。
- actor 丢弃 stale report，合并同 generation evidence，并冻结单次 budget 语义。

### W2-S2：functional watchdog

- system gateway 增加认证 `/internal/live`。
- 加入 periodic probe、suspicion confirmation、single-flight 和 teardown/restart 接线。
- 覆盖无流量 wedge、false positive、evidence storm 和 credential rotation。

### W2-R3：剩余 Standard runtime limits

- 在 `standalone-isolate-limits.*` 完成 memory/startup，在 request accounting 完成 simultaneous connections。
- 复用 condemnation，不把进程 RSS/operator ceiling 混入 Worker outcome。

### W2-P：平台合同与 formal pin

- 接通 Wrangler/v4 settings、Version、capabilities/deviations 和 Workers Logs outcome。
- 在 fork revision 冻结后完成四平台构建与 digest/version/source provenance，协调更新 formal pin 和 Git LFS
  archives；开发 binary 不得绕过 pin verification。
- 依次完成 focused tests、静态检查、coverage 和一次最终 workspace Gate。测试不得隐式下载 runtime。

R1/R2 与 S1/S2 可在独立提交序列中开发，但合并后的 `#67` real-runtime test 才是关闭条件。R3 不修改
request watchdog 或 supervisor state machine，只复用稳定的 isolate failure contract。

## 10. 完成定义

### 10.1 `#67` 关闭条件

- CPU/subrequest 原生执行、isolate condemnation、generation-fenced functional watchdog 全部进入 formal
  pinned fork；
- 第 7 节 real-runtime 场景在正式固定 binary 上通过；
- 正常 tenant limit 不重启 workerd，真实 runtime wedge 无需重启 `ocd` 即恢复；
- 在途工作、credential、process group、listener 和 restart budget 的清理/轮换均有回归；
- capability 和 deviation 不再把这条故障描述为只能人工重启。

### 10.2 W2 / `OC-WKR-LIMIT-001` 完成条件

- CPU、subrequest、128 MiB isolate memory、1 秒 startup 和每 invocation 6 个 simultaneous connections 都由原生代码
  执行，并有边界、并发、故障和恢复测试；
- Wrangler/v4 → Version → snapshot → Loader → workerd → logs/outcome 全链一致；
- tenant、platform-hard、product 和 operator-capacity 四类限制在 API、capability、错误与文档中不混淆；
- stock comparison、fork source identity、四平台 artifacts 和 formal pin 协调完成；
- 文档声明的已知 native interruption/memory-accounting 限制与实际证据一致，不夸大兼容性；
- 按仓库规则完成 coverage 与一次最终 workspace Gate，失败证据保留且无 process/secret 泄漏。

在这些条件全部满足前，未由正式固定 workerd 执行的字段继续 fail closed，`OC-WKR-LIMIT-001` 保持开放；
不得以“配置已接受”“bridge 会超时”“supervisor 能重启普通 crash”或 mock 结果宣称完成。
