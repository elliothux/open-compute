# P3.2：Service Binding 与原生 Worker 调用

状态（2026-09-01）：**Day1 核心实现与本地最终验收完成，设计已归档；Service 直接 Cloudflare
differential 由[独立验收计划](../acceptance/p3-assets-service-bindings-acceptance.md)继续追踪**。
SB-0 至 SB-5 的本地部分已落到当前 Day1 schema、工具链、stock workerd 原生 RPC、调用预算、
deployment pin、generation 回收与有界指标；`p3-services-hard`、`p3-services-product`、
`p3-services-events` 和 `p3-services-recovery` 均已通过。事件源矩阵实际覆盖 Queue、Cron、Durable
Object 与 Workflow 内的 Service 调用；独立进程 Gate 以 SIGKILL 证明真实 workerd 退出后在途
handle/pin 回收和替换进程恢复。Service contract 已进入 P3.4 catalog；共享 runner 已在真实
Cloudflare 上完成一项 Cache API fixture，但它不覆盖 Service fetch/RPC/event-source/lifecycle，
因此不能给扩展目标的 hosted verdict。后续固定 vinext workload 已取得 Application Go，但产品
Service Binding 组合在其 excluded case 中，不构成 Service differential 证据。

本阶段让 Worker 通过声明的 binding 调用同账户其他 Worker 或自己，支持默认/命名入口的
fetch 与原生 RPC。绑定冻结目标 Worker ID；每次新的 service 调用解析目标当前 active
deployment，再固定这次调用。整个数据调用留在现有 stock workerd 中，不经公网路由，不
引入 HTTP JSON RPC、注册中心、额外网关或新的数据库。

本文细化[总方案](open-compute-workerd-platform.md) P3.2 中的 Service Binding 部分。
[Static Assets 方案](p3-1-static-assets.md)提供目标默认 HTTP 路由；P3.2 的完整 Node API
清单和 P3.3 缓存/Images 仍需分别完成。Service Binding 通过不等于 P3 平台或应用验收完成。

## 实现与当前证据（2026-08-30）

当前实现只有一套 Service Binding 路径：工具链把 Worker 名解析成明确的同账户 Worker ID，
部署事务写入 `deployment_services` 和 descriptor digest；每个调用由 generation-authenticated
loopback authority 重新校验声明、解析目标 active deployment、取得 pin，再把业务参数、
Response/Stream/WebSocket 和 RPC capability 留在 stock workerd 原生 RPC 中。默认 object、
function 与 `WorkerEntrypoint` fetch 共用目标自己的 env；默认/命名 RPC 只暴露受控公开成员。

调用根共享固定预算：深度 16、总调用 128、并发 32、deadline 30 秒。返回的 stream、
WebSocket、`waitUntil` 和 capability 分别以可观察的 drain/close/dispose 协议持有 deployment；
普通完成不靠 TTL 或 GC 猜测。已确认 workerd generation 退出时，旧 frame/handle 与保守 pin
统一失效；删除 Worker 会原子 fence 它的 deployment 集合并拒绝有效跨 Worker 引用。指标
使用固定 operation/outcome 标签及 root/operation/retention gauges，不包含 Worker ID、方法名
或路径；固定 metrics series 从 567 增至 582。

当前可重复证据如下：

| 证据 | 已验证内容 |
| --- | --- |
| `bun run build` | TypeScript 7 strict typecheck、runtime/toolchain/types 构建；Service facade、默认 object/function env、动态 waitUntil、DO/Workflow root scope 与工具链解析 |
| `cargo test -p open-compute-service --lib service_invocations::tests` | 5 个 registry 测试；返回 capability pin、深度/并发/总量预算、跨 root 隔离、generation 失效 |
| `./test/gate.py p3-services` | hard/product/events/recovery 共 4 个 stock-workerd target 通过；报告为 `.temp/gate-run/20260830T162132-c3d07c73/report.json` |
| `./test/coverage.sh` | 完整 workspace 单轮 Gate 通过，Rust line coverage 为 90.11%；报告为 `.temp/gate-run/20260830T170738-1521145b/report.json` 与 `target/llvm-cov/summary.json` |
| `OPEN_COMPUTE_GATE_ROUNDS=3 ./test/gate.py --workspace` | 原生 inventory 校验通过，完整第一轮与两个 fresh-process TIMING 附加轮共 834/834 case 通过；报告为 `.temp/gate-run/20260830T171917-ab19b6a0/report.json` |
| `contract-report.json` | `services.fetch.rpc` 的 hard/product/events/recovery 证据及 `OC-SERVICE-001` 映射全部通过；本地结论为 `contract_go` |
| `p3-services-hard` | primitive/结构化值、TypedArray、Date、Request/Response、Readable/WritableStream、WebSocket、函数 callback、RpcTarget、getter、pipeline、dup/dispose；WeakMap 明确拒绝 |
| `p3-services-product` | 默认/命名 fetch 与 RPC、object-style 默认 fetch、Assets/Assets-only、业务异常、waitUntil、返回/嵌套 capability、callback、self 深度限制、b1 在途固定与后续 b2 动态解析、最终 drain |
| `p3-services-events` | Queue、Cron、Durable Object、Workflow 四类真实事件源调用，保持各自 root/attempt/ack 生命周期 |
| `p3-services-recovery` | 持有 RPC capability/stream 后 SIGKILL workerd，证明退出边界清除 registry 与 deployment pin，替换 generation 的新调用成功 |

