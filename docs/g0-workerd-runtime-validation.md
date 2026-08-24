# G0：workerd 动态运行时可行性验证

状态：待实施  
性质：架构 Gate / disposable spike  
后续阶段：P0 Workers、binding framework、Durable Objects  
关联方案：[SQLite + workerd 单体平台方案](./sqlite-workerd-platform.md)

## 1. 结论先行

G0 只回答一个问题：**不 fork workerd，能否用一个受监督的 workerd 进程，稳定承载动态
Worker、受限 host binding 和原生 SQLite Durable Object facet。**

验证顺序必须是：

```text
固定 workerd
    ↓
静态 host runtime 可启动
    ↓
workerLoader 加载 immutable Worker A/B
    ↓
loaded Worker 只能访问 binding-scoped adapter
    ↓
动态 DO class 作为 native facet 运行
    ↓
DO SQLite 跨 workerd crash/restart 持久化
    ↓
版本切换只重启 facet，不改变 storage identity
```

全部硬门槛通过后，才进入 P0 control plane。失败时应调整运行时架构，而不是先用业务代码
绕过。G0 的 fake backend、fixture metadata 和测试路由都不得直接升级为生产实现。

## 2. 为什么需要单独的 G0

整体方案的大多数复杂度可以在普通应用层解决：SQLite schema、S3 key、resource lifecycle、
Queue lease 和 Workflow replay 都能自行实现。但下面四项由 workerd 的真实行为决定：

1. `workerLoader` 的 cache identity、cold load 和 dynamic Worker 生命周期；
2. `WorkerStub.getEntrypoint()` 是否能安全注入 immutable props 和受限能力；
3. `WorkerStub.getDurableObjectClass()` 与 `ctx.facets.get()` 能否保留原生 DO 执行/存储语义；
4. `localDisk` 中 facet SQLite 在进程崩溃、重启和代码换版后的行为。

这些假设如果错误，后续 control、KV、D1、R2 做得再完整也无法补救，因此必须最先用真实
workerd 黑盒验证。

### 2.1 三类参考的分工

G0 同时参考 workerd、Miniflare 和 WDL，但三者承担不同角色：

| 来源 | 在 G0 中的角色 | 不能替代什么 |
| --- | --- | --- |
| workerd upstream | runtime contract 和最终 Hard Gate | 不能替代宿主/control 设计 |
| Miniflare | workerd process/config harness、plugin/service 装配和本地 persistence 参考 | 不能替代生产隔离、immutable deployment 和 crash contract |
| WDL | dynamic loader、binding-scoped capability、stable DO storage identity 参考 | 不把 Redis、多副本和微服务拓扑带入 G0 |

Miniflare 尤其值得参考以下已落地模式：

- 宿主生成 binary Cap'n Proto config，通过 stdin 传给 workerd；
- 使用 control fd 等待必需 socket 的 `listen` 消息，而不是固定 sleep；
- 缓冲 startup stderr、处理 structured logs，并区分 startup failure 和运行后 crash；
- config/plugin 分别生成 binding、entry service、embedded system Worker 和 disk service；
- D1/R2 用共享 entry service 配合 `ctx.props` 传递具体 resource ID；
- `resourcePersistencePath` 按 plugin 分目录，并让 workerd `localDisk` 管理 SQLite。

但 Miniflare 是面向本地开发和工具作者的 simulator。它允许 Node bridge、direct binding/storage
access、dev registry 和 config reload，这些都不是 tenant-safe production surface。它当前的
Queue broker 还是 in-memory，因此不能作为 Queue persistence 或 recovery 的正确性依据。

Miniflare 已有的 worker-loader plugin 只负责生成 `workerLoader` binding，证明 config wiring
可以很薄；它没有替本方案定义 immutable deployment key、active route、bundle authority 或
tenant authorization。后四项仍必须由本 G0/WDL 思路验证。

因此测试层级固定为：

```text
可选：Miniflare differential/reference test
                ↓
必须：自有 config + pinned stock workerd 黑盒测试
                ↓
必须：SIGKILL/restart + 原数据目录恢复测试
```

只有后两层决定 G0 是否通过。某个 fixture 只在 Miniflare 中通过，不构成 Go 证据。

