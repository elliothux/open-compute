# P16：Cloudflare Containers 兼容设计

状态：Day 1 合同与架构设计完成；待合同冻结、workerd 动态 Container G0、外部 Container Engine Broker、实施与验收。

本文细化 [P6 Cloudflare v4 API 与 Wrangler 子集兼容设计](implemented/p6-cloudflare-v4-wrangler-compatibility.md)
中的 Container upload、Containers control API 和 Durable Object Container runtime。P16 以固定 Cloudflare 公共合同、
`@cloudflare/containers`、Wrangler/Miniflare source snapshot 与已授权的 `third_party/workerd/` fork 为依据，不把
Miniflare 的开发期 Docker socket 当作生产安全边界，也不宣称复制 Cloudflare 全球 Container fleet。

## 1. 范围与结论

P16 Day 1 目标：

- 标准 `wrangler.jsonc` 的 `containers[]`、对应 Durable Object binding/export/migration；
- Wrangler multipart Worker metadata、Container application、image 与 rollout 调用序列；
- 固定 Workers types 中公开的 Durable Object `ctx.container`；
- 固定 `@cloudflare/containers` 的 Container class、routing、readiness、alarm、HTTP/WebSocket 与 lifecycle 行为；
- `start`、`exec`、`destroy`、`signal`、`monitor`、`getTcpPort` 和声明支持的 outbound interception；
- operator-owned 外部 Container Engine Broker 的本机执行、资源限制、网络隔离与 recovery；
- 单机部署适用的 image admission、capacity、instance inventory 和 rollout；
- 固定 Wrangler `containers` commands 中通过 C0 inventory 选定的标准子集。

结论：**Worker 侧公开 API 可以以固定版本为边界做到兼容；Cloudflare 全球基础设施语义不能、也不应伪装为完全等价。**
实现必须保留 workerd 原生 `ctx.container`，让固定官方 package 直接运行：

```text
tenant Worker / @cloudflare/containers
  -> native ctx.container
  -> workerd Container implementation
  -> operator-owned Container Engine Broker over Unix socket
  -> Docker/containerd-compatible local runtime
  -> one application container + one egress interceptor sidecar per running identity
```

P16 与 [P15 Browser Run](p15-browser-run.md) 只共享外部依赖的部署边界：

1. `ocd` 仍是唯一公开 listener 和 account/deployment/image/capacity authority；
2. Broker 由 operator 预先部署，open-compute 不下载、不搜索 `PATH`、不启动、不 supervise；
3. 正式 release 仍是单个 `ocd` executable 与既有单个 workerd child；
4. engine socket、runtime credential、provider container ID 与 host topology 永不暴露给 tenant；
5. Broker 未配置、合同不匹配或资源不足时 upload/readiness/start fail closed；
6. tenant 不能指定 engine endpoint、host mount/device、privileged mode、runtime socket 或 provider credential。

Containers 与 Browser Run 的关键差异是：Browser binding 可以落为 HTTP/CDP provider protocol；Container 是绑定在 DO
context 上的 native capability，包含 Fetcher、Socket、streaming exec、monitor 和 lifecycle identity。P16 不用普通 HTTP
service binding 重写这套 API。

## 2. Compatibility authority

实施和 qualification 固定：