核心实现之外仍未完成的 qualification 项目：

- P3.4 已提供固定 Service Binding contract catalog、能力/类型映射和本地产品证据；共享 portable
  runner 的 Cache API 对照不覆盖 Service Binding，仍需直接 differential qualification，因此当前
  产品 Gate 不能单独给最终 Platform Go。
- S14/S15 的本地缺口已经由 `p3-services-recovery` 与 `p3-services-events` 补齐；两项都使用正式
  stock workerd 与生产 authority 路径，不再以 watcher 单测或 scope wiring 间接代替实际行为。
- 截至 2026-08-30 当次证据，vinext 应用产物、workload 和浏览器输入尚未固定；后续 P4 已取得
  Application Go，但其产品 Service Binding 组合明确 excluded，因此不替代本计划要求的 direct
  differential。

## 1. 基线、范围与优先风险

### 1.1 当前接入点

| 当前基础 | 本阶段职责 |
| --- | --- |
| `packages/runtime/src/loader/host.ts` 已使用 `LOADER.get()` / `getEntrypoint()` | 抽出共用加载器，public fetch 与 service call 走同一校验/装配规则 |
| `packages/runtime/src/loader/bindings.ts` 由可信 snapshot 组装 env | 加入 service descriptor 与惰性能力，不递归加载目标依赖 |
| `packages/runtime/src/loader/wrappers/` 适配 env/入口 | 保持默认、命名 RPC 及事件上下文，不吞掉用户导出 |
| `crates/workers/src/runtime_source.rs` 读取不可变 descriptor、artifacts、目标 env | 增加 service 目标解析；目标只拿自己的配置和 secrets |
| `crates/workers/src/pins.rs` 提供进程内删除 fence/pin | 扩展为可由可信 runtime controller 持有的调用存活引用 |
| public ingress 当前 pin 直接附着于 Rust body | 新增没有 Rust body 的原生 RPC/内部 fetch 存活协议 |

按已验收的 [Day1 约束](day1-architecture-cleanup.md)修改当前 schema/descriptor/wrapper，不保留
旧开发版绑定形状或两套 RPC 引擎。生产仍是一个 `platformd`、一个 verified workerd，正式
pin 来自 [workerd.lock.json](../../packages/runtime/workerd.lock.json)，当前为 `v1.20260826.1`。
布局与验收按 [runtime 布局](runtime-and-test-layout.md)和[测试规范](../references/testing.md)。