## 3. G0 边界

### 3.1 必须验证

- workerd 使用明确版本和 checksum，不跟随 `latest`；
- `platformd` 能启动、探活、终止和重启 workerd child；
- 一个 workerd config 内同时运行 ingress、loader host、binding host 和 DO supervisor；
- module Worker 可通过 `workerLoader` 动态加载；
- immutable deployment A、B 可同时存在；
- promotion/rollback 只改变 active route，不覆盖已有 loader key；
- loaded Worker 可经 JSRPC 调用 binding-scoped fake adapter；
- loaded Worker 无法取得泛化 backend Fetcher、宿主路径或平台 secret；
- 动态导出的 DO class 可作为 native facet 执行 fetch 和 RPC；
- facet 的 KV/SQL storage 相互隔离；
- workerd 被强制终止后，SQLite 数据可恢复；
- deployment 换版时，facet 可重启到新 class，同时保留稳定 storage identity。

### 3.2 明确不做

- control/admin API、用户认证和 Wrangler 兼容；
- S3 bundle store、artifact cache 和正式 deploy pipeline；
- 产品级 KV、D1、R2 adapter；
- DO namespace migration、alarm、WebSocket、hibernation 和跨进程 owner lease；
- Queue、Cron、Workflow 或 scheduler；
- 多副本、跨机器迁移、共享 SQLite 和自动 failover；
- 压测、计费、完整 metrics 或 production dashboard。

G0 可以用本地 fixture 文件提供 WorkerCode，用临时内存 Map 或临时 SQLite 提供 fake
binding。这样能隔离 runtime 风险，不把 S3 和 control plane 的故障混入结果。

## 4. 待确认的架构合同

### 4.1 进程模型

G0 的目标形态是一个可部署服务单元：

```text
platformd
└── workerd child
    ├── ingress worker
    ├── loader host worker
    ├── fake binding host worker
    ├── DO supervisor Durable Object
    └── localDisk -> temporary G0 data directory
```

这里的“单体服务”不等于所有逻辑写在一个 isolate。一个 workerd 进程仍应包含多个静态
system Worker/service，用 workerd capability binding 隔离权限。`platformd` 是进程、文件和
恢复边界；tenant code 永远不直接接触它。

G0 不证明将来必须永远只有一个 workerd 进程，只证明 SMB 单机版不需要 Redis、网关或
额外 runtime 容器也能成立。

### 4.2 Dynamic Worker identity

loader key 必须是 immutable deployment identity，而不是逻辑 Worker 名：

```text
deploymentKey = <accountId>/<workerId>/<deploymentId>
```

合同如下：

- 相同 key 必须表示逐字节相同的 WorkerCode；
- 新 bundle 必须生成新 `deploymentId`；
- A、B 使用不同 key，允许同时 warm；
- active route 只保存当前 deployment ID；
- rollback 是把 route 从 B 指回 A；
- 禁止“原地更新 A 后清 cache”的设计。

workerd 的 loader 本身也是 cache：同名 Worker 已存在时会复用。因此 callback 是否被调用
不能作为 authorization 或 route 状态的唯一校验；authorization 必须在取得 loader stub 之前
完成，key 的 immutability 必须由 deploy/control 层保证。

### 4.3 Binding capability

loaded Worker 只得到面向某个资源实例的能力：

```json
{
  "accountId": "acct_fixture",
  "resourceId": "kv_fixture_a",
  "deploymentId": "deploy_a"
}
```

G0 用一个最小 `FixtureKV` facade 验证：

```ts
interface FixtureKV {
  get(key: string): Promise<string | null>;
  put(key: string, value: string): Promise<void>;
}
```

这些 props 由宿主从冻结的 deployment metadata 生成，tenant request/body/header 不能覆盖。
adapter 内部再次验证 resource scope，并只返回 tenant-safe error。禁止把以下能力放进动态
Worker 的 env：

- 任意 resource ID 的通用 admin client；
- 任意 URL 的 internal Fetcher；
- SQLite 文件路径或文件句柄；
- S3 endpoint/access key/secret key；
- platform master key 或 internal auth token。

### 4.4 Durable Object identity

DO 的执行版本和存储身份必须分离：