- [Cloudflare Containers](https://developers.cloudflare.com/containers/)；
- [Durable Object Container API](https://developers.cloudflare.com/durable-objects/api/container/)；
- [Container lifecycle](https://developers.cloudflare.com/containers/concepts/architecture/)；
- [Container local development](https://developers.cloudflare.com/containers/guides/local-dev/)；
- [Container rollouts](https://developers.cloudflare.com/containers/configuration/rollouts/)；
- [Container limits and instance types](https://developers.cloudflare.com/containers/platform/limits/)；
- [Wrangler Containers commands](https://developers.cloudflare.com/workers/wrangler/commands/containers/)；
- [Cloudflare `@cloudflare/containers`](https://github.com/cloudflare/containers)；
- [workerd Container configuration/source](https://github.com/cloudflare/workerd/blob/main/src/workerd/server/workerd.capnp)；
- repository snapshot `wrangler@4.127.1`、`miniflare@5.20260828.0-alpha` 与 commit
  `f8085545bcaa2c639f171c25e4424685036a0e10`；
- 当前研究候选 `@cloudflare/containers@0.3.7`；C0 必须把最终 package tarball、integrity、types 和 source revision 固定；
- 当前 `third_party/workerd/` source revision `b3e1a27840299f493d9425dc4d9972381d02ef23`；正式实现仍须按
  [workerd runtime policy](workerd/README.md) 完成协调 pin；
- 固定 Cloudflare remote traces、OpenAPI revision/hash、Workers types 与 Wrangler subprocess fixtures。

网页和 upstream source 用于发现合同；进入 Gate 的 property descriptor、method signature、同步 throw、promise rejection、
stream/Socket 行为、HTTP/WebSocket、signal/exit、restart 和 CLI/API wire shape 必须固定为 inventory/fixture。Cloudflare
Containers 仍在快速演进，未进入 inventory 的 beta、experimental 或新字段默认 unsupported。

不得因为当前 fork 暴露了比官方文档更多的 method/options 就自动扩大兼容范围。workerd-only experimental capability 必须由
兼容日期/flag 隐藏或明确拒绝，除非 C0 把它登记为正式支持的 Cloudflare 合同。

## 3. 兼容声明边界

P16 把“兼容”拆成三个可验收层次：

| 层 | 目标 | 声明 |
| --- | --- | --- |
| Worker API | 固定 `ctx.container`、Workers types、官方 package | 目标为逐 API 行为兼容 |
| Wrangler/control plane | 固定 config、upload、application/image/rollout/commands | 只声明 inventory 中通过的标准子集 |
| Cloudflare fleet | 全球 placement、预热、跨机房路由、Cloudflare VM/配额/计费 | 明确不实现，不宣称等价 |

正式 capability 文案：

> open-compute supports the documented Cloudflare Worker Containers API for the pinned compatibility date, workerd
> revision, Workers types, Wrangler revision, and `@cloudflare/containers` package. Placement and execution use a
> single-machine operator-owned provider and do not emulate Cloudflare's global scheduling infrastructure.

允许的单机偏差：

- placement 固定为当前 open-compute host，不接受或伪装 `Region:Earth`；
- DO 与其 Container 在同一台机器执行，但仍通过 native capability 通信，不泄露 localhost endpoint；
- 不承诺 Cloudflare image prefetch location、1–3 秒 cold-start、fleet autoscaling 或跨机迁移；
- instance type 映射到 operator 配置的本机资源上限，不表示 Cloudflare plan entitlement；
- rollout 保持公开状态机和可观察顺序，但只调度本机实例；
- runtime isolation 由声明的 Docker/containerd/provider profile 实现，不把普通容器称为 Cloudflare per-instance VM。

这些偏差必须进入 `references/cloudflare-compatibility.md`、capability manifest、用户文档和 differential report；不能只埋在
P16 中。

## 4. 五层合同，不混为一个 API

| 层 | 调用方 | 合同 | authority |
| --- | --- | --- | --- |
| Wrangler config/upload | Wrangler | JSONC、multipart Worker metadata | 固定 Wrangler/source fixture |
| Containers public API | Wrangler、SDK、operator | `/client/v4/accounts/{account_id}/containers/**` | `ocd` |
| Worker high-level API | tenant package | `@cloudflare/containers` classes/helpers | 固定 package |
| Worker low-level API | tenant DO | native `ctx.container`、Fetcher/Socket/streams | workerd |
| Engine provider | workerd/`ocd` | operator-private Broker contract | external prerequisite |

Worker upload 成功不代表 image materialization 或 rollout 已完成。Cloudflare 当前顺序是先激活 Worker，再 build/push image，
最后启动 rollout；后两步不是事务，Wrangler success 只表示 rollout 已启动。P16 必须兼容固定 Wrangler 可观察到的顺序和错误，
但不能因此让一个未 materialize 的镜像在 runtime 被静默 pull。

## 5. Wrangler config 与 upload contract

### 5.1 标准配置

```jsonc
{
  "$schema": "./node_modules/wrangler/config-schema.json",
  "name": "container-app",
  "main": "src/index.ts",
  "compatibility_date": "2026-09-07",
  "containers": [
    {
      "name": "api-container",
      "class_name": "ApiContainer",
      "image": "./Dockerfile",
      "max_instances": 4,
      "instance_type": "lite",
      "rollout_step_percentage": [10, 100]
    }
  ],
  "durable_objects": {
    "bindings": [
      {
        "name": "API_CONTAINER",
        "class_name": "ApiContainer"
      }
    ]
  },
  "migrations": [
    {
      "tag": "v1",
      "new_sqlite_classes": ["ApiContainer"]
    }
  ]
}
```

字段、默认值、互斥关系、named environment inheritance 和错误文本以固定 Wrangler schema/tests 为准。特别要求：

- `class_name` 必须是同一 script 的 SQLite-backed DO export 和 binding，不能指向另一个 script；
- application name、class name、binding name 与 account/script 唯一性逐项按 fixture 校验；
- `instance_type` 的 named/custom 两种形状不能混用，resource 值在 admission 时归一化一次；
- `max_instances`、rollout steps/grace、scheduling/affinity 中未实现的字段明确拒绝，不能静默忽略；
- `image` 是开发/部署输入，不作为 tenant runtime 可修改字段；
- tenant config 不接受 engine socket、Docker host、privileges、devices、mounts、network mode 或 provider auth；
- 不保留 Wrangler 旧 `containers.configuration` 兼容路径；P16 只实现固定 Day 1 schema。

### 5.2 Worker multipart metadata

C0 从固定 Wrangler upload tests 冻结 metadata。已确认的核心关系是：

- metadata `containers` 关联 application name 与 DO `class_name`；
- Worker export 标记相应 DO class 的 Container link；
- immutable Version 保存 Container descriptor digest，不保存本地 Dockerfile path、engine endpoint 或 credential；
- deployment active pointer 与 application target/rollout 分开，保留 Cloudflare 非事务顺序。

当前 service 在 [DO lifecycle validator](../crates/service/src/workers_http/v4/do_lifecycle.rs) 明确拒绝 `container` export。
P16 实施时直接修改当前 Day 1 validator/schema/fixtures，不保留“旧版本拒绝、新版本接受”的历史分支。

### 5.3 Build 与 image push

镜像 build 属于 developer/CI tooling：

- 本地 Dockerfile 由固定 Wrangler 与 operator-installed Docker-compatible CLI build；
- production `ocd` startup、Worker request 和 Container `start()` 都不得运行 Dockerfile build 或访问公网 registry；
- 标准 push/import route 接收 OCI manifest/config/layers，按 digest 校验后写入平台 artifact authority；
- image tag 只在 deployment admission 时解析一次为 immutable digest；runtime 不重新解析 `latest`；
- private registry credential 只由 developer CLI/provider secret reference 使用，不进入 Worker metadata、SQLite plaintext、logs 或
  Broker labels；
- remote registry ref 若未通过 C0 和安全 import flow，部署明确 unsupported，不能由 Broker 在首次请求时偷偷 pull；
- image 缺失、digest mismatch、architecture 不支持或 provider materialization 失败时 rollout/start fail closed。

## 6. Worker `ctx.container` contract

### 6.1 Public inventory

C0 从固定 docs、Workers types、workerd JSG declarations 和 package call graph 生成唯一 inventory。Day 1 至少覆盖：

```text
ctx.container.running
ctx.container.start(options?)
ctx.container.exec(command, options?) -> ExecProcess
ctx.container.destroy(reason?)
ctx.container.signal(signo)
ctx.container.monitor()
ctx.container.getTcpPort(port) -> Fetcher with fetch()/connect()
ctx.container.interceptOutboundHttp(...)
ctx.container.interceptOutboundHttps(...)
ctx.container.interceptOutboundTcp(...)  # only when public for the pinned contract
```

`ExecProcess` inventory 包含固定版本公开的：

```text
pid, stdin, stdout, stderr, exitCode
output(), kill(signal?), resize(...)
```

必须逐项固定：

- property 在 prototype/instance 上的位置、enumerability 和 `undefined`/`null`；
- `start()` 的同步返回与后台启动语义；
- already-running/not-running、空 command、非法 signal/options 的 exception class/message；
- stdin EOF、stdout/stderr pipe/ignore/combined、PTY、AbortSignal 和 backpressure；
- `output()` 的单次消费规则与 stream 已消费后的 `TypeError`；
- `monitor()` resolve/reject、destroy reason、restart generation 与旧 monitor fencing；
- Fetcher HTTP body/WebSocket upgrade、Socket half-close、container exit/disconnect race；
- compatibility date/flag 对 HTTPS/TCP interception、PID namespace 和其他新能力的影响。

workerd 当前还含 images、inspect、labels、snapshots、restore 与其他 experimental surface。它们只有同时出现在固定官方
docs/types/package contract 并完成 provider、安全、restart 与 differential coverage 后才可加入 supported inventory。

### 6.2 官方 package

固定 `@cloudflare/containers` 必须不经 fork、不经 import rewrite 直接运行，至少覆盖：

- `Container` subclass construction 与 `ctx.container` presence check；
- `defaultPort`、`sleepAfter`、`envVars`、`entrypoint`、`enableInternet`；
- `start`/`stop`/`destroy`、readiness retry 和 activity renewal；
- `onStart`、`onStop`、`onError`、alarm/schedule；
- `fetch()` HTTP/WebSocket proxy；
- `getContainer()` 的 DO identity/routing；
- 声明支持的 outbound handlers/interception；
- DO eviction、workerd restart、Broker/runtime restart 后的状态恢复。

只让一个 demo request 成功不构成 package compatibility。package 的 storage/alarm state 与 Container process state 必须在
restart 和 rollout 中保持一致的可观察行为。

## 7. DoHost 与 workerd 原生集成

### 7.1 当前结构阻点

open-compute 的 tenant DO 通过 [DoHost](../packages/runtime/src/durable-objects/host.ts) 和动态
`WorkerLoader::WorkerCode` 装载。当前 [WorkerLoader descriptor](../third_party/workerd/src/workerd/api/worker-loader.h) 没有
Container namespace/image/policy；workerd 则把 Container `imageName` 配在静态 DO namespace 上。这导致：

- 所有 tenant class 共用一个静态 `DoHost`，不能用一个 `imageName` 表示不同 account/script/version/class；
- 仅给 `DoHost` 静态开启 Container 会把 capability 扩散给未声明 Container 的 tenant；
- tenant deployment image 更新不能自然参与现有 route generation/version fencing；
- TypeScript facade 无法安全复制 native Fetcher/Socket/stream/monitor brand 与 lifetime。

### 7.2 Day 1 workerd extension

在已授权 fork 中增加一个聚焦的动态 Container attachment，不建立第二套 runtime：

```text
trusted DoHost loads immutable Worker snapshot
  -> WorkerLoader receives optional validated ContainerClassPolicy
  -> loaded DO class/facet gets a native rpc::Container capability
  -> ctx.container exists only for that declared class
  -> native workerd Container API talks to configured Broker
```

`ContainerClassPolicy` 是系统 Worker 与 workerd 之间的私有 structured value，至少包含：

```text
application_resource_id
deployment/version/class descriptor digest
immutable image digest/provider image reference
normalized cpu/memory/disk/pid limits
internet/egress policy
instance generation and rollout target generation
```

规则：

- policy 只由 `ocd` 持久 authority 生成的 authenticated/immutable runtime snapshot 派生；
- tenant module、env、request、DO RPC props 不能构造或覆盖 policy；
- 未声明 Container 的 class 得到与 Cloudflare 一致的 absent/unsupported surface，不共享 `DoHost` 的 capability；
- native `start()` 使用 policy 中的 immutable image；tenant 不能通过 experimental `image` option 绕过；
- workerd 给 Broker 的 create request 带 platform-owned opaque labels 与资源字段，不带 account name、secret 或 source path；
- route generation、object generation、deployment rollback 和 rollout target 共同 fence 旧 capability；
- superseded helper、静态占位 `imageName` 和 JS membrane 在正式实现中删除。

G0 可以用一个最小 JS membrane 验证 official package call graph，但不得把它作为最终 security/compatibility boundary。若 native
brand、facet context 或 lifecycle 无法保持，G0 直接转为上述 fork extension，不堆叠 wrapper workaround。

### 7.3 Container identity

一个 running Container 对应一个明确的 tenant DO object generation：

```text
account / script / class / object-id / object-generation
  + application-target-generation
  -> opaque engine instance key
```

engine name 使用 keyed digest/opaque ID，不把 account、script、class、DO ID 或 deployment name 暴露给 Docker CLI、provider logs
或其他 tenant。相同 DO object generation 同时最多一个 application container；旧 generation、旧 image rollout 或不匹配 start
identity 必须拒绝，不能连接到“名字碰巧相同”的进程。

## 8. External Container Engine Broker

### 8.1 为什么需要 Broker

workerd 当前 `containerEngine.localDocker` 直接连接 Docker socket，并标明只用于 local development/testing。生产不能把真实
Docker socket直接交给 workerd Container path，因为：

- Docker API 权限接近 host root；
- workerd config 的 capabilities/devices/security options 会原样传给 Docker，当前无 allowlist；
- local implementation 不是 open-compute 的 account/application capacity authority；
- tenant-visible option 和 engine extension 演进可能扩大可创建对象；
- raw Docker errors、IDs、paths 和 daemon topology 不得进入 Worker。

Broker 是一个受限 Docker Engine protocol endpoint，不是新的 public Container API。优先实现 workerd 实际调用的最小 HTTP
subset，使 upstream Container lifecycle/exec/network/recovery code保持唯一实现；只有该 subset 无法安全表达 provider contract
时，才在 workerd 增加更窄的 native RPC provider，不能同时长期保留两套 provider path。

### 8.2 部署合同

- Broker 只监听绝对路径 Unix domain socket；不支持 tenant 配置 TCP endpoint；
- socket parent、owner、mode、symlink/path containment 与 peer credential 在 `ocd`/workerd startup 前验证；
- operator 在启动 `ocd` 前提供 Broker 与 underlying runtime；
- `ocd` 不启动、停止或 supervise Broker，但 readiness 检查 capability/version；
- Broker contract version、runtime kind、isolation profile、supported architecture/features 形成 secret-free digest；
- contract 变化后旧实例不透明认领为新合同；按 reconciliation 关闭或标 lost；
- Broker 不可用时 workerd/container readiness degraded，非 Container Workers 可继续按现有平台 admission 运行；
- 正式 binary 不打包 Docker、containerd、runc、CNI、BuildKit、镜像或 `proxy-everything`。

### 8.3 最小 engine operation inventory

C-G0 从当前 workerd `ContainerClient` trace 冻结最小集合：

```text
container create/start/stop/kill/wait/inspect/delete/archive
exec create/start/resize/inspect
image inspect/delete/commit where required by supported snapshots
bridge/network inspection
volume create/inspect/delete where required by supported snapshots
application/egress-sidecar status and reconnect
```

任何未登记 Docker API route、query、HostConfig、mount、device、capability、namespace 或 registry credential 都由 Broker 拒绝。
不做通用 Docker remote proxy，也不提供 operator shell passthrough。

### 8.4 双重 enforcement

workerd 在 native authority boundary 校验 Worker-visible参数；Broker 在 host security boundary 再校验 effective spec：

- image 必须是已准入 immutable digest；
- CPU、memory、disk、PID、open files、process count 和 instance count 不超过 application/operator limit；
- rootless/user namespace/seccomp/AppArmor 等由 provider profile声明并可验证；
- 禁止 privileged、host PID/network/IPC、host device、Docker socket 和任意 host bind mount；
- 只允许 platform-owned ephemeral volumes/mounts；
- container name、labels、network 和 cleanup token 由平台生成；
- log/output size、exec count、exec stream 和 concurrent port connections bounded；
- unsupported architecture、FUSE/device/capability 明确拒绝，不模仿 Miniflare 自动提权。

信任 workerd 是唯一合法 socket caller不等于信任每个 create payload。Broker 应核对 Unix peer identity、固定 workerd-generated
labels/spec schema 和 operator policy；在单机边界足够时不新增分布式签名/token 服务。

## 9. Image authority 与 supply chain

建议当前 Day 1 schema 直接包含：

```text
container_images
  resource_id, account_id, manifest_digest, config_digest,
  platform, architecture, compressed_bytes, unpacked_bytes,
  artifact_key, admission_state, created_at

container_applications
  resource_id, account_id, script_id, class_name, name,
  max_instances, cpu_limit, memory_limit, disk_limit, pid_limit,
  egress_policy, target_generation, effective_generation, state

container_application_targets
  application_id, generation, worker_version_id, image_digest,
  provider_contract_sha256, rollout_kind, rollout_steps,
  active_grace_period, created_at
```

这是 Day 1 current schema，不增加旧 schema backfill/dual read。SQL sequence、checksums、fixtures 和 dispatch 同步修改。

image invariant：

- OCI manifest/config/layer digest 全链校验；
- platform/architecture、size、entrypoint、declared ports 与不安全 metadata 在 admission 时验证；
- artifacts immutable/content-addressed，tag 只是 control-plane pointer；
- Broker materialization 从平台已验证 artifact/provider import contract取得内容；
- engine cache 是 disposable acceleration，不是 image authority；
- cache hit 仍验证 digest；cache corruption 删除该 cache entry 并重新 materialize，不修改 authority；
- runtime 未准备好目标 digest 时 `start()` 返回稳定 unavailable，而不是运行旧 tag 或下载公网镜像；
- rollout/rollback只切 target/effective pointer，不修改已存在 image bytes。

## 10. Instance lifecycle、recovery 与 ownership

职责保持单一：

| concern | owner |
| --- | --- |
| application/image/target/rollout/capacity authority | `ocd` + SQLite/artifact store |
| Worker/DO code、native API、per-DO operation ordering | workerd |
| process/container creation与host isolation | Broker/runtime |
| application process与其临时 filesystem | application container |
| outbound mediation | workerd + egress interceptor sidecar |

SQLite 建议记录可恢复 lease，而不是把 Docker memory/list 当 authority：

```text
container_instance_leases
  application_id, object_id_digest, object_generation,
  target_generation, opaque_engine_key, provider_contract_sha256,
  state, lease_generation, created_at, connected_at,
  stopping_at, stopped_at, lost_at, last_observed_at

container_rollouts
  application_id, from_generation, to_generation,
  kind, steps, current_step, state,
  created_at, started_at, completed_at, failed_at
```

状态机：

```text
admitting -> starting -> running -> stopping -> stopped
admitting/starting/running/stopping -> lost
```

规则：

- SQLite transaction 内只做 authority state change；image/Broker/process I/O 在 transaction 外；
- create 使用 lease generation 和 opaque engine key 幂等；未知已存在实例不按名字认领；
- workerd reconnect 时同时验证 start identity、image digest、provider contract 和 process/container状态；
- application main process exit 驱动 `monitor()`，不能因 sidecar/daemon状态伪造正常 exit；
- `destroy()`、重复 stop、DO eviction、client disconnect 与 `ocd` shutdown 行为逐项固定；
- `ocd` restart 后从 SQLite target/leases 与 Broker inventory reconciliation；匹配的实例重连，不匹配的已知实例停止；
- Broker/runtime restart 后无法证明身份或状态的实例标 `lost`，不把新进程冒充旧 Container；
- provider orphan 由 keyed label + authority record识别并 bounded reap；未知 operator container 不触碰；
- recovery 失败不删除 retained failure evidence，不自修复数据库或 image authority。

## 11. Rollout 与 rollback

Wrangler/Cloudflare 的 observable order 保留：

```text
activate Worker version
  -> admit/materialize image and target
  -> create rollout if effective Container config changed
```

Worker code可能在 rollout 完成前访问旧 image instance。P16 因此必须持久化：

- Worker deployment active version；
- Container target generation；
- 每个 running instance 的 effective generation/image；
- rollout step 与 terminal state。

单机 rollout：

- `max_instances < 2` 时一个 100% step；其他固定默认/自定义 steps 按 Wrangler合同解释；
- 每一步只选择本机 eligible instances，不能为了“达到百分比”超过 `max_instances`；
- selected instance 先等待 active grace，再向 main process发送 `SIGTERM`；
- 按官方固定合同最多等待 15 分钟，之后 `SIGKILL`；
- process exit、`onStop` 可观察后再启动 target image；
- image/app code mixed window 是正式语义，不通过全局 stop-the-world 隐藏；
- rollback 创建指向旧 immutable target 的新 target generation，不复活旧 lease/capability；
- rollout失败保留 Worker已激活这一事实，返回固定 CLI/API 状态，不伪造事务回滚。

`scheduling_policy`、affinity、regional placement 等无法在单机产生对应效果的配置默认拒绝。若固定 Wrangler 要求 round-trip，
可以保存并返回 `unsupported` 状态，但不能接受后忽略并宣称生效。

## 12. Networking 与 egress

Container 没有 public listener；外部请求仍先到 `ocd`/Worker/DO，再通过 `getTcpPort()` 访问 Container。不得把 engine published
port、bridge IP 或 localhost port 返回给 tenant/client。

workerd local implementation 为每个 application container 配一个 `proxy-everything` egress interceptor sidecar，并让应用容器
共享其 network namespace。P16 保留一个 authoritative egress path：

```text
container connect/fetch
  -> egress interceptor sidecar
  -> workerd container egress handling
  -> platform-owned Network(allow = ["public"])
```

约束：

- `enableInternet=false` 默认无 general outbound；显式 interception binding 仍按固定 API合同处理；
- `enableInternet=true` 也只能使用 open-compute 已有 public-address-only 网络能力；
- private、loopback、link-local、metadata、Unix、IPv4-mapped private IPv6、DNS-to-private、redirect-to-private 和所有平台 listener
  在 address layer 拒绝；
- 不新增 hostname pre-resolution gateway 或第二条 container NAT fallback；
- outbound HTTP/HTTPS/TCP interception 的 hostname/glob/IP/CIDR匹配、replacement 和 open-connection update 按固定 workerd
  behavior；
- CA injection、HTTPS interception、credential/material 不能写进 application image layer、env、logs 或 exec output；
- sidecar image是 operator/provider正式 dependency，必须 pin digest、离线可用并通过 supply-chain验证；缺失时 fail closed；
- app/sidecar任一 crash、network namespace teardown 和 restart race有独立 regression coverage。

## 13. Limits、admission 与 backpressure

不复制 Cloudflare account plan 数值，也不设置 LynxOS “约 20 人”默认值。operator 必须配置或选择一个明确 provider profile：

- total/per-account/per-application running instances；
- pending starts、concurrent stops/reconcile/materializations；
- CPU、memory、disk、PID/process/open-files；
- image compressed/unpacked/layer/manifest limits；
- start/readiness/idle/max-lifetime/stop deadlines；
- concurrent execs、exec stdin/stdout/stderr bytes、buffer/queue；
- ports、connections、HTTP body、WebSocket frame/message/queue；
- logs/metrics retention 与 Broker inventory bounds。

admission permit 从 `admitting` 持有到 `stopped/lost`，不是 Worker request 生命周期内的短 semaphore。`max_instances` 在生产
必须由 SQLite/`ocd` admission 与 Broker effective count 双层执行；不能复制 Miniflare 本地开发中“不应用
`max_instances`”的行为。

instance type 映射表是 operator deployment capability：

- 固定 Cloudflare name可以作为 compatibility input；
- effective CPU/memory/disk必须能由 Broker实际 enforce；
- provider不支持某资源或上限时 deployment fail closed；
- API/status返回配置值与effective值，不把本机 profile谎报成 Cloudflare entitlement；
- current local Docker client 未证明完整执行 custom instance resources，因此 Broker Gate 必须观测 cgroup/runtime effective state，
  不能只检查 request JSON。

## 14. Containers public API 与 Wrangler commands

C0 从固定 Wrangler source/OpenAPI 抽取唯一 route inventory。当前 source表明至少涉及：

```text
/client/v4/accounts/{account_id}/containers/applications/**
application rollouts
instances
images and registries
selected command/SSH surfaces
```

不能根据 route 名称猜 schema。每个选定 command/route 单独固定：method、query、request/response、v4 envelope、pagination、
error、polling、terminal status 与 CLI exit code。

Day 1 优先级：

1. `wrangler deploy` 实际调用的 application create/update、image、rollout；
2. list/status/delete 与安全 operator diagnosis；
3. image list/delete/prune 中可由 immutable authority 安全表达的部分；
4. instance list/status；
5. exec/SSH 只有独立 auth、audit、stream/terminal resize、timeout 和 Broker capability通过后开放。

SSH/interactive exec 不是实现 Worker `ctx.container.exec()` 的前置条件。未支持命令返回明确标准失败，不提供把 Docker socket、
host shell 或 provider CLI 暴露给用户的替代入口。

## 15. Error、observability 与 secret handling

内部稳定 error classes：

```text
invalid_config, unsupported, image_not_found, image_rejected,
image_materialization_failed, capacity_exhausted, start_timeout,
provider_unavailable, provider_contract_mismatch, container_lost,
container_not_running, exec_rejected, exec_failed, port_unavailable,
egress_denied, rollout_failed, malformed_provider_response
```

Worker exception type/message、public API code/message/status 与 Wrangler exit code由固定 fixture分别映射，不能直接暴露 Broker/
Docker error。retryability、`Retry-After`、monitor rejection 和 signal/exit semantics 有明确表，不能用 provider message regex 猜。

日志/metrics 推荐低基数维度：

```text
account_id, script_id, application_id, operation,
target_generation, provider_contract_sha256,
instance_profile, result_class,
queue_wait_ms, start_ms, duration_ms, bytes_in, bytes_out
```

禁止记录：

- env/secrets、exec stdin/stdout/stderr/body；
- image registry credential、signed URL、Broker auth/socket、raw Docker response；
- raw DO ID、container ID、engine name、mount path、host PID/IP/topology；
- HTTP authorization/cookie、intercepted TLS material、Container traffic body。

P7 tail只显示 Worker触发的 Container operation metadata/outcome；operator diagnosis可以用 opaque ID关联受保护的 Broker审计，
不能把 host细节送进 tenant tail。

## 16. Miniflare、workerd 与 WDL 参考边界

### 16.1 Miniflare

采用 pinned Miniflare/Wrangler 证据：

- config normalization、upload metadata 与 application/rollout call order；
- Container metadata关联到 DO namespace；
- `containerEngine.localDocker` socket discovery/config；
- Wrangler build/pull/tag local image；
- workerd按 Worker call启动本地实例；
- safe environment 下的 FUSE privilege detection；
- dev session teardown与hot rebuild行为。

明确不复制：

- raw Docker socket作为 production boundary；
- developer Docker context/PATH discovery进入 `ocd` startup；
- 首次 Worker request build/pull image；
- local dev 不执行 `max_instances`；
-自动授予 FUSE、device、CAP_SYS_ADMIN 或 rootful Docker privilege；
- Miniflare临时 tag/cache作为 image authority；
- dev-only cleanup/error/retry 作为 production lifecycle。

### 16.2 workerd

复用 current fork 的 native Container API、RPC、Docker client、exec streams、port tunnel、monitor、egress interception 与 reconnect
代码。新增内容限于：

- dynamic loaded DO class的 Container attachment/policy；
- platform-owned identity/resource labels；
- Broker所需的严格 provider metadata；
- compatibility/security bug fix及其 upstream-style tests。

不复制第二套 Container runtime到 Rust/TypeScript。fork变更在 `third_party/workerd/` 独立提交，再由协调 pin、archive/digest、
compatibility date/flags和正式 real-runtime Gate更新主仓库 gitlink与 runtime lock。

### 16.3 WDL

[WDL](https://github.com/wdl-dev/wdl) 当前没有 `ctx.container`/Cloudflare Containers compatibility implementation。它使用
容器部署自身平台组件，不作为 P16 provider或API authority。后续发现的 WDL功能只有通过同一官方合同/测试后才可引用，不能因
项目也是 self-hosted Workers platform 就推断兼容。

## 17. 实施顺序

### CT0：冻结合同

- 固定 Wrangler schema、upload metadata、deploy/container command call graph；
- 固定 Workers types、`@cloudflare/containers` package tarball/integrity/source；
- 固定 `ctx.container` JSG descriptors、exceptions、streams、Fetcher/Socket/monitor behavior；
- 固定 public API/OpenAPI、rollout状态、CLI polling/exit与remote traces；
- 建 config/field/route/API/error/compatibility-flag/capability inventory；
- 把 Worker API compatibility 与 single-machine fleet deviations 分开登记。

### CT-G0：动态 Container feasibility Gate

- 使用当前 fork与一个 operator-installed Docker engine进行 disposable probe；
- static `DoHost` 中只给一个声明 Container 的动态 tenant class挂载 native `ctx.container`；
- 固定 `@cloudflare/containers` constructor/start/readiness/fetch/WebSocket/stop通过；
- low-level start/exec streams/monitor/getTcpPort/outbound intercept通过；
- 两个 tenant classes使用不同 immutable image，non-Container class看不到 capability；
- DO eviction、tenant version change、workerd restart与engine process restart不串 identity；
- 记录需要的最小 workerd extension与Docker API trace；probe代码完成后删除，保留结果证据。

Exit：若 native capability不能安全挂到 dynamic loaded DO/facet，P16保持 unsupported；不能退回JS模拟或把所有tenant共用一个
静态 image/container capability。

### CT1：workerd dynamic attachment

- 实现 `ContainerClassPolicy` 与dynamic loaded DO attachment；
- per-class/per-object/per-generation identity和capability fencing；
- platform image/resource/egress policy注入；
- tenant experimental override拒绝；
- native workerd unit/integration tests与formal fork commit。

### CT2：image/application authority

- P6 metadata decode、current Day 1 SQL/schema/checksum；
- OCI manifest/config/layer validation与artifact authority；
- application/target/rollout/lease repositories；
- immutable image digest、rollback pointer与API wire models；
- provider image materialization contract。

### CT3：Container Engine Broker

- operator config、Unix socket validation、capability/version handshake；
- minimal Docker Engine subset或单一native provider protocol；
- image/resource/privilege/network/mount enforcement；
- application + egress sidecar lifecycle、status/reconnect/cleanup；
- provider crash/restart、orphan与corrupt cache recovery。

### CT4：runtime API 与 official package

- full supported `ctx.container` inventory；
- exec stream/PTY/AbortSignal、Fetcher/Socket/WebSocket；
- monitor/destroy/signal/restart races；
- outbound HTTP/HTTPS/TCP interception；
- fixed official package lifecycle/alarm/schedule/hooks/helpers。

### CT5：Wrangler/control plane/rollout

- standard config/upload/deploy sequence；
- application/image/rollout routes与CLI polling；
- local single-machine capacity与rollout worker；
- list/status/delete/image/instance command subset；
- P12 launcher/target/auth集成与unsupported command behavior。

### CT6：security、operations 与 qualification

- cgroup/runtime effective resource checks、egress/private-network regression；
- identity/cross-account/cross-script/cross-version differentials；
- Broker/workerd/`ocd` crash、restart、upgrade、orphan cleanup与soak；
- P7/P9、readiness、metrics、backup（authority metadata only）与runbook；
- fixed Wrangler/package/remote differential；
- capability/deviation/reference/examples/Dashboard同步。

## 18. 必测矩阵

| case | 预期 |
| --- | --- |
| standard JSONC `containers[]` | fixed Wrangler normalization与metadata精确通过 |
| Container class未绑定同script SQLite DO | deploy固定失败，无部分application |
| unsupported placement/affinity/privilege |明确拒绝，不静默忽略 |
| Dockerfile build/push | build在Wrangler/developer侧；`ocd` startup/request不build/download |
| image tag changes after deploy | runtime仍使用admission时固定digest |
| missing/mismatched image | rollout/start fail closed，无旧tag fallback |
| official package constructor | Container-enabled class成功；普通class无`ctx.container` |
| two tenant classes/images | identity/image完全隔离，不受共享`DoHost`影响 |
| `start()`/already running | sync/async和exception与fixture一致 |
| `exec()` streams/output/combined/PTY | bytes、EOF、backpressure、single-consume一致 |
| exec AbortSignal/kill/invalid signal | process与JS异常正确，无leaked exec |
| `getTcpPort().fetch/connect` | HTTP/WebSocket/TCP/half-close与native behavior一致 |
| `monitor/destroy/signal` | resolve/reject/reason/exit/restart generation一致 |
| official package readiness/sleep/alarm | restart/eviction后无重复或lost timer |
| outbound disabled | Container无general Internet |
| outbound enabled | 只走public Network；private/metadata/platform listener拒绝 |
| HTTP/HTTPS/TCP interception | matching、replacement、open connection behavior一致 |
| cross-account/script/object ID | 不能连接、控制或观察其他instance |
| tenant尝试image/privilege/socket override | native boundary拒绝；Broker无危险request |
| max instances/pending starts | stable capacity error/Retry-After，无无限队列 |
| effective CPU/memory/disk/PID | provider实际限制与reported profile一致 |
| app crash/sidecar crash | monitor/egress/recovery分别正确收敛 |
| Broker unavailable/restart | readiness degraded；匹配实例reconcile，否则lost |
| workerd/`ocd` restart | identity、image、lease、monitor不串代 |
| unknown provider container | 不认领、不删除operator其他workload |
| rollout mixed generations | old/new按target/lease可解释，DO storage不变 |
| rollout SIGTERM/drain/SIGKILL | fixed grace/15-minute stop contract通过 |
| rollout later step fails | Worker active事实与rollout失败状态保留 |
| rollback | 新target generation指向旧digest，不复活旧capability |
| raw Docker/provider error | Worker/API/log均为sanitized stable mapping |
| Wrangler deploy/list/status/delete | route、envelope、polling、exit code与fixed CLI一致 |
| unsupported Wrangler SSH/command | 明确失败，不暴露host shell/socket |

## 19. Definition of Done

P16 只有同时满足以下条件才可归档：

- 固定 Wrangler的标准 Container config、upload、application/image/rollout序列对真实 `ocd` 通过；
- 固定 Workers types和`@cloudflare/containers`在正式 pinned workerd中直接运行，无custom package/import rewrite；
- supported `ctx.container` API的descriptor、exception、stream、Fetcher/Socket、monitor与compatibility flags逐项通过；
- dynamic `DoHost` 只为声明class附加native capability，不共享静态image或泄露给non-Container class；
- immutable image digest、application target、instance lease、capacity与rollout由`ocd`/SQLite authority持有；
- Broker是operator prerequisite，不被open-compute下载、打包、启动、supervise或公开；
- workerd/Broker没有raw Docker socket通用权限，dangerous HostConfig/mount/device/capability全部fail closed；
- CPU/memory/disk/PID/instance/network limits经过provider effective-state测试，不只验证配置；
- image chain/digest/materialization/cache corruption与offline runtime startup通过；
- app/sidecar/Broker/workerd/`ocd` crash、restart、eviction、rollout、rollback、orphan cleanup和soak通过；
- single-machine placement/isolation/rollout deviations进入compatibility matrix、capability manifest和用户文档；
- WDL/Miniflare只作为固定source evidence，不成为production authority或fallback；
- Cloudflare remote differential完成，或credential/availability限制拆成独立active acceptance；
- P6/P7/P9/P12、references、examples、runbook和Dashboard同步；
- 正式release仍是单个`ocd` executable + 既有单个pinned workerd child，外部runtime/Broker边界明确。

文档变更本身只运行`git diff --check`、Markdown链接和固定源码/命令核对。实施属于workerd fork、protocol、container/process、
network、security、persistence、artifact和release变更，必须先显式`bun run build`准备runtime assets，再执行仓库`AGENTS.md`
要求的focused tests、coverage与最终单轮workspace Gate。Linux rootful/FUSE、loopback mutation或其他privileged fixture仍需用户明确授权。