平台支持面以 Cloudflare Service Binding/RPC 官方契约、固定 workerd pin 与 P3.4 catalog 为准。
可选 vinext qualification 沿用总方案的
[`5d0b53088c689b75d63672eab6ff66434afa5b3b`](https://github.com/cloudflare/vinext/tree/5d0b53088c689b75d63672eab6ff66434afa5b3b)，
完整输入和用例清单由独立 application manifest 固定；本阶段不更换版本、不关闭框架能力来绕过失败。

### 1.2 承诺与非目标

基础交付包含：

- 同账户跨 Worker 和 self binding；目标无需配置公共域名或路由。
- 默认 `fetch(Request | URL | string, init?)`，以及命名 `WorkerEntrypoint` 的 fetch。
- 默认/命名 `WorkerEntrypoint` 的公开 RPC 方法，参数/返回值使用原生 RPC。
- 惰性加载、warm/cold 一致性、发布/回滚中的单调用固定、有限循环调用。
- Request/Response 流、异常、`waitUntil`、WebSocket、RPC capability 的实际生命周期验证。
- 可选应用 qualification 覆盖选定 vinext workload 实际使用的 service API，但不为
  `WORKER_SELF_REFERENCE` 写特殊分支，也不扩大平台支持面。

RPC 支持面必须按类型和操作记录。primitive/结构化值、函数 callback、`RpcTarget`、
Readable/WritableStream、Request/Response、`dup()`/dispose、promise pipelining 和公开
getter 分别验证，不能因一个 `add(1, 2)` 成功就全标支持。公开 prototype 方法是第一工作包；
其他项目由 SB-0 确定原生转发能否保持，已被基线启用用例依赖的项目失败即阻塞交付。
保留方法、字段可见性和不支持值的拒绝由原生规则约束，不按任意 JS 对象反射导出内部状态。
外部契约参见 [Service RPC](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/rpc/)
与 [Workers RPC](https://developers.cloudflare.com/workers/runtime-apis/rpc/)。

不做跨账户 service、按任意 URL/digest 加载 Worker、租户自行指定目标版本、Cloudflare
多地域调度或完整环境/渐进发布管理面。Service Binding 不替代 Durable Object：它不提供
单对象串行执行或持久状态，也不在调用失败后自动重试可能已有副作用的方法。

### 1.3 先验证什么

数据库表和 `getEntrypoint()` 不是最高风险。先验证三件事：

1. 动态目标代理经过现有 wrapper 后仍保持原生 RPC 类型、权限可见性和资源释放。
2. 同一个暖 isolate 并发处理不同 root event 时，调用链身份与预算不串、不被每跳重置。
3. 没有 Rust HTTP body 的调用，其后台任务、流、返回能力仍能阻止 deployment 提前删除，
   并在实际完成后释放引用。

这三项放在 SB-0 的真实运行时 Gate。若当前 stock pin 无法证明，不先做完整管理 API 再靠
超时猜测补洞；记录不支持面或阻塞并回到模型。不能未经决定 fork workerd、下载新 pin，
也不能改用 JSON 把困难类型序列化掉。

## 2. 对外配置与冻结规则

### 2.1 配置与 authority

工具链可接受 Cloudflare 风格的声明，再在 deploy 时解析成平台 ID：

```json
{
  "services": [
    { "binding": "CATALOG", "service": "catalog", "entrypoint": "CatalogApi" },
    { "binding": "WORKER_SELF_REFERENCE", "service": "web" }
  ]
}
```

上例假定当前 Worker 叫 `web`。名字只用于工具链解析；服务端最终接收和保存明确的
`targetWorkerId`、可选 `entrypoint`，不运行时按名字重绑定。任意合法 binding 名都可自绑定，
不引入 `self: true` 与 target ID 两套真值，也不要求框架换名字。

拟扩展部署元数据：

```json
{
  "services": {
    "CATALOG": { "targetWorkerId": "<catalog-worker-id>", "entrypoint": "CatalogApi" },
    "WORKER_SELF_REFERENCE": { "targetWorkerId": "<this-worker-id>" }
  }
}
```

创建调用方 Worker 的 ID 必须先于部署，因此首次 self binding 不需要先有 active deployment。
所有目标必须在同账户存在且未进入删除 fence。允许引用尚无 active 的目标以启动自绑定/循环
部署；控制面报告依赖未就绪，实际调用返回 `SERVICE_TARGET_NOT_READY`，不在 env 装配阶段
递归验证整个依赖图。绑定名与全部 vars/secrets/产品 bindings 共享命名空间，冲突拒绝。

命名入口使用现有合法 export 名校验。部署时可对已有 active 目标做无副作用 export probe，
但它不是永久证明：目标以后可能更新或变成 Assets-only。每次调用依据当时解析出的部署
检查入口；缺失必须失败，不能悄悄退回 default。

### 2.2 调用的四条路径

| binding / 操作 | 实际目标 |
| --- | --- |
| 默认 `SERVICE.fetch()` | 目标部署的 `routeDefaultHttp`，含 Assets/Worker 决策 |
| 默认 RPC，例如 `SERVICE.getUser()` | 目标 default 用户 `WorkerEntrypoint`，绕过 assets |
| 命名入口 `SERVICE.fetch()` / RPC | 对应用户命名入口，绕过默认 assets 路由 |
| `ASSETS.fetch()` | 所属部署资源，不是 service 解析，不跟随 active |

Assets-only 目标只支持默认 fetch，RPC/命名入口返回不支持或入口缺失。普通 object-style
default fetch 按原生方式使用；不能假定随便在 default 对象上添加一个函数就等于可 RPC 的
`WorkerEntrypoint`。Queue/Cron/Workflow handler 不因为名字可猜就自动暴露成 service RPC。

### 2.3 哪个时刻读 active

例：A@a1 声明绑定 B，B 当前是 b1。

```text
t0  A 调用 B.fetch / B.method → resolve b1，取得 pin
t1  B promote 到 b2
t2  t0 的执行、流和返回 RpcTarget → 仍属于 b1
t3  A 再调用同一个 env.B 的新顶层方法 → resolve b2
t4  A 对 t0 返回的 RpcTarget 再调用 → 仍是 b1 的对象，不重新 resolve b2
```

self binding 同理：旧 A@a1 的新 self 调用在 active 已为 a2 时进入 a2；不是无条件递归
回 a1。调用方 deployment 不因目标 promotion 改变 descriptor。目标 Worker 改名不影响
绑定；删掉再创建同名 Worker 得到新 ID，旧绑定不会转移。

单次调用线性化在 authority 成功读目标并取得 pin 的区间内；promotion 后续不使已准入的
调用改投。删除 fence 与 resolve/pin 必须协调，不能在“读完 active、尚未 pin”的空隙删掉
部署。并发失败可以返回稳定 busy/unavailable，但不得重新执行用户方法。

## 3. 元数据与删除引用

### 3.1 control.sqlite

逻辑表按当前 schema 直接整理：

```sql
CREATE TABLE deployment_services (
  deployment_id     TEXT NOT NULL REFERENCES worker_deployments(id),
  binding_name      TEXT NOT NULL,
  target_worker_id  TEXT NOT NULL REFERENCES workers(id),
  entrypoint        TEXT,
  descriptor_sha256 BLOB NOT NULL CHECK(length(descriptor_sha256) = 32),
  PRIMARY KEY (deployment_id, binding_name)
);

CREATE INDEX deployment_services_target
  ON deployment_services(target_worker_id, deployment_id);
```

这是逻辑约束示例，不是可以独立执行的 migration。账户一致性、binding 名唯一、合法入口和
目标 lifecycle 在插入 deployment 的同一事务验证；FK 不能替代这些检查。同步更新 descriptor
与 Rust/TS 严格 DTO。Service 不是 KV/R2 那类 `ResourceId`，不伪造 resource catalog 行。

descriptor 的 canonical 内容包含 binding 名、目标 Worker ID、entrypoint 与调用 policy
身份；不包含目标 active deployment、当前 route generation、明文 secret 或一次调用 token。
目标引用与 deployment 同时提交。查询 inbound referrers 由索引导出，不再维护第二张可
独立更新的关系图。没有运行时 service discovery 表或新的 scheduler.sqlite 状态机。

### 3.2 生命周期规则

被保留的 ready deployment 即使 inactive，也可能回滚成为调用方，因此它的 service 声明
继续保护目标 Worker。已删除/已拒绝且不能执行的 deployment 退出持久引用集合；staging/
validating 引用在部署事务中保护目标，失败后按既有清理流程撤销。

删除目标 Worker 时，先检查其他 Worker 的有效 inbound 声明，再检查运行 pin/已有 DO、
Queue、Workflow 等依赖。存在引用返回 `SERVICE_TARGET_REFERENCED`，附有界、同账户的
referrer 列表。不要为了删目标而静默解除调用方 binding。

删除自身 Worker 时，自身 deployment 的 self 声明随自身一起退出，不应把自己锁成永远
不可删除；但必须先停止新调用并等在途执行结束。跨 Worker 循环 A↔B 不禁止声明：管理员
先发布去掉引用的版本并显式删除仍保留的旧调用方部署，再删除目标。不要“自动递归删除”
整张图，也不忽略 inactive rollback 版本的引用。

删除目标的旧 deployment 与删除目标 Worker 不同：service 声明没有固定目标版本，不单独
保护所有目标旧部署；已准入的调用/返回能力的 pin 保护实际版本。其他产品固定的 deployment
引用仍按其规则生效。备份与恢复保存 ID/声明，预检同账户目标及可达对象；不按名字重建关系。

## 4. 原生运行时装配

### 4.1 组件分工

```text
调用方用户代码
  → 当前事件内的 service facade
  → system ServiceBinding (WorkerEntrypoint / native RPC)
      → Rust authority：验证 binding、准入、resolve、pin（只传控制元数据）
      → 共用 RuntimeSource / WorkerLoader
      → default HTTP router 或目标用户 entrypoint（原生值/流/capability）
```

本地 facade 只适配事件 scope 和 fetch overload，不传业务数据给 Rust。原生 RPC 参数/结果
只在 workerd isolates 之间流动。JSON 可以用于可信控制面 resolve/release，不可以用来编码
用户方法参数、对象、Request/Response 或流。

env 装配只创建绑定到调用方不可变声明的能力：`callerDeploymentId + bindingName + descriptor`
identity。它不加载目标，不保存永久 targetVersion，不复制目标 env。真正调用时才 resolve。
`A→A`、`A→B→A` 的 env 装配因此是有限的；实际无限递归由调用链预算终止。

### 4.2 复用的加载步骤

1. 受信任 controller 取得当前 root/parent frame；没有合法 frame 的调用拒绝，不自行制造
   一个“全额新预算”。
2. Rust 验证 caller deployment 可执行、binding 属于 caller、descriptor 一致、target 同账户
   且未被 fenced；对调用总数、深度、并发和 deadline 做原子准入。
3. 读取目标 active deployment，取得 `DeploymentPin`，复核 lifecycle；返回固定 target
   snapshot identity 和仅 controller 可持有的 call handle。
4. 通过共用 RuntimeSource 验证目标 descriptor/artifact；即使 LOADER key 暖，也不能跳过
   authority。装配目标自己的 vars、secrets、资源能力、outbound 与 compatibility/limits。
5. `LOADER.get(immutableTargetKey, factory)` 后选择默认 router 或
   `getEntrypoint(name, options)`。执行一次用户方法，不做透明重试。
6. 按第 6 节回收 frame 和 pin，返回原生结果或稳定系统错误。

loader key 继续由不可变目标身份及现有需要的 generation 构成；同 key 不得生成不同
WorkerCode。root ID/request ID 不是 WorkerCode 缓存 key 的一部分，也不把每次请求的身份
写入共享 env。不能靠“每请求新 isolate”回避并发上下文隔离。

### 4.3 WDL / Miniflare 的借鉴边界

WDL 的 [ServiceBinding](https://github.com/wdl-dev/wdl/blob/cf4e63e50d5f74ea0f75ed62dda21589cfc9be59/runtime/bindings/service.js)
使用 `WorkerEntrypoint` 和 Proxy 转发到动态加载目标，避免普通含函数对象跨 isolate 的
clone 问题。可借鉴 capability 与动态方法转发，但不能复制永久 `targetVersion`、调用方
secrets 透传或给异常附加平台内部身份的策略。

Miniflare 的 [RPCProxyWorker](https://github.com/cloudflare/workers-sdk/blob/296a1a7c97e027a308740e1eaaa6d904dec8f102/packages/miniflare/src/workers/assets/rpc-proxy.worker.ts)
把默认 fetch 发给 router，其余 RPC 发给用户 Worker。这是 assets/service 组合的参考。
其固定绑定代理并未解决本平台的每次动态 resolve、Rust pin、根预算和 GC，不可直接当成
完成实现。

未知方法转发必须区分普通方法、getter、保留属性和 thenable：不能合成 `.then` 导致
`await env.SERVICE` 永不结束，也不能把内部控制方法通过 Proxy 顺带导出。使用实际 prototype
与 native RPC 规则进行对照；不要用一个 `Record<string, (...args) => ...>` 类型断言宣称
所有 property/pipelining 已支持。方法动态发现失败应有明确拒绝，不以 default fetch 代偿。

### 4.4 wrapper 与全局 env

现有 wrapper 对 fetch、命名导出和产品 facade 的处理必须一起审查。不能只改
`tenantEnv()` 就假定 RPC 可用：默认 class 的 prototype 方法、命名 class、构造器 env、
`this.ctx`、`import { env } from 'cloudflare:workers'` 都要保留正确语义。

当前 `loader/wrappers/runtime.ts` 已通过原生 `withEnv()` 包装构造和方法调用；在此基础上
建立事件 scope，把相同的用户 env 提供给参数、`this.env` 与 importable env，不再另建一套
环境 shim。跨服务、getter 和并发作用域的完整性仍由 SB-0 证明。JS 的
AsyncLocalStorage 不自动跨 JSRPC，不能仅在外层设置一次 ALS。跨 isolate 时通过可信的
内部 dispatch 能力传递 frame，目标 wrapper 恢复本次 scope 后才进入用户代码。

frame、release handle、backend token 只存在于可信闭包/controller；不落到用户可读的
`ctx.props`、env、业务 headers 或 args。用户调用自己的 `withEnv()` 不能制造新 root 或
增加预算。系统注入模块的 import/export 边界必须测试，不能只靠 obscure 名字隐藏能力。
不要重写 Cloudflare 内置模块为一份不完整 shim 来满足 importable env 测试。

## 5. 控制协议、权限与预算

### 5.1 私有 authority 操作

拟新增 `service.resolve`、`service.complete` 和必要的 scope/lease 操作，由 service crate
挂到已有 generation-authenticated loopback backend；不暴露为公网管理路由。

resolve 请求仅含 caller/descriptor identity、binding 名、操作类别（default fetch、named
fetch、RPC）、controller 持有的 parent frame。RPC 方法名只用于受限校验/日志分类，不携带
参数。返回 target deployment/descriptor identity、entrypoint、limits 与 controller 的
call handle。不能允许请求额外覆盖 account、Worker、版本、entrypoint 或资源权限。

complete 幂等，只能结束本 generation、本 controller 获得的 handle；重复完成不减到负值。
完成顺序不代表子任务全结束，lease owner 按第 6 节决定能否真正释放。API 鉴权本身与
“调用方有权使用这条 binding”分别验证；持有一个内部网络地址不构成授权。

每次新调用均验证持久声明和状态。返回的 RPC capability 则是已经授权的对象引用，后续
操作固定其原 deployment，并检查 scope/generation/fence，不重新解释为某条新 service
声明。它可以按原生规则被显式传给第三个 Worker；这属于能力委派，不应错误地声称永不能
转交，但不能借此得到其他资源或任意目标的调用权。

删除 admission fence 只拦新的顶层调用，已准入 scope 内的继续调用允许完成；不能一边
等旧 RpcTarget drain、一边禁止完成它所需的操作。generation 已失效或显式终止的 scope
则拒绝继续调用。该区别必须与其他产品的资源删除 fence 协同测试。

### 5.2 调用链预算

沿用现有 CPU/subrequest 限制，再增加平台持有的调用链额度；两者不是同一件事。
当前 loader profile 的 `cpuMs: 50, subRequests: 16` 不能未经测量就宣称适合任意应用。
SB-0/P3.0 用 portable contract fixture 确定普通产品 profile；应用 qualification 可以暴露需要调整的
预算，但调整后必须对同类 Worker 一致生效，不能只给 vinext 放宽。

初始建议：深度 16、每 root 最多 128 次 service 调用、32 个并发子调用；HTTP/RPC 初始
执行 deadline 30 秒。它们是待测默认值，不是 Cloudflare 精确限制。后台任务、长流和
WebSocket 各有独立有界策略；不能把 30 秒套给所有长连接后宣称已兼容。拒绝无限预算。

root frame 在可信入口建立：HTTP、Queue batch、Cron、Workflow attempt、DO request/alarm
等分别沿用事件原有生命周期。子调用只递减共享额度，深度来自真实 parent；并发 siblings
也共享总量。等待/循环/失败不会补充额度。Queue 重投或 Workflow 新 attempt 才按原有产品
规则成为新的执行事件；持久 Workflow 等待不长期持有 service frame。

target 再次 service 调用、RPC callback 和可继续调用的返回 capability 必须保持原链约束。
native workerd 如果能对某类 RPC callback 保持有界递归/资源限制，应记录实测；不能仅给
“经过 resolve 的调用”计数，却允许 callback 绕开所有预算。无法观测的路径需要受信任
wrapper 补齐或在 SB-0 判为支持面阻塞，不把计数器放到租户可改的 header。

admission 采用短临界区，只保留父子关系和计数，不持有 SQLite 写事务等待 Worker 执行。
链路额度不足立即失败，不能因 A 等 B、B 等 A 又遇到全局 semaphore 而死锁。重入和并发
扇出必须独立测试。不得用 public HTTP 子请求来实现循环调用，因为这会重建 root 预算。

## 6. 生命周期与取消：SB-0 的硬门槛

### 6.1 谁持有 pin

Rust 的 `DeploymentPins` 是唯一删除 fence 权威。拟新增进程内 `InvocationRegistry` 持有
RAII pin 与 root/child 关系；它不是持久消息系统，不写 scheduler.sqlite。controller 拥有
generation-scoped handle，通过原生 capability/可信事件完成协议通知 registry。

一个调用至少区分 `admitted → executing → result-returned → drained`。`result-returned`
不自动等于 drained。可保守地把子 deployment pin 保留到整个 root drain，但必须有可证明
的 root 完成信号和有界资源占用；不能永远等 JS GC 或用固定延迟当完成证明。

| 执行形态 | 可以释放引用的证据 | 不能当作证据的事件 |
| --- | --- | --- |
| 标量/结构化 RPC | 方法完成，后台任务/子调用结束，且无存活能力 | 外层 Proxy 的 `finally` 执行 |
| HTTP/Response/ReadableStream | producer/consumer 完成或确认终止，相关后台任务结束 | `fetch()` resolve 或仅响应 header 发出 |
| `waitUntil` | 可信 wrapper 跟踪的任务完成/已确认终止 | 主 handler 返回、Rust body 被 drop |
| WebSocket | 双向连接关闭/确认运行时终止及相关任务结束 | 101 handshake 完成 |
| `RpcTarget`/callback/stream capability | 原生 execution context 与所有派生存活引用结束 | 第一次 RPC return、调用方某一个 stub dispose |
| workerd 崩溃 | supervisor 确认该 generation 子进程已退出 | 心跳超时或新 generation 已创建 |

原生 RPC 的执行上下文会随传入/返回 stub 延长，dispose/dup 还有独立规则，见
[RPC lifecycle](https://developers.cloudflare.com/workers/runtime-apis/rpc/lifecycle/)。
不能把远端对象重包装为 JSON ID 然后宣称已保持这些语义。

### 6.2 拟定验证路径

先用可信 wrapper/controller 关联 root scope 与 Rust lease：wrapper 跟踪用户处理器、
`ctx.waitUntil`、服务子调用和流终止；RPC 能力的存活应与 stock workerd 的执行上下文
关联。内部 lifetime capability 的 dispose 通知可以作为候选机制，但必须实测它在返回
`RpcTarget`、嵌套对象、callback、`dup()` 和 promise pipeline 时不会提前触发。

这里有一个明确的待证点：**workerd 是否能让本平台准确观察原生 RPC 资源的最终 drain**。
`RpcTarget[Symbol.dispose]` 并不天然等于整个用户调用结束；仅给代理加一个 disposer 不够。
SB-0 必须给出可执行的最小证据和选定实现，再进入 SB-4；本文不把假设写成现有能力。

如果精确观察不到，安全状态是保持 pin、拒绝删除并暴露稳定 busy/诊断。可以通过受控关闭
相应连接或已确认退出的 workerd generation 最终 drain，但它会影响其他 Worker，不能在
普通调用超时或管理删除时静默重启整个运行时。只能把它作为显式运维恢复操作，不能把这种
保守状态计为正常 pin 释放测试通过。不能为赶进度增加无证据的 TTL 自动放行。

### 6.3 取消与进程恢复

取消尽量传播到 Request/body/RPC 子操作，释放已经确实结束的资源。既有 G0 `D-abort`
限制仍意味着“客户端断开”不是可靠的用户代码停止证明；尤其不能提前释放 pin 后允许旧
代码继续写入已被删除/复用的资源。

deadline 到达先关闭新 admission、标记 cancelling、主动终止可终止的流与调用；确认 drain
前仍保护部署。对仍在执行的副作用不自动重试。租户得到稳定 timeout/unavailable，不得到
“已回滚所有副作用”的错误保证。

workerd 退出后，supervisor 确认进程死亡，fence generation，再统一释放该 generation 的
registry。平台进程崩溃后的重新启动必须处理/拒绝旧子进程存活，不能只换 token 并假设旧
进程已停。新 generation 重新读当前 authority，不恢复旧 JS stub 或重放 RPC。泄漏检测
覆盖失败、取消、异常和空响应，不以服务重启后计数归零代替正常释放。

## 7. 安全、错误与观测

### 7.1 隔离规则

目标加载自己的 env/outbound。A 拥有的 D1/R2/KV 权限、secret 和默认 outbound 不能因为
A 调用了 B 就变成 B 的 env。目标 B 可以按应用逻辑返回数据或主动把 capability 传给 A，
但平台不自动做这种委派。binding caller 并不因此获得 B 的 runtime-source/control token。

service fetch 保留应用 URL、method、query、body 和业务 headers，包括应用主动转交的
认证信息；URL host 不是目标选择器。内部保留 headers 不从业务输入信任，跨调用应剥离或
由可信入口生成。不得把 caller ID/预算放进一个可以伪造的业务 header。backend endpoint
不能作为 outbound SSRF 捷径，沿用现有 egress 隔离。

native RPC 仅暴露允许的公开方法/getter；测试不能读出 `env`、`ctx`、构造器、私有字段、
prototype 或 system helper。上游保留方法按原生语义处理，不用自制黑名单替代 workerd
可见性规则。平台 helper 本身也不能因为继承 `WorkerEntrypoint` 而多暴露控制方法。

### 7.2 错误契约

拟新增/复用的稳定码：

| 失败 | 结果 |
| --- | --- |
| 未声明/伪造 descriptor/跨账户 | `SERVICE_BINDING_DENIED`，不披露目标存在性 |
| 目标未 active、删除中或运行时不可用 | `SERVICE_TARGET_NOT_READY` / `SERVICE_UNAVAILABLE` |
| 指定入口或公开方法不存在 | `SERVICE_ENTRYPOINT_NOT_FOUND` / 原生缺失方法错误 |
| 预算或 deadline | `SERVICE_LIMIT_EXCEEDED` / `SERVICE_TIMEOUT` |
| 目标部署完整性错误 | 既有 invariant/integrity 系统错误，绝不 fallback 到别的 deployment |
| 管理删除受保护 | `SERVICE_TARGET_REFERENCED` / `DEPLOYMENT_REFERENCED` |

业务 fetch 返回的 4xx/5xx 原样作为业务 Response；不自动解释为可重试平台失败。
业务 RPC 异常保持固定 pin 的原生错误传递/序列化规则，平台异常只返回稳定、脱敏的分类。
不把 S3 异常、内部 URL/token 或 loader identity 附加到用户异常 message。错误发生在已
发送的流上时终止流并记录分类，不能替换为一个从未发送的 JSON 响应。

### 7.3 观测与限额证据

指标包括 resolve/加载/调用延迟、cold/warm、在途 root/child/pin、深度/总量拒绝、取消未
drain、目标不可用、RPC release、WebSocket 数。metrics 不使用任意方法名/Worker ID/路径
作无界标签；受限调试事件记录 root/call/parent/request/deployment identity 与稳定错误。
不记录 RPC 参数、返回值、body、cookies、完整 query 或 secrets。

测量需区分额外本地 authority 往返与原生执行耗时。SMB 首版可以每次调用查 authority；
只有测量证明必要后再考虑带失效证明的缓存，不拿暖 WorkerLoader 命中代替鉴权。

## 8. 工作包与测试

### 8.1 按依赖实施

| 顺序 | 工作包 | 依赖 | 退出条件 |
| --- | --- | --- | --- |
| SB-0 | 原生 RPC/上下文/存活期 Gate | 现有 runtime pin、P3.0 平台契约输入 | 明确 fetch/RPC 类型矩阵，证明链预算与 drain 机制；未证项明确阻塞 |
| SB-1 | schema/descriptor/声明导入 | SB-0 可行性结论 | IDs、same-account、self/cycle、命名冲突、事务与引用约束 |
| SB-2 | authority 与共用加载器 | SB-1 | 每次 resolve+pin、暖缓存校验、lazy env、目标自己的配置 |
| SB-3 | 原生 fetch/RPC 接入 | SB-2、Assets SA-3 | 四条路由正确，默认/命名/self/跨 Worker 和 streaming 正常 |
| SB-4 | 根预算、pin、删除与恢复 | SB-0 选定协议、SB-3 | 所有返回形态和事件源下不提前释放、不泄漏；超时/崩溃安全 |
| SB-5 | 产品 conformance qualification | SB-4、Assets SA-4/SA-5、P3.4 harness | portable contract、Cloudflare differential、事件源/crash 与现有产品回归 |
| SB-A1 | 可选应用 qualification | SB-5、独立应用 baseline | 选定 vinext self/service workload 的正常 build/deploy/browser 与应用报告 |

SB-0 可在资产上传工作同时推进，因为它不依赖完整 Assets；SB-3 的组合验收必须使用真正
SA-3 路由。SA-4/SB-4 共同依赖的是 SB-0 产出的存活协议，不让两份方案互相等待“对方整阶段
先完成”。不同工作包可以有中间结果，但 SB-0 失败不能跳过后称 SB-5 完成。
当前 SB-0 至 SB-4 的实现早于正式 P3.4 catalog；已有 Gate 保留为核心证据，SB-5 负责把它们映射
到 contract、补齐缺口并执行 differential，不能由既有 PASS 直接推导 Cloudflare conformance。

### 8.2 运行时与 authority 矩阵

| 编号 | 用例 | 关键断言 |
| --- | --- | --- |
| S01 | 默认 fetch、命名 fetch、默认/命名 RPC | 正确入口；默认 object fetch 与 class RPC 分别验证 |
| S02 | B 无公共路由，A 调用 B | 不经 public listener/DNS；URL hostname 不改变目标 |
| S03 | B 带 Assets、B 为 Assets-only | 默认 fetch 走 router，RPC/命名入口绕开；不支持入口明确失败 |
| S04 | 冷启动 self、A↔B、未知目标/未 active | 装配不递归；实际循环受预算；未就绪稳定失败 |
| S05 | B 在 A 连续调用间 promote/rollback | 新调用跟随 active，在途 b1 流/RPC 对象仍固定 b1 |
| S06 | B 改名、删除/重建同名、目标入口被移除 | ID 不漂移；不回退 default/旧版本；检查删除引用 |
| S07 | 数值/结构化值/二进制/Date 等、非法值 | 原生 clone/拒绝语义，不经 JSON 丢失类型 |
| S08 | callback、RpcTarget、嵌套 capability、dup/dispose/pipeline/getter | 逐项记录原生支持与释放；不会误暴露 then/内部方法 |
| S09 | 大请求/响应、慢 reader、Response/Streams、WebSocket | 背压、首块可提前到达、101 后存活、取消/错误不全量缓冲 |
| S10 | 后台 waitUntil 与返回能力未完成时删除 | pin 拒绝删除；完成后在有界时间释放；无假定 GC/TTL |
| S11 | 并发 root、深递归、siblings/callback 扇出 | 预算不串、每跳不复位、不死锁、不超过全局准入 |
| S12 | 伪造 frame/header/token、跨账户、暖 cache | authority 每次有效，tenant 看不到控制能力/目标 env |
| S13 | 所有 env 访问路径和异常 | 参数、this.env、importable env 一致；系统错误不泄露凭据 |
| S14 | workerd/platformd crash、旧 generation complete | 不重放 RPC；旧 handle 无效；确认退出后回收，不留 orphan 执行 |
| S15 | Queue/Cron/DO/Workflow 内调用 service | 各自 root 生命周期和既有 ack/retry/attempt/fencing 保持 |
| S16 | 无 service 的既有应用 | module wrapper、DO 类、Queue/Workflow handler 和 outbound 无回归 |

S15 中的 Workflow service call 属于具体 step/attempt 的副作用，沿用至少一次执行风险；
不能因底层叫 RPC 就承诺 exactly-once。DO 内部对 Service 的调用不自动改变已有其他产品的
限制，例如已记录的 DO 内 Workflow mutation 不支持；若 mapped supported contract 需要该限制外能力，应
明确补齐或阻塞，不能用旁路 service 隐式突破已有安全策略。

### 8.3 可选应用 qualification

使用固定 vinext 的真实构建与选定 workload 原始断言。发现 self binding 后通过普通部署声明注入，
不改框架逻辑、不针对 fixture 名字分支。覆盖实际产生的 Server Actions/Flight/streaming
请求、importable env 和服务间错误；用真实 target Worker 标记确认调用确实发生。

只有实际列出的 test/project/mode 能计数。依赖 Node API、Workers Cache 或 Images 的失败映射到
对应 contract 或应用/upstream 分类，不作为 Service 平台阶段“全部框架通过”的理由。当前没有
选定 workload 清单和应用通过率，结果文档不得填估计值。该 qualification 不参与 Platform Go
分母。

### 8.4 文件归属与入口

拟新增 `packages/runtime/src/services/` 的 capability、scope、router glue 与严格类型；
共用加载代码留在 `loader/`，不复制整个 host。工具链声明解析在 `packages/toolchain`；
domain/errors 在 `core`，表和引用查询在 `storage`，resolver/lifecycle 在 `workers`，
loopback/InvocationRegistry 与 supervisor 组合在 `service`，子进程桥接在 `runtime`。
不让 `workers` 依赖 `runtime`，不把业务参数传进 `storage`。

拟注册原生目标 `p3-services-hard`、`p3-services-product`，再建立包含二者的
`p3-services` 分组。hard 对应 SB-0，product 对应集成矩阵；同一 RPC 类型/释放不变量
由 hard 拥有，product 只补真实部署、authority 和产品组合断言，不重复执行同一探测。
`test/gate.py` 对分组取并集去重。下面命令仅在注册后可用：

```sh
./test/gate.py p3-services-hard --list
./test/gate.py p3-services-hard
./test/gate.py p3-assets p3-services
OPEN_COMPUTE_GATE_ROUNDS=3 ./test/gate.py p3-assets p3-services
```

开发/修复每次只跑相关目标一轮；全部修复并完成 build/generated、JS、静态
检查与 90% coverage 后统一最终验收：完整 workspace 一轮，登记的时序用例补两轮。
固定类型/权限/序列化矩阵一轮；并发、在途取消、生命周期等三轮，分类必须与原生 discovery
吻合。每轮每个选中的原生目标至多一次、一次构建、失败停止；JS/
浏览器测试和库检查不塞进每轮递归。新目标只有通过独立 TMPDIR/SQLite/S3/端口/generation
隔离审查后才并发。不恢复旧 `/poc` 或旧 `test-p*.sh`。

结果保存 `.temp/gate-run/` 与正式结果文档，列出源码/lock/artifact/测试 executable 身份、
逐项支持面、失败/未运行、限额、实际 pin drain 证据及应用测试映射。两个能力完成并不自动
代表 P3.4 Cloudflare conformance。核心实现完成后归档设计；外部资格拆到 active acceptance。

## 9. 关键选择与交付判断

| 选择 | 原因与代价 |
| --- | --- |
| Worker ID 声明 + 调用时 resolve | 目标可独立发布，额外一次本地 authority 检查 |
| 原生 RPC | 保持对象/流/callback；必须认真解决 scope 与释放，不能靠 JSON 简化 |
| 共享 assets router | fetch 行为一致；RPC 与命名入口必须明确分流 |
| root 共享预算 | 防止循环/扇出逃逸；需要跨 isolate 的可信上下文 |
| 有证据才 release pin | 防止删除仍被使用的部署；无法证明时返回 busy 而非冒险回收 |
| 不做透明自动重试 | 保持副作用边界；调用方仍需业务幂等设计 |

本阶段 Platform Go 需要：声明/目标/入口/原生类型矩阵通过，单调用版本固定，权限与预算没有可
绕开的暖缓存或 callback 路径，存活引用正常释放，删除/崩溃/恢复可靠，所有 advertised contract
有 portable fixture、Cloudflare differential 或明确 deviation 的真实证据。若只能返回字符串但
native 生命周期没有证明，应记录为中间进展，不是完成。应用 workload 另给 Application verdict。