```text
执行代码：deploymentId + className
存储身份：doStorageId + className + objectId
facet identity：由稳定存储身份派生，不包含 deploymentId
```

同一逻辑 Worker 的普通 promotion 不改变 `doStorageId`。新版本若需要立即生效，应对已有
facet 执行 `abort()`，下一次 `get()` 用新的 class 重建；不得调用 `delete()`，因为后者会永久
删除 facet SQLite。只有整个逻辑 Worker 删除并重建时，才分配新的 `doStorageId`。

### 4.5 持久化边界

workerd config 的 `localDisk` 指向一个专用数据目录：

```text
<g0-data>/do/
```

G0 不依赖内部 SQLite 文件名，也不直接打开或修改 workerd 创建的数据库。所有读写只能从
DO Storage API 进入。备份、迁移和磁盘布局属于 P0/P1。

上游目前仍把 workerd `localDisk` 标为 experimental，并明确可能发生不兼容变化。这不会
直接否决单机方案，但意味着：

- workerd 升级必须是显式 migration event；
- G0 必须锁定二进制，不可滚动跟随；
- 后续 release gate 必须用旧数据目录跑升级/回滚 fixture；
- 如果无法定义可接受的升级路径，则整个 DO 方案 No-Go。

## 5. 建议的 G0 目录和产物

实现时建议集中放在独立目录，防止 spike 代码渗入产品模块：

```text
g0/
├── README.md
├── workerd.lock
├── workerd/
│   ├── config.capnp
│   ├── ingress.js
│   ├── loader-host.js
│   ├── binding-host.js
│   └── do-supervisor.js
├── fixtures/
│   ├── worker-a/
│   ├── worker-b/
│   ├── bad-syntax/
│   ├── binding-client/
│   └── do-counter/
├── harness/
│   ├── process-supervisor.*
│   ├── fixture-loader.*
│   └── assertions.*
└── tests/
    ├── loader.*
    ├── binding.*
    ├── durable-object.*
    └── crash-recovery.*
```

`workerd.lock` 至少记录：

```text
release/version
binary source URL
sha256
target os/arch
compatibility date
required process flags
upstream commit or release notes URL
```

最终另写 `docs/g0-results.md`，记录真实命令、平台、workerd 版本、每个 case 的结果和已接受
限制。这个结果文件应由实际执行产生，本设计文档不预填“通过”。

## 6. 工作包与依赖顺序

### G0.0：版本固定和最小启动

目标：得到可重复的 workerd runtime baseline。

实现：

1. 选择一个明确 upstream workerd release；
2. 固定二进制来源与 checksum；
3. 固定 compatibility date 和是否需要进程级 experimental flag；
4. 写最小 Cap'n Proto config，并支持保存一份净化后的 debug dump；
5. 参考 Miniflare，把 binary config 通过 stdin 传给 workerd；
6. 使用 control fd 等待必需 socket ready，同时保留 `/health` 语义检查；
7. 由 test harness 创建独立临时数据目录并启动 workerd；
8. 捕获 structured logs 和 startup stderr。

验证：

- `workerd --version` 与 lock 一致；
- checksum 不匹配时在启动前失败；
- config 编译/解析错误时 workerd 非零退出；
- 端口冲突和数据目录不可写时 fail closed；
- control fd 未报告全部必需 socket 时不得宣告 ready；
- workerd 在 ready 前退出时，harness 必须返回 startup error 而不是永久等待；
- 正常 SIGTERM 可退出，SIGKILL 可由 harness 发现；
- 测试结束只清理本次创建的临时目录。

产物：`workerd.lock`、最小 config、process harness。

Gate：无法从固定 artifact 重复启动，不进入 G0.1。

### G0.1：静态 host runtime

目标：先证明 workerd 的静态服务拓扑、内部 binding 和 process boundary，避免把 config 问题
误判为 loader 问题。

最小路径：

```text
HTTP client -> ingress -> static echo service -> Response
```

验证：

- 默认/named entrypoint 可被静态宿主调用；
- internal service 不监听 public socket；
- tenant-facing socket 无法路由到管理 handler；
- handler exception 只使请求失败，不终止 workerd；
- health 只能在 workerd ready 后通过；
- control-fd ready 与 HTTP health 均通过后，才允许执行后续 case；
- workerd child 退出后，platform harness 能启动一个新 PID。

Gate：静态拓扑和恢复不稳定，不进入 dynamic loading。

### G0.2：`workerLoader` 和 immutable deployment

目标：验证最小 Dynamic Worker fetch 路径及 loader cache 合同。

fixture：

- Worker A 返回 `{ deployment: "A", module: <imported value> }`；
- Worker B 返回 `{ deployment: "B", module: <different value> }`；
- bad-syntax 在 module parse 阶段失败；
- missing-module 引用不存在的 main module；
- throw-startup 在 top-level evaluation 阶段失败。

加载路径：

```text
route(deploymentId)
  -> LOADER.get(immutableDeploymentKey, getCode)
  -> WorkerStub.getEntrypoint()
  -> fetch(request)
```

`getCode` 在 G0 中从只读 fixture 目录组装 `WorkerCode`，至少包含明确的
`compatibilityDate`、`mainModule`、`modules` 和 `globalOutbound: null`。G0 不从 S3 读取。

验证：

1. A 首次请求 cold load 成功；
2. A 后续请求复用同名 loaded Worker；
3. B 使用新 key cold load，A 仍可访问；
4. active route A -> B 后新请求返回 B；
5. route B -> A 后无需重传或覆盖 bundle；
6. 并发请求同一 cold key 不产生不同代码实例；
7. bad-syntax/missing-module 失败不污染 A、B；
8. 同一个 key 被错误映射到不同内容时，harness 必须将其视为平台 invariant violation；
9. `globalOutbound: null` 时 fixture 的外部 fetch 确定失败；
10. workerd restart 后可从同一 fixture 再次 cold load。

必须采集 cold/warm callback 次数，但不把具体 cache 驱逐时机写成产品语义；上游可以在
Worker 不再使用时卸载它。

Gate：A/B 不能共存，或 rollback 依赖覆盖同一 loader key，则 No-Go。

### G0.3：内部 dispatch envelope

目标：固定 ingress 到 loaded Worker 的最小调用合同，为后续 fetch、scheduled、queue 和
workflow 共用 transport 奠基，但只实际执行 fetch。

G0 envelope：

```json
{
  "kind": "fetch",
  "accountId": "acct_fixture",
  "workerId": "worker_fixture",
  "deploymentId": "deploy_a",
  "entrypoint": null,
  "requestId": "opaque-test-id"
}
```

正文和 HTTP metadata 仍用 Request 传递；平台 identity 来自已验证的内部上下文，而不是
tenant 可伪造的 header。`scheduled`、`queue`、`workflow` 只预留可辨识的 kind，不实现 handler
或 scheduler。

验证：

- default 与 named entrypoint；
- 未知 entrypoint 返回稳定错误；
- 未知 dispatch kind fail closed；
- request body、response stream 和 abort signal 的基本传递；
- tenant 同名 header 不能覆盖 account/deployment identity；
- 错误日志包含 request ID 和 deployment ID，不包含源码、secret 或绝对数据路径。

Gate：身份只能靠 tenant-controlled header 传递，则必须重做 transport。

### G0.4：binding-scoped host adapter

目标：证明动态 Worker 能使用平台能力，但只能使用 deployment 已冻结的具体 resource。

fixture 拥有两个逻辑 namespace：

```text
kv_fixture_a -> { shared: "A" }
kv_fixture_b -> { shared: "B" }
```

同一 binding host entrypoint 使用不同 immutable props 实例化 adapter。loaded Worker 只看到
`env.KV.get/put` facade，不看到 host service 或 props 修改入口。

验证：

1. binding A 读取 `shared` 只能得到 `A`；
2. binding B 读取同名 key 只能得到 `B`；
3. Worker request 伪造 `resourceId=B` 仍只能访问 A；
4. Worker payload 尝试传绝对路径、其他 resource ID 或 internal URL 被当作普通值或拒绝；
5. adapter 不提供 list-resources/admin/open-file/generic-fetch 等方法；
6. tenant 不可枚举 hidden backend capability；
7. structured-clone 支持和不支持的类型均有确定行为；
8. host exception 转换为稳定 tenant-safe error，不泄漏 stack/path；
9. A 的 adapter failure 不影响 B 和无 binding Worker；
10. cold/warm Worker 的 scope 一致。

fake backend 只需 Map 或临时 SQLite；它证明的是 capability shape 和 isolation，不证明 KV
语义。

Gate：如果只能把泛化 backend client/credential 交给 tenant，整个 binding 架构 No-Go。

### G0.5：动态 DO class 和 native facet

目标：证明动态 Worker 导出的 DO class 可在静态 supervisor Durable Object 内作为原生 facet
运行。

最小调用：

```text
loaded bundle
  -> workerStub.getDurableObjectClass("Counter")
  -> supervisor ctx.facets.get(facetName, () => ({ class, id }))
  -> facet.fetch() / facet RPC
  -> facet-private SQLite
```

`Counter` fixture 至少实现：

- `fetch()`：原子增加一个 SQL counter 并返回值；
- `getValue()`：RPC 读取 counter；
- `failAfterWrite()`：在事务内写入后抛错，用来确认 rollback；
- `getIdentity()`：返回 fixture 可观察的 `ctx.id`，不返回宿主内部路径。

facet name 从稳定 identity 派生：

```text
facetName = encode(doStorageId, className, objectId)
```

编码必须 reversible 或至少 collision-resistant、长度受限、拒绝 malformed Unicode。不得只用
`objectId`，也不得包含当前 `deploymentId`。

验证：

1. 相同 storage/class/object 命中相同 facet；
2. 不同 object 的 counter 独立；
3. 同名 object、不同 class 独立；
4. 同名 object、不同 `doStorageId` 独立；
5. supervisor 自己的 SQLite 对 facet 不可见，facet SQLite 对 supervisor 不可见；
6. fetch 和 RPC 都可用；
7. 同一 object 的并发 increment 无 lost update；
8. 不同 object 可独立推进，不共享应用状态；
9. transaction 中抛错不提交；
10. 不存在的 class、非法 class name 和非法 object ID 安全失败。

G0 只验证原生语义，不自行模拟 input gate、output gate 或 `ctx.storage.sql`。

Gate：若需要 fork workerd、用普通 SQLite adapter 模拟 DO，或 facet 无独立 SQLite，则 No-Go。

### G0.6：SQLite crash/restart 持久化

目标：证明 DO 数据属于稳定 localDisk identity，而不是 workerd PID 或 loader cache。

测试序列：

```text
start workerd PID 1
  -> Counter(object-1) increment to 3
  -> confirm RPC returns 3
SIGKILL PID 1
start workerd PID 2 with same pinned binary/config/data dir
  -> cold-load same deployment
  -> reconstruct supervisor/facet
  -> confirm RPC returns 3
  -> increment returns 4
```

追加验证：

- object-2 仍保持自己的值；
- supervisor metadata 和 facet data 均能恢复；
- 使用全新 data dir 时从空状态开始；
- 只读/不可写 data dir 启动失败而非降级到 in-memory；
- 在一次持续写入循环中随机 SIGKILL，多轮重启后值只能是已确认值或其后值，数据库可继续用；
- 一个 fixture 的业务错误不损坏其他 facet。

G0 不通过读取 WAL 文件判断成功，只从 API 观察恢复结果。若 crash 后一次请求的提交状态无法
确定，记录为 `result-unknown`，不宣称 exactly-once。

Gate：正常 workerd restart 丢失已确认写入，或单个 facet crash 可重复破坏其他 facet，则
No-Go。

### G0.7：deployment 换版与 facet lifecycle

目标：证明代码版本变化不会意外切换或删除 DO storage。

fixture：

- Counter A：响应包含 `codeVersion=A`；
- Counter B：读取同一 schema，响应包含 `codeVersion=B`；
- 两者对 counter storage schema 兼容。

测试序列：

1. deployment A 创建 facet，counter 写到 3；
2. promotion 到 B，但不改变 `doStorageId`；
3. 对旧 facet 执行 `ctx.facets.abort(facetName, reason)`；
4. 下一次 `ctx.facets.get()` 使用 B 的 class；
5. 返回 `codeVersion=B, counter=3`；
6. rollback A，再次 abort/recreate；
7. 返回 `codeVersion=A, counter=3`；
8. 显式 `facets.delete()` 后重新 get，counter 才从空状态开始。

这里明确区分：

- `abort()`：结束执行实例，保留 SQLite，用于 code restart；
- `delete()`：结束实例并永久删除 SQLite，只用于资源删除；
- workerd restart：所有内存执行实例消失，SQLite 保留。

Gate：如果切换 deployment 必须改变 facet name/storage path，或无法在保留存储时加载新 class，
则 DO promotion 设计 No-Go。

### G0.8：supervisor recovery 和完整回归

目标：把前面的单项验证串成一个可重复、无人工操作的黑盒 suite。

harness 必须：

- 为每轮生成独立端口、PID file 和 temp data dir；
- 等待 ready，而不是固定 sleep；
- 能向指定 workerd PID 发送 SIGTERM/SIGKILL；
- 保留失败轮次的日志和数据目录；
- 保留净化后的 generated config，便于与 Miniflare config pattern 对照；
- 成功时只清理本轮自己创建的目录；
- 给每个 case 固定 seed，失败可重放；
- 串行执行共享端口/目录的 crash cases；
- 测试退出时回收 child process。

完整回归至少连续运行三轮，排除偶然 warm cache 或残留目录造成的假通过。

Gate：只靠手工 curl、Miniflare API 或 mock workerd 得到的结果不算 G0 通过；Hard Gate 必须
直接运行自有 config 和 pinned stock workerd。

## 7. 黑盒测试矩阵

| Case | 前置状态 | 操作 | 期望结果 | Gate |
| --- | --- | --- | --- | --- |
| L01 cold load A | 空 loader | 请求 A | A 成功，loader callback 发生 | Hard |
| L02 warm A | A 已加载 | 再请求 A | A 成功，不依赖重取源码 | Hard |
| L03 coexist A/B | A 已加载 | 请求 B，再请求 A | 两版本均可用 | Hard |
| L04 promote | route=A | route 指向 B | 新请求执行 B | Hard |
| L05 rollback | route=B | route 指回 A | 无 bundle 覆盖即可执行 A | Hard |
| L06 invalid bundle | A/B 正常 | 请求 bad-syntax | 该版本失败，A/B 不受影响 | Hard |
| L07 cold concurrency | key 未加载 | 并发请求同一 key | 代码身份一致，无串版 | Hard |
| L08 outbound denied | `globalOutbound=null` | fixture 发外网请求 | fail closed | Hard |
| B01 resource isolation | KV A/B 有同名 key | 分别读取 | 返回各自值 | Hard |
| B02 forged scope | Worker 绑定 A | payload/header 声称 B | 仍只访问 A | Hard |
| B03 safe error | host 抛内部异常 | Worker 调 adapter | 无 stack/path/secret | Hard |
| D01 facet fetch | 空 object | 连续 increment | 单调 1、2、3 | Hard |
| D02 facet RPC | counter=3 | `getValue()` | 返回 3 | Hard |
| D03 object isolation | object-1=3 | 读 object-2 | 空/0 | Hard |
| D04 storage isolation | supervisor 有私有表 | facet 探测 | 不可见 | Hard |
| D05 transaction | 已知 counter | write 后 throw | 值不变 | Hard |
| D06 process restart | counter=3 | SIGKILL + restart | 仍为 3 | Hard |
| D07 code promotion | A/counter=3 | abort，使用 B 重建 | B/counter=3 | Hard |
| D08 rollback | B/counter=3 | abort，使用 A 重建 | A/counter=3 | Hard |
| D09 explicit delete | counter=3 | facet delete + get | 空/0 | Hard |
| R01 repeated suite | 无残留状态 | 全套运行三轮 | 三轮确定通过 | Hard |

可选、不阻塞 G0：

- 一个普通 WebSocket upgrade 穿过 facet；
- cold/warm latency 基线；
- dynamic Worker code/env 上限附近的边界测试；
- workerd 新旧候选版本对同一数据目录的只读兼容演练。

这些项目即使成功，也不代表 P0 已支持 WebSocket hibernation、生产 limits 或升级回滚。

## 8. 故障注入点

至少在下列边界注入可重放故障：

```text
F1  getCode 开始前
F2  WorkerCode 组装后、loader 返回前
F3  loaded Worker top-level evaluation
F4  adapter 调用前
F5  adapter 已写入、响应前
F6  DO transaction 内、commit 前
F7  DO write 已确认、response 前
F8  workerd idle
F9  workerd 正在处理并发 DO 请求
F10 promotion 已切 route、facet abort 前
F11 facet abort 后、新 class get 前
```

结果分类必须区分：

- `not-applied`：明确未提交；
- `applied`：明确提交并可恢复；
- `result-unknown`：可能已提交但调用方未收到结果；
- `runtime-unavailable`：workerd/process 不可用；
- `platform-invariant-violation`：immutable key 被复用等宿主错误。

不要把所有故障都折叠成 HTTP 500，否则 P0 无法设计安全 retry。

## 9. G0 必需观测

G0 不建完整 observability stack，但每条请求必须有结构化日志，至少包括：

```text
timestamp
requestId
workerdPid
deploymentId
loaderKeyHash
loaderOutcome = cold | warm | error
dispatchKind
entrypoint
bindingType
resourceIdHash
doStorageIdHash
className
objectIdHash
durationMs
outcome
errorCode
```

禁止记录：

- Worker source/bundle 正文；
- vars/secrets；
- S3 credential；
- tenant request/response body；
- 完整本地数据路径；
- 未净化的内部 exception stack 返回给 tenant。

测试 harness 另外记录 child PID、启动次数、退出 signal/status、ready latency 和每个 fixture
callback 计数。

## 10. Go / No-Go 标准

### 10.1 Hard Go 条件

以下条件必须全部满足：

1. 使用 stock、固定版本 workerd，无源码 patch；
2. 一个 workerd 进程能承载所需静态 host services；
3. `workerLoader` 能按 immutable key 加载、缓存并隔离 A/B；
4. promotion/rollback 不依赖覆盖 bundle 或 cache invalidation；
5. loaded Worker 只能访问 binding-scoped capability；
6. 动态 DO class 能通过 native facet 执行 fetch、RPC 和 SQLite；
7. supervisor 与各 facet storage 相互隔离；
8. 已确认 DO 写入跨 SIGKILL/restart 保留；
9. `abort()` 换版保留 storage，`delete()` 才删除 storage；
10. suite 可无人值守重复运行三轮。

### 10.2 Hard No-Go 条件

出现任一项即停止 P0：

- 核心路径需要 fork/patch workerd；
- loader key 不能让 immutable A/B 同时存在；
- loaded Worker 必须持有通用 backend credential/Fetcher；
- tenant 可改变 props 或越权选择 resource；
- 动态 DO 只能用普通 adapter 模拟，无法获得 native facet storage/gate 语义；
- facet storage identity 必须包含 deployment ID；
- code promotion 只能通过删除 SQLite 生效；
- 正常 restart 丢失已确认 DO 写入；
- 单个 malformed bundle/facet 可稳定导致其他 tenant 数据损坏；
- `localDisk` 的版本/恢复风险无法通过 pin 和 release migration 控制。

### 10.3 Conditional Go

下面结果可记录为限制，不阻塞进入 P0：

- cold load latency 高，但可测且不影响正确性；
- loader cache eviction 时机不可控；
- process crash 后一次 in-flight write 返回 `result-unknown`；
- G0 未验证 alarm、WebSocket hibernation 或 DO migration；
- 单机形态不支持跨节点 DO relocation；
- workerd `localDisk` 需要版本绑定和 forward-only upgrade。

Conditional Go 项必须进入 P0/P1 risk register，不能从结果报告中省略。

## 11. 建议的提交切片

按依赖拆成五个可独立审查的变更：

1. `G0-1 bootstrap`：workerd lock、config、static health、process harness；
2. `G0-2 loader`：A/B fixture、cold/warm、promotion/rollback、invalid bundle；
3. `G0-3 binding`：scoped props、fake adapter、越权/错误净化测试；
4. `G0-4 durable-object`：supervisor、dynamic class、facet SQLite、版本 lifecycle；
5. `G0-5 recovery`：SIGKILL matrix、三轮 suite、`g0-results.md`。

后一切片只在前一切片 Gate 通过后开始。每个切片都应保留一条统一 runner contract，例如：

```text
g0 test bootstrap
g0 test loader
g0 test binding
g0 test durable-object
g0 test recovery
g0 test all
```

这是待实现的命令合同，不表示仓库当前已存在这些命令。

## 12. G0 通过后冻结的接口

进入 P0 前，把验证成功的最小合同整理为 ADR 并冻结：

- workerd version/compatibility/flags policy；
- immutable deployment key grammar；
- WorkerCode bundle envelope；
- fetch/internal dispatch envelope；
- binding props 和 tenant-safe error shape；
- `doStorageId`、class、object 和 facet name 的 identity 规则；
- `abort`、`delete`、process restart、whole-worker recreate 的区别；
- workerd child readiness、shutdown 和 recovery contract；
- `result-unknown` 的错误分类。

P0 可以替换 fixture loader 为 S3 artifact store、fake binding 为正式 KV/D1/R2 adapter、测试 route
为 control.sqlite active deployment，但不得改变上述 identity 和 isolation 合同而不重新跑 G0。

## 13. 参考依据

- [workerd `WorkerLoader` / `WorkerStub` source](https://github.com/cloudflare/workerd/blob/main/src/workerd/api/worker-loader.h)：`get()`、`getEntrypoint()`、`getDurableObjectClass()` 和 WorkerCode shape。
- [workerd config schema](https://github.com/cloudflare/workerd/blob/main/src/workerd/server/workerd.capnp)：loader cache identity、DO `localDisk` 与 SQLite 文件布局说明。
- [Cloudflare Dynamic Workers — Durable Object Facets](https://developers.cloudflare.com/dynamic-workers/usage/durable-object-facets/)：`ctx.facets.get/abort/delete`、facet storage isolation 和动态 class 官方模型。
- [Cloudflare SQLite-backed DO Storage](https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/)：`ctx.storage`、SQL/KV 和 storage gate 语义。
- [WDL runtime design, pinned tag `wdl.20260817.1`](https://github.com/wdl-dev/wdl/blob/wdl.20260817.1/docs/modules/runtime.md)：immutable loader identity、binding-scoped host adapter 和 trust boundary 参考。
- [WDL Durable Objects design, pinned tag `wdl.20260817.1`](https://github.com/wdl-dev/wdl/blob/wdl.20260817.1/docs/modules/durable-objects.md)：dynamic class、native facet、stable storage ID 和 lifecycle 参考。
- [Miniflare package](https://github.com/cloudflare/workers-sdk/tree/main/packages/miniflare)：基于 workerd 的本地 simulator 和 tool-author API。
- [Miniflare runtime supervisor](https://github.com/cloudflare/workers-sdk/blob/main/packages/miniflare/src/runtime/index.ts)：binary config stdin、control fd readiness、日志和 workerd child lifecycle 参考。
- [Miniflare worker-loader plugin](https://github.com/cloudflare/workers-sdk/blob/main/packages/miniflare/src/plugins/worker-loader/index.ts)：最薄的 `workerLoader` binding config 参考。
- [Miniflare D1 plugin](https://github.com/cloudflare/workers-sdk/blob/main/packages/miniflare/src/plugins/d1/index.ts)：共享 entry service、binding props、embedded Worker 和 localDisk service 参考。
- [Miniflare DO plugin](https://github.com/cloudflare/workers-sdk/blob/main/packages/miniflare/src/plugins/do/index.ts)：workerd DO disk service 和 persistence path 参考。
- [Miniflare Queue plugin](https://github.com/cloudflare/workers-sdk/blob/main/packages/miniflare/src/plugins/queues/index.ts)：仅参考 producer/consumer dispatch；当前 broker 使用 in-memory storage，不能作为 durability 设计依据。

这些资料用于确认可验证的上游机制，不把 WDL 的 Redis、多副本 owner lease、独立 do-runtime
容器或 alarm/Workflow 架构照搬进本单机 G0，也不把 Miniflare 的 dev-only Node bridge、registry、
热重载或内存 Queue 当作生产平台能力。
