# Containers 运维

Containers 使用独立的 operator-owned Docker Broker。`ocd` 保存镜像、application target 和 execution lease；
Broker 执行受限的 Docker 操作。`ocd` 不启动 Docker 或 Broker，不在启动或请求路径构建、拉取镜像。

当前 [P23](../implemented/p23-cloudflare-containers.md) 已实现固定 65 项 API、完整快照与目录恢复。
2026-10-23（UTC），R22 输入通过三项真实产品用例、coverage 与最终 Gate 子进程，以及三宿主候选包检查。
最终 Gate 外层守卫失败与独立摘要补证分别保留；正式 tag 工作流与公开发布尚未执行。
以下按日期保留的记录不覆盖本段当前状态；完整原始证据与限制由 P23 结果页记录。
当前全量验收目标包含 snapshot capture、snapshot restore 和 `setInactivityTimeout()`。
2026-10-20（UTC），R21 输入下的 Linux ARM64 coverage 和最终 Gate 已通过。
同日，macOS ARM64 的当前 P23 两项真实产品用例均通过。
原完整 macOS 批次与两个失败目标的分批复验分别保留，详见 [P23 结果](../implemented/p23-cloudflare-containers.md)。
固定 65 个成员的编译用例与真实容器用例已登记；发行验收仍待完成。
这些 stable 成员属于兼容目标，不属于 experimental。
正式 workerd 输入由 [`workerd.lock.json`](../../packages/runtime/workerd.lock.json) 拥有。
Provider 资格、产品 Gate 与成员资格是不同证据。
产品验收前，应确认 Provider 合同接口返回成功，且摘要与资格输入相同。
进程身份和 Engine 库存相同，不足以证明 Provider 仍可用。
原版 SDK 的 HTTP 场景不验证 TCP、WebSocket 或 HTTPS 拦截行为。
当前 types 固定为 `@cloudflare/workers-types@5.20260930.2`，Container 清单有 65 个稳定成员。
成员包括方法、属性和参数类型，不是 65 个方法。
2026-10-21，用户确认保留完整 65 项，并补齐九个目录成员的真实恢复能力。
验收依据是固定类型、匹配版本的上游 API 和本地真实行为。
托管实测用于发现差异，不要求逐项复刻，也不替代本地权限、持久化和产品测试。
`snapshotDirectory()` 的创建入口仍保留实验开关；这不免除稳定目录恢复字段的实现和验收。
旧 64 项清单包含 `snapshotDirectory()`；当前 stable types 已移除该方法，并新增 `images` 与 `inspect()`。
当前 capability manifest 声明这 65 个成员的 API 实现范围，不声明完整发行验收通过。
实现结果与验证记录由 [P23](../implemented/p23-cloudflare-containers.md) 拥有。

## Provider 前提

当前 provider profile 需要 rootful Linux Docker、Engine API 1.42 或更高版本、cgroup v2、seccomp 和 overlay2。
rollout 的 stop 请求显式使用 `SIGTERM`，不采用 image 的 `StopSignal`。
[Docker Engine API 1.42](https://docs.docker.com/reference/api/engine/version/v1.42.yaml) 提供该参数；
更早的 API 不满足此 profile。
overlay2 的 writable disk limit 需要 XFS backing filesystem 并启用 `pquota`，见
[Docker storage driver 说明](https://docs.docker.com/reference/cli/docker/container/run/#set-storage-driver-options-per-container---storage-opt)。
安装与声明的 CPU、内存、磁盘、PID 和文件上限相符的 engine。
Broker 检查实际 cgroup、进程、网络和磁盘行为；Docker 接受配置字段不代表资源限制生效。
2026-10-10 的 OrbStack Docker 29.4.0 允许写入超过 `storage-opt size` 的数据，不能作为成功资格证据。
同日的独立 Debian 测试 machine 使用 XFS 与 `pquota`。systemd cgroup driver 将 0.0625 vCPU
取整为 0.07 vCPU，导致实际限制与声明不符；改用 cgroupfs 后，CPU、内存、磁盘、PID 和文件限制检查通过。
同一 machine 的正常生产 Broker 还通过 sidecar 协议、鉴权和双向回调资格检查。
两个 probe 已删除并确认不存在，Engine 的容器、镜像、网络和卷库存没有变化。
2026-10-11，日志确认旧 Broker 首次失效发生在响应 headers 阶段。
固定 5 秒期限耗尽；Engine 未重启，socket 未替换。
首次失效时间为 `2026-10-10T22:24:45.519807Z`。
响应延迟超过 5 秒的底层原因尚未确定。
Broker 记录固定阶段和原因，不记录原始错误或凭据。
失效后仍拒绝 mutation，不自动重连或重新授权。

2026-10-11，新源码的正常 Broker 在 4 GiB、2 CPU machine 上通过资格检查。
检查耗时 2.597 秒；两个镜像证明均返回 `200`。
两个 probe 已删除，最终 inspect 均返回 `404`。
Engine 库存和 anchor 保持不变，合同摘要不变。
后续约 0、6、11 秒的三次合同检查均返回 `200`。
这些有界检查只证明当时的 provider 资格，不证明长期健康或 Container 产品验收通过。
cleanup-only 私有协议的真实产品验收以 P23 结果页为准。

2026-10-12，快照候选 Broker 在实际 4 GiB、2 CPU machine 上通过独立资格检查。
检查耗时 40.670 秒；合同返回 `200`，`qualified` 与 `snapshots` 均为 true。
检查包含同一 probe 的资源限制、暂停、导出、恢复进程及删除确认。
最终 Engine 库存未变，probe 已删除，spool 为空。
该结果不证明新 native 协议或完整 stable API 的产品验收通过。

同日，包含 exec helper 的新 Broker 在实际 4 GiB、2 CPU machine 上通过资格检查。
检查耗时 38.313 秒；合同确认 `qualified`、`snapshots`、sidecar image 与 helper SHA-256。
两个 probe 均已删除，最终 inspect 均返回 `404`，spool 为空。
Engine 库存保持不变，operator-owned helper volume 保留。
三次合同读取均返回相同字节；此结果不证明新原生协议的产品行为。
资格进程保留运行；该进程使用 Provider 私有协议 v1。

snapshot size 修正将 Provider 私有协议升为 v2。
`/images/check` 成功时返回实际 Engine `Size`，不再返回空 body。
新 consumer 拒绝 v1；native Container 协议仍为 v3。
上述 v1 资格证据不覆盖新协议。
2026-10-13，v2 Broker 在实际 4 GiB、2 CPU machine 上通过独立资格检查。
检查耗时 35.926 秒；合同确认 `qualified` 与 `snapshots` 均为 true。
两个镜像的返回大小均与实际 Engine `Size` 相同。
两个 probe 已删除，最终 inspect 均返回 `404`，spool 为空，Engine 库存未变。
该结果不证明原生 Container 产品或完整 stable API 已通过验收。
后续 Wait 正文修复改变 Broker 源码，应重建 Broker 并重新验证资格。

预先物化 qualification image 和 interceptor sidecar image，并配置各自的 immutable image configuration digest。
qualification image 提供 `/bin/sh` 和 `dd`。sidecar 还须通过固定协议、控制端鉴权和 CA 检查。
无鉴权的上游 sidecar 不满足该 profile。Broker 不自动替换缺失镜像，也不把普通 builder image 当作 sidecar。

sidecar 使用固定的认证 profile v2。workerd 从私有 control relay 主动建立连接，
sidecar 通过这些连接发送反向回调。资格检查验证回调 token 和实际双向交换，无需开放 Docker bridge 上的宿主监听。
空闲反向连接收到 EOF 或提前数据时，sidecar 关闭该连接。
关闭或取用闲置连接时，sidecar 立即释放 16 条 unused pool 中的槽位；active 连接沿用原预算。
sidecar 先结束空闲 reader，再把连接交给 callback caller，避免两个 reader 竞争协议字节。
连接关闭不代表 main process 退出，也不释放 execution lease 的容量。
application container 固定移除 `NET_RAW`，并拒绝新增 capability；实际 inspect 应确认这一限制。
这防止 application 在共享网络命名空间内抓取控制端认证数据。

Broker 先验证本 lease 的 proxy labels、镜像和资源，再读取实际 Docker ID。
application 的网络命名空间应精确引用该 ID；Docker 规范化名称不授权其他 proxy 或网络。
身份不匹配时拒绝操作，不按名称前缀或其他 alias 放行。

Docker socket 只交给 Broker。`ocd` 与 workerd 只访问 Broker 的私有 Unix socket。
socket 使用绝对路径，编码长度最多 103 bytes；父目录、owner、mode、symlink 与 peer UID 检查失败时拒绝访问。
容器化部署还须保证 Broker 与 workerd 使用约定的 loopback relay 地址和网络命名空间。

## CI 夹具输入

CI 在运行 Gate 前应准备镜像、sidecar、exec helper 和独立 Broker。
普通用户使用 `test/containers-fixture/` 中的工具准备输入。
这些工具不安装 Docker，不拉取镜像，也不替换已有 Engine 或 Broker。
运行环境应先满足上述 Provider 前提。

从已验证的 sidecar image 建立新的 helper volume：

```sh
python3 test/containers-fixture/materialize-helper.py \
  --engine-socket "$ENGINE_SOCKET" \
  --image "$SIDECAR_IMAGE" \
  --helper-sha256 "$EXEC_HELPER_SHA256" \
  --volume "$EXEC_HELPER_VOLUME" \
  --architecture "$ENGINE_ARCHITECTURE" \
  --output "$HELPER_PROOF"
```

operator 应填写实际 socket 绝对路径、image digest 和 helper SHA-256。
`ENGINE_ARCHITECTURE` 使用 `arm64` 或 `amd64`；volume 名称和证明文件路径应尚不存在。
工具拒绝已有容器的 Engine，也拒绝复用已有 volume。
工具校验两个镜像路径中的 helper，写入新卷，再核对回读字节。
成功后，工具删除并确认本次创建的容器不存在，保留新 helper volume 和私有证明文件。
本工具的成功结果不代表 Broker 已通过资格检查。

`oci-images.py` 只生成两个 OCI layout 的镜像输入，供应用构建入口使用。
`prepare-input.py` 还要求真实 Broker 合同，生成 Gate 所需的完整输入。
不能用只有镜像的输入替代完整输入。
`with-broker.py` 从指定 binary digest 建立独立副本，启动新 Broker，再运行显式命令。
工具向该命令传递私有 `OPEN_COMPUTE_TEST_CONTAINERS_INPUT` 文件路径。
命令结束后，工具终止并回收本次 Broker，核对 socket 和容器清理结果。
每次运行应使用只有私有 `broker.toml` 文件的新目录，不能重用上次的 state 或 spool。
Engine、sidecar、helper 和镜像的供应由运行环境负责；工具不把既有环境视为新 runner 的资格证据。

## 独立启动 Broker

准备 operator-owned TOML 文件，使用实际绝对路径、UID 和已验证的镜像 digest：

```toml
socket = "/var/lib/open-compute/run/container-broker.sock"
engine_socket = "/var/run/docker.sock"
peer_uid = 65532
sidecar_image = "sha256:<verified-sidecar-config-digest>"
exec_helper_volume = "open-compute-exec-helper"
exec_helper_sha256 = "<verified-helper-sha256>"
qualification_image = "sha256:<verified-probe-config-digest>"
max_leases = 4096
max_instances = 16
max_requests = 128
relay_host = "127.0.0.1"
spool_dir = "/var/lib/open-compute/container-broker-spool"
state_dir = "/var/lib/open-compute/container-broker-state"
max_image_bytes = 1073741824
max_stream_bytes = 67108864

[resources]
cpuNanos = 1000000000
memoryBytes = 1073741824
diskBytes = 4000000000
pids = 128
openFiles = 1024
```

```sh
ocd container-broker --broker-config /etc/open-compute/container-broker.toml
```

该命令以前台独立进程运行。operator 管理其启动和退出；普通 `ocd run` 不 supervise Broker。
`state_dir` 是必填的持久私有目录，mode 为 `0700`，由一个 Broker 独占。
它保存凭据写入、proxy 删除和目录暂存的非 secret pending marker。
操作结果未知时，重启后仍拒绝该 lease 的新授权。
确认两个容器均已删除后才清除 marker。不要把该目录作为可删除 cache。
未配置 sidecar 或资格结果为 `qualified = false` 时，Broker 只提供诊断合同，拒绝 mutation。
诊断合同中的 `qualified = false` 不授权 Worker upload 或 Container start。
资格检查返回错误时，Broker 退出，不提供合同。
Broker 并行核对 sidecar 对缺失或错误凭据的拒绝。
12 个响应均应为 HTTP 401。
每个鉴权请求的期限为 3 秒，整次 sidecar 资格检查的期限为 30 秒。
Broker 的 sidecar 资格失败事件记录固定阶段和整体期限标记。
事件不含凭据或上游异常。
`exec_helper_volume` 与 `exec_helper_sha256` 是必填输入。
operator 应从固定 sidecar image 中复制 `/.open-compute-exec/helper`，预先建立 helper volume。
volume 只能使用 local driver，不能指定 driver options。
volume 应包含与 sidecar image 和 helper SHA-256 匹配的两个 label：
`dev.open-compute.exec-helper.image` 和 `dev.open-compute.exec-helper.sha256`。
Broker 核对 volume metadata、实际挂载、可执行文件字节和 SHA-256。
主容器将 helper volume 只读挂载到根目录下的 `/.open-compute-exec`。
main、exec 和目录 helper 入口拒绝符号链接或可写文件系统。
该路径不能通过移动可写父目录来替换；sidecar 不挂载此 volume。
Broker 不自动创建或修复此输入，也不在 execution 终止时删除此输入。
快照不保存此挂载中的文件。
`max_image_bytes` 同时限制 registry staging、压缩与解压后的 image graph，以及 Docker-save transport。
operator 应按完整镜像设置该上限。

启动资格检查创建独立 probe container，正常完成后删除并确认其不存在。
若 Broker 在 create 响应未知时退出或被强制终止，probe 可能残留。
operator 应核对本次检查生成的名称、image digest 和 Docker ID，再删除该 probe。
probe 的自动 crash recovery 属于后续范围；execution lease 的持久 fencing 不依赖该 probe。

## 实例 admission

在实例配置中选择同一个 Broker endpoint，并填入当前已通过资格的 provider contract SHA-256：

```toml
[containers]
broker_socket = "/var/lib/open-compute/run/container-broker.sock"
provider_contract_sha256 = "<qualified-contract-sha256>"
max_instances = 4
max_snapshots = 32
max_snapshot_bytes = 8589934592

[containers.resources]
cpuNanos = 1000000000
memoryBytes = 1073741824
diskBytes = 4000000000
pids = 128
openFiles = 1024
```

实例上限不能超过 provider 已验证的上限。workerd 还须提供匹配的原生 attachment 协议。
正式 binary 缺少该协议或 Broker 不合格时，Container admission 拒绝请求；其他 Worker 保持原有入口。
`max_snapshots` 与 `max_snapshot_bytes` 是必填的快照保留上限。
前者限制快照数量，范围为 1–100000；后者限制保留字节，范围为 1–9223372036854775807。

应用使用固定 cf、官方 Vite 插件和 `@cloudflare/containers`，配置方式见 [P23 标准配置](../implemented/p23-cloudflare-containers.md#51-标准配置)。
2026-10-13，固定 `@cloudflare/config@0.23.0` 的绑定类型要求 `worker` 字段。
仓库补丁按该 [官方配置转换器](https://github.com/cloudflare/workers-sdk/tree/main/packages/config) 的 `config.name` 比较目标 Worker。
同 Worker 绑定省略 `script_name`；跨 Worker 绑定保留该字段，仍受官方跨 Worker 容器校验限制。
转换回归见 `test/cf-config.test.mjs`；该补丁不增加跨 Worker 容器能力。
cf 在开发者侧构建和上传镜像。平台验证完整 OCI graph，将 tag 解析为不可变 digest，保存离线 artifact。
SQLite 同时保存配置 digest 和有序的 layer diff IDs；最多 128 个，每个为 32 bytes。
application create/update 只验证已准入镜像并提交 target，不导入 Engine，也不启动容器。

`default` scheduling policy 从不可变 target 取得镜像和资源。
该 policy 拒绝 `start()` 中显式的 image、instance profile 或自定义资源。
env、labels、entrypoint 和 Internet 设置仍可传入。
`durable_object` policy 没有默认 image 或 configuration。
Worker Version 保存每个 named image 的不可变 reference；`Container.images` 返回该声明的副本。
每个 Container 声明最多包含 100 个 named images；每个名称包含 1–128 个字符。
应用应在 `start()` 中显式选择 image 或 container snapshot。
省略 `instance` 时使用 Lite；operator 上限不作为默认实例大小。
平台只准入已验证的普通 OCI artifact，不自动拉取缺失镜像。
snapshot capture 与 restore 只对 `durable_object` policy 开放。
上述 policy 边界见 [Durable Object Container API](https://developers.cloudflare.com/containers/api/durable-object-container/)。
named images 边界见 [Scheduling Policies](https://developers.cloudflare.com/containers/configuration/scheduling-policy/)。

首次 execution start 先保留 lease 和容量，再检查 Engine 的配置 digest、有序 rootfs、Linux OS、架构和 volume 声明。
只有明确的 image 404 才从已验证的离线 artifact 生成有界 Docker-save transport，并通过 Broker 导入。
导入后还须通过相同证明。证明不匹配、响应未知或 provider 不可用时拒绝请求，不删除或替换已有 Engine 镜像。
重启后复用同一 lease；Engine 镜像存在且证明匹配时不重复转换或导入。runtime 不重新解析 tag，也不从公网 registry pull。

## 快照持久化与回收（验收中）

本节定义当前本地快照流程；R21 的真实产品结果和原始报告由 [P23](../implemented/p23-cloudflare-containers.md) 记录。
该结果不证明全部 65 个成员或正式发行资格。
官方快照合同见 [Snapshots](https://developers.cloudflare.com/containers/guides/snapshots/)，于 2026-10-12 核对。
快照保存文件系统，不保存内存、进程或挂载文件系统。
快照只能在同一 application 和原始镜像版本内恢复，可跨 Durable Object 使用。
快照在捕获完成后保留 30 天；成功恢复后重新获得 30 天保留期。
周期性状态检查不延长保留期。

`ocd` 在 SQLite 中记录 `pending`、`ready` 和 `retiring` 三种快照状态。
捕获前，`ocd` 冻结 source lease、start identity 和配额。
`ocd` 先预留原始镜像缓存、rootfs 和输出文件所需的磁盘空间。
rootfs 和输出文件均使用匿名临时文件；进程退出后，内核回收文件。
Broker 验证实际 Docker ID 与 start identity 后暂停进程，导出有界 rootfs，再恢复进程。
Broker 确认同一进程已恢复后才返回 archive。
`ocd` 验证 archive，并用原始镜像配置生成单层 OCI artifact。
平台不把 runtime env 或控制端凭据复制到镜像配置。
捕获的文件内容仍由应用负责；应用写入文件的秘密也属于快照内容。
application 的 `/run` 使用受内存上限约束的 tmpfs；快照不保存该挂载中的内容。

`ocd` 在上传前持久化 artifact digest 与大小。
发出 PUT 前，`ocd` 持久化不可撤销的 `put_attempted` 标记。
上传成功后，`ocd` 复用有界离线导入路径，校验新快照镜像的 config digest 和有序 diff IDs。
`ocd` 从该镜像读取 Docker `ImageInspect.Size`，并在 ready 事务中冻结独立的 `image_size`。
原捕获临时文件关闭后，平台释放捕获阶段的磁盘预留；导入阶段使用自己的磁盘预留。
只有完整 artifact 校验和 source authority 校验通过后，快照才进入 `ready`。
上传响应未知、凭据旋转或捕获失败时，快照继续占用配额。
客户端断开不会取消已经准入的上传。
daemon 重启后，maintenance 继续回收未完成的快照。
如果捕获未发出 PUT，maintenance 确认 source 已终止、artifact 已不存在后释放配额。
如果 PUT 已发出但捕获未完成，maintenance 永久保留 `retiring` 配额。
HEAD 404 不能证明远端 PUT 此后不会提交，因此不解除该标记。
当前版本没有自动解除未知 PUT 配额的流程。
未回收的快照也阻止 application 删除；未知 PUT 可能永久阻止删除。
使用快照恢复且未终止的 execution 保留 artifact。

`pending` 和 `retiring` 按捕获时的最大保留字节计费；`ready` 按实际 artifact 字节计费。
同一 artifact 仍有引用时，回收继续保留配额。
快照生成的镜像属于私有恢复输入，普通 image override、application target 和 registry tag 不授权使用该镜像。
候选源码的 `ContainerSnapshot.size` 返回新快照镜像实际的 Docker `ImageInspect.Size`。
匹配 upstream workerd 也返回该字段。
SQLite 单独保存 `image_size`；`artifact_size` 继续用于 artifact 校验和配额。
镜像导入或大小校验失败时，捕获不能进入 `ready`；已发出的 PUT 继续占用配额。
捕获在返回前导入新快照镜像；未恢复的快照也会占用 Engine 磁盘。
Engine 保留已导入的镜像缓存；快照到期或 application 删除不会回收这些镜像。
snapshot 配额和 lease 的 `cleanupComplete` 不计入或回收 Engine 镜像缓存。
operator 应单独预算 Engine 磁盘，并回收已不用的镜像缓存。
托管结果尚未确定完整计量公式；该差异保留为诊断，不作为 API 验收门槛。
当前 P23 产品用例已核对持久化大小、新快照镜像的 Engine 大小和重启后的同一数值。
这些本地不变量不证明托管计量公式相同。

### 目录恢复实现状态（验收中）

目录恢复清单包含备份 ID 和容器内的目标路径；不含备份 ID 的项表示新建空目录。
平台在同一 SQLite 事务中校验清单、预留执行并冻结清单。
平台拒绝根目录、重复目标路径、其他应用的备份、整容器备份和已过期的备份。
已有执行保留其目录备份，daemon 重启和备份过期不解除该引用。
停止中的执行、旧执行标识和旧对象权限不能取得恢复输入。
应用删除期间，artifact 回收也保留存活执行使用的目录备份。

目录捕获服务和存储回归覆盖 `default` 与 `durable_object` 两种调度策略。
整容器快照的策略限制不变。
2026-10-21（UTC），Linux ARM64 的实际 Engine fixture 验证了独立目录挂载。
恢复后的文件内容、UID、GID、mode、硬链接和相对符号链接均保留。
目录与根文件系统共用 32 MiB 的 XFS project quota；根文件截断后，目录可使用释放的容量。
整容器 export 不含目录挂载中的文件。

目录 helper 只接收固定路径，不接收租户提供的宿主路径或 project ID。
helper 在租户启动前，将空卷加入根文件系统的 project；helper 不增加 Linux capability。
helper 拒绝符号链接、非空目录、其他文件系统和已有 project 的目录。
XFS 允许文件所有者改写 project 属性；仅移除 `CAP_SYS_ADMIN` 不能守住限额。
主进程与 exec 入口因此叠加 seccomp 过滤，并同步到全部线程。
过滤拒绝 `FS_IOC_FSSETXATTR` 和两种 ABI 的 `FS_IOC_SETFLAGS`；租户不能移除 project 或继承标志。
其他 ioctl 继续受原 Docker profile 约束。

目录归档解码器验证完整 OCI artifact，只输出唯一一层的原始 tar，不向宿主目录解包。
Broker 在注册时冻结目标路径、tar 摘要和字节数；本地上传不能改写该清单。
Broker 校验 tar、摘要和大小，然后将归档追加到每个执行独有的匿名文件。
归档通过偏移量区分；目录数量不会增加暂存文件描述符的数量。
所有目录归档的总大小不能超过 `max_image_bytes`。
追加前的持久 marker 保留取消或部分写入的隔离；失败后不能继续启动。
停止和清理释放匿名文件；重启不信任已丢失的暂存数据。

这些字段使用 Broker 私有协议 v4；旧合同在 consumer 预检时被拒绝。
候选源码已接入 `directorySnapshots` 的权限校验、冻结清单、归档传输和实际挂载。
Linux ARM64 Broker 已通过真实目录恢复、重复恢复和重启恢复检查。
R22 workerd 通过现有回调传递目录参数，私有 Container 协议为 v4。
正式 pin 已更新为 R22；四个平台构建、发布和十个资产的校验均已完成。
R22 真实普通容器用例已覆盖公开目录 API。
完整 coverage、最终 Gate 用例和三宿主发行包的独立状态由 [P23](../implemented/p23-cloudflare-containers.md) 记录。
当前证据和后续验收由 [P23](../implemented/p23-cloudflare-containers.md) 记录。

### CF 对比证据与冲突

2026-10-12，独立托管探针通过跨 Durable Object 恢复。
恢复结果保留 64 KiB 文件的 SHA-256、文件 mode `0600` 和目录 mode `0711`。
探针资源已删除；Worker、application 和 namespace 库存与测试前相同。
这些结果不替代本地产品 Gate。

同日，托管 signal 探针向主进程发送 `SIGUSR1`。
主进程的 trap 返回 `23`；`monitor()` 拒绝并携带 `exitCode = 23`，`running` 变为 false。
TCP 探针在确认应用监听后，通过 `getTcpPort().fetch()` 保留 POST、路径、query、请求头和正文。
首次未等待监听的 TCP 请求失败；该失败记录保留。
两次探针均已删除资源，最终库存与各自测试前相同。

同日，新的托管探针通过 HTTPS 拦截与 WebSocket 检查。
HTTPS 客户端信任注入 CA，并校验主机名、POST、请求头和正文。
客户端确认响应状态 `201`、响应头、正文和连接关闭。
WebSocket 探针确认 `101`、响应头、带 mask 的请求帧和 echo 正文。
关闭握手保留 code `1000`、reason `finished` 和 `wasClean = true`。
容器内的 WebSocket server 返回退出码 `0`，stdout 到达 EOF。
application 与 Worker 删除均返回 `200`；三项库存与测试前相同。
该探针不验证 wildcard、其他端口、规则上限或本地 runtime 行为。
前一次探针在部署后的只读 API 请求发生传输失败，未执行 tenant 检查；资源已清理。

后续托管 PTY 探针确认初始尺寸为 80 列、24 行。
调用 `resize()` 后，真实 `SIGWINCH` 触发，尺寸变为 100 列、30 行。
`ExecProcess.pid` 与 Bash 的实际 PID 均为 `4`。
stdin 关闭后，stdout 保留合并后的两个输出流，进程返回退出码 `7`，输出到达 EOF。
combined PTY 的 `ExecProcess.stderr` 为 `undefined`。
官方 cf build 与 deploy 均成功退出；资源已删除，三项库存与测试前相同。
原先未分离启动阶段的 PTY 探针返回 `Worker not found.`，该失败记录保留。
这些托管观察不替代本地产品验收。

2026-10-13，CF19 在同一容器中捕获两份内容不同的快照。
两个新 Durable Object 分别恢复了对应的 marker 和 SHA-256。
A、B 的快照 ID 不同，恢复后的 `inspect().image` 却相同。
恢复后的 reference 与旧托管观察相同。
原容器仍返回 `cloudflare/debian-trixie`。
公开 application metadata 未提供 immutable image 对应关系。

本次对照不能确定返回值属于 base、派生 snapshot 或共享平台镜像。
本次对照也未确定 registry 命名公式，恢复值的来源冲突仍未解决。

CF19 的官方 build、deploy 和单次 driver 均退出 `0`。
独立复核确认 Worker、application 和 namespace 库存与测试前相同。
测试已删除新建 application 和 Worker；临时 secret 不存在。

测试未调用 snapshot 删除接口。
本次调查未找到公开 snapshot 删除接口。
application 删除不证明 snapshot 字节已回收或计费停止。
官方 snapshot TTL 为 30 天，restore 会刷新 TTL。
删除 application 后的 snapshot 保留行为未确定。
证据标识为 `inspect-image-controlled-19-preparation/hosted-observations-review.json`。

### 已接受的快照兼容性差异

2026-10-21（UTC），用户接受以下两项差异。
平台按已合并的上游 API 改动和官方明确写出的恢复语义实现。
两项差异不再阻止本次验收或发行资格；本地功能和其余必需检查仍需通过。
下表保留官方文档和托管观察的不同结果。

| 行为                             | 官方来源（文档更新于 2026-09-30） | 2026-10-12 托管结果             | 当前实现                                     |
| -------------------------------- | --------------------------------- | ------------------------------- | -------------------------------------------- |
| 从快照恢复后的 `inspect().image` | 空字符串                          | 非空的 registry image reference | 空字符串，遵循文档                           |
| 省略 `snapshotContainer()` 参数  | 抛出 `TypeError`                  | 捕获成功                        | 参数可省略，与固定 types 和匹配 workerd 一致 |

文档来源为 [Durable Object Container API](https://developers.cloudflare.com/containers/api/durable-object-container/)。
固定 types 为 `@cloudflare/workers-types@5.20260930.2`。
匹配 upstream workerd revision 为 `d99bc6b777e35d72d71c2f1fe2fd1db53284528a`。
2026-09-21（UTC），上游 [PR #7453](https://github.com/cloudflare/workerd/pull/7453) 将 `snapshotContainer()` 的选项改为可省略。
固定 R22 的上游基线包含该修复；修复不含 compatibility date 分支。
该版本行为与已保留的托管成功观察一致，官方 API 文档仍声明选项必填。
官方作者在 PR 中确认，可省略选项是原始 API 设计意图。
平台允许 `snapshotContainer()` 和 `snapshotContainer({})`；不复刻文档所述的省略参数错误。

2026-09-30（UTC），官方 [文档 PR #33825](https://github.com/cloudflare/cloudflare-docs/pull/33825) 明确说明恢复后的空字符串。
`inspect().image` 表示传给 `start()` 的镜像引用；快照恢复没有传入该引用。
平台因此返回 `""`，不生成或推测托管环境的 registry 引用。
托管非空引用的来源、生成公式及可比较范围没有明确对照，保留为未知。

接受范围仅限这两个可观察结果，不免除文件内容、权限、隔离、跨 DO 恢复和重启测试。
平台不按环境切换上述行为，也不增加兼容分支。
历史来源冲突和失败记录保留；后续验收采用本节已接受的规则。

官方文档另已说明 exec stream 的类型与运行值差异。
固定 types 将 `stderr` 声明为 `ReadableStream \| null`。
文档明确 combined／PTY 的 `stderr` 为 `undefined`，并提示类型仍声明为 `null`。
匹配 workerd 的 Optional 缺省行为与托管 PTY 观察均为 `undefined`。
平台保留固定 types 和上游运行行为，不增加 `null` 转换。
复核证据为 `official-stream-discrepancy-reconciliation/review.json`。
这项官方说明不替代当前正式 runtime 的成员验收。

托管 `size` 随文件长度和目录变化；单独修改 mode 时，`size` 未变化。
探针未确定完整计量公式。
同日，独立探针再次提交 `directorySnapshots` 的 mount-only 输入。
托管 `exec()` 与 `monitor()` 均拒绝，错误为 `The container has not been started`。
`inspect()` 返回 null，`running` 为 false；资源已删除，最终库存未变。
固定 types 接受该输入，但探针没有证明成功挂载语义，成员资格继续保持 `blocked`。

2026-10-13，新探针在 mount-only 请求前后分别启动普通容器。
两个对照均通过 ready、`inspect()` 和 `destroy()` 检查，镜像 reference 相同。
中间的 mount-only 输入仍未启动；`exec()` 与 `monitor()` 拒绝，`inspect()` 为 null。
本次对照排除了同一轮镜像或后端普遍无法启动的情况。
结果只适用于本次账号、部署和输入，不证明所有合法目录输入均不可用。

2026-10-13，目录输入调查检查了固定 cf `1.0.0-beta.12`、已安装 SDK 和公开管理 API。
六项 cf help/search 均成功退出；已查命令、路由和文档未提供目录快照的 producer、list 或 reference。
调查未取得可供 stable startup 恢复的合法目录句柄；九个目录成员继续保持 `blocked`。
该结论只覆盖已查公开来源，不证明私有 API 或账号开关不存在。
调查记录为 `.temp/p23-full64/directory-stable-input-investigation/report.json`。

2026-10-20（UTC），只读复核补充了上游变更依据。
上游于 2026-09-01（UTC）合入 [PR #7123](https://github.com/cloudflare/workerd/pull/7123)。
该改动将 `snapshotDirectory()` 放回实验开关，并从稳定声明中移除该方法。
固定 R21 的上游基线包含该改动；稳定声明仍保留九个目录快照和恢复成员。
该来源不替代合法稳定句柄和正向恢复验收。

2026-10-12，启动环境变量探针确认非空名称可包含 ASCII 字母、数字、下划线、点和连字符。
257 字符名称启动成功；Unicode 和空名称被 `monitor()` 拒绝。
探针从 `/proc/1/environ` 核对指定的测试变量，确认它们只进入主进程环境。
未传 `exec()` 的 `env` 时，exec 进程没有继承这些启动变量。
本地 startup 与 exec 共用名称校验；这批托管结果尚未单独验证 exec 的名称边界。

数值信号 `34` 的两批托管请求均在 60 秒后超时。
第二批增加了 trap 就绪屏障，但仍未返回可核对的终态；原因未确定。
这些请求不构成该信号的兼容性证明。
两批 Worker、application 和 namespace 资源均已清理，最终库存与各自测试前一致。

后续分阶段探针确认 `/bin/bash` 与 `/usr/bin/bash` 指向同一文件。
Bash exec 向自身发送 `34` 后触发 trap，并返回退出码 `23`。
主进程信号 `10` 的准备、发送和观察阶段均返回，`monitor()` 保留退出码 `23`。
主进程信号 `34` 在准备阶段返回托管错误 `Worker not found.`，未执行发送阶段。
此结果仍不证明主进程信号 `34` 的行为。
本次测试已删除 application 和 Worker；独立只读复核确认三类资源库存与测试前一致。
原始测试和清理读取失败记录保留，后续复核不把原测试改为通过。

后续单对象探针的官方 cf build 与 deploy 均成功退出。
workers.dev 在 30 秒就绪窗口内持续返回 `404`，探针未启动容器或发送信号。
本次 Worker、application 和 namespace 库存已恢复为测试前状态。
该失败只证明探针未就绪，不用于判断信号 `34` 的行为。

下一次独立探针完成了信号 `34` 的准备、发送和观察阶段。
主进程的 trap 已就绪；`/proc/1/status` 确认信号未屏蔽且已注册 handler。
同一容器内的 Bash exec 向自身发送 `34` 后返回退出码 `23`。
`container.signal(34)` 返回，但两次各 2 秒的观察仍显示主进程运行、monitor 未完成、marker 为 `ready`。
探针没有观察到 trap 退出；有限观察窗口不能证明信号永久被忽略，原因和兼容性仍未验证。
tenant stop 请求返回 `404`，不能作为停止成功的证明。
后续 application 与 Worker 删除均返回 `200`；三类库存与测试前一致，临时 secret 文件已删除。

2026-10-13，新探针从 exec 直接向已准备的 PID 1 发送信号 `34`。
主进程触发 trap，`monitor()` 拒绝并保留退出码 `23`，`running` 变为 false。
另一个对象只调用一次 `container.signal(34)`。
六次观察共耗时 12.225 秒，主进程仍运行，monitor 未完成，marker 仍为 `ready`。
15 秒是观察预算上界，不是实际观察时长。
该对照证明 PID 1 可接收信号 `34`，不证明 API 调用最终会如何结束。
API 信号 `34` 的兼容性继续保持 `blocked`；本地保留经过范围校验的数值信号。
本次 application 与 Worker 已删除；三类库存恢复，临时 secret 文件已删除。

## Exec 取消语义

`exec()` 的 signal 是否已取消，决定两种不同结果。
如果 signal 在调用前已取消，`exec()` 抛出 `AbortError`，不创建进程。
如果 signal 在 `exec()` 返回后取消，runtime 发送 `SIGKILL`。
`output()` 仍收集 stdout、stderr 和退出码；被 `SIGKILL` 终止的进程返回退出码 `137`。
调用方不应把 `output()` 未抛错当作取消失败，也不应把任意异常当作终止证明。

该合同来自固定 workerd upstream `d99bc6b777e35d72d71c2f1fe2fd1db53284528a`：
[`container.c++` 的 abort action](https://github.com/cloudflare/workerd/blob/d99bc6b777e35d72d71c2f1fe2fd1db53284528a/src/workerd/api/container.c++#L147-L162)
只发送 signal；[`output()`](https://github.com/cloudflare/workerd/blob/d99bc6b777e35d72d71c2f1fe2fd1db53284528a/src/workerd/api/container.c++#L247-L259)
返回 `ExecOutput`。
[原生回归第 13、14 项](https://github.com/cloudflare/workerd/blob/d99bc6b777e35d72d71c2f1fe2fd1db53284528a/src/workerd/server/tests/container-client/test.js#L343-L366)
分别断言调用前的 `AbortError` 和启动后的退出码 `137`。
正式 R9 `a8e3d0bd40dff4c3457d0eb75acd000601f4b7b2` 保留上述实现。
本项目的真实 tenant 回归使用 compatibility date `2026-09-08`。
成员资格仍以上述 capability manifest 和最终产品验收为准。

全量验收还须证明 exec 不依赖应用镜像中的 shell 或可写目录。
exec 的退出码应来自目标进程；helper 意外退出不能证明目标进程已经退出。
目标进程退出后，后代进程仍可继续输出。
`exitCode` 应在目标进程退出后完成；`output()` 应等待输出流结束。
新增 helper 的源码、正式 runtime pin、Provider 资格和产品回归须使用同一组输入。
R21 当前输入已完成 helper、Provider 资格与 P23 真实产品回归。
该轮尚未完成 stable API 成员验收；当时输入和原始报告见 [P23 结果](../implemented/p23-cloudflare-containers.md)。

2026-10-12，维护中的 P23 场景新增独立对象的 exec 生命周期回归。
回归核对真实子进程 PID，并在子进程退出后才释放后代进程的输出屏障。
另一回归在收到真实退出码后终止 helper，要求 `output()` 拒绝且保留退出码。
回归还删除该测试容器中的 `/bin/sh`，再以 `65534:65534` 在不可写目录执行程序。
该输入使用官方文档要求的完整 `uid:gid`，回归同时核对 UID 和 GID。
最后，回归要求 `destroy()` 中断仍活跃的 exec 输出，并核对 Broker 的正向清理证明。
该批当时通过两个 strict TypeScript 检查和官方 cf build；当时的真实产品仍待正式 R15 输入。
后续 R21 产品结果见 [P23 结果](../implemented/p23-cloudflare-containers.md)。

P23 场景还包含真实 HTTPS 和 WebSocket 断言。
HTTPS 客户端使用注入 CA，并验证主机名、请求、响应和 EOF。
WebSocket 客户端通过 `getTcpPort().fetch()` 验证升级、正文和关闭握手。
PTY 回归用 stdin 与 `SIGWINCH` 握手，读取 resize 前后的真实尺寸。
回归还核对合并后的 stderr 和真实退出码。
named images 回归使用两个已准入的镜像，并核对 Version 声明、getter 副本和 daemon 重启后的 map。
该回归核对显式 image 的实际输出，以及默认 Lite、自定义资源和字符串 Lite 的资源证明。
自定义资源为 `0.03125` vCPU、`128` MiB 内存和 `1000` MB 磁盘。
三项数值都不同于 Lite，回归逐项核对 SQLite 和 Engine 的资源配置。
这些数值仅为本机资源映射的回归输入。
2026-10-20（UTC）复核的 [Cloudflare 托管限制](https://developers.cloudflare.com/containers/platform/limits/)要求自定义规格至少使用 1 vCPU。
同一文档要求每 vCPU 至少配置 3 GiB 内存。
本机夹具的成功不证明同一组数值可用于托管环境。
2026-10-21（UTC）的五次托管对照确认了有限差异。
前后两次 `1` vCPU、`3072` MiB、`4000` MB 均启动成功。
本机组合、仅降低 CPU 至 `0.03125`、仅降低内存至 `128` MiB 均返回 readiness `Error`。
另外两项资源在单字段对照中保留前后成功值。
这些结果不确定具体校验谓词、完整下限或实际分配量。
本机资源映射的成功不证明相同输入可用于托管环境。

独立 DO 回归验证主进程数值信号 `10` 和 `34`、monitor 退出码、destroy 原因身份和 exec kill。
快照回归直接读取持久化 artifact，核对字节数与 SHA-256。
daemon 重启后，回归再次读取并核对同一 artifact。
两次恢复后，回归复用 verified cache，扫描 OCI 的唯一 rootfs 层。
回归拒绝 exec helper 文件、symlink、hardlink 和指向 helper 的 hardlink。
该检查不另存 rootfs 或复制完整 artifact 到内存。
上述已维护的产品断言已通过 R21 输入的 Linux ARM64 整轮验证。
macOS ARM64 的当前两项 P23 产品用例也已通过。
这些结果不代表完整 65 成员资格通过。

## 日志设置

application 的 `observability.logs` 保存 cf 请求的设置；GET 返回该请求意图。
基础 13 family 没有 application 主进程日志读取操作，此次未实现 stdout/stderr 采集。
`logs.enabled = true` 不能作为采集生效的证据，日志能力保持 `blocked`。

## Owner 与出站回调

standalone Container client 在 shutdown 时撤销其独占的出站映射。
私有 client 与 Owner 共享映射；client 退出、替换或凭据旋转只取消自身的 callback pipe。
Owner 或 attachment grant 撤销时，平台取消其全部回调并清空共享映射。
旧 client 的退出不应删除当前 Owner 的映射；native Owner 退出也不代表 Engine process 已终止。
execution 的终止与容量释放仍由 SQLite authority 和 Broker 的正向清理证明决定。

当前原生候选为每个 tenant 异步任务和原生 Container 操作分配独立的活跃期。
内部自动 monitor 只在准入期间保活，不延长 tenant 活跃期。
每个活跃期在任务结束或启动满 15 分钟时结束；后启动的任务可延长 Owner 的活跃期。
活跃期到期只释放 Owner 引用，不完成或取消 tenant promise。
`IoContext` 将 DO 的 pending I/O、定时器和显式 `waitUntil()` 转入同一任务登记入口。
来源为官方 [Durable Object 生命周期](https://developers.cloudflare.com/durable-objects/concepts/durable-object-lifecycle/)，
于 2026-10-12 核对。

当前候选在 Owner 不活跃后启动 inactivity timer。
`setInactivityTimeout()` 接受大于 0 且不超过 6 小时的毫秒值。
DO 重建后，旧 timeout 配置失效；构造函数可重新设置 timeout。
未设置 timeout 时，本地候选采用现有 Actor 的 10 秒空闲间隔，不声明 CF 的默认时长等于 10 秒。
timer 到期后，原生控制事件先登记 stop intent，再等待 Broker 的正向清理证明。
未知停止结果仍占用容量。
该轮原生专项回归已通过；当时尚未完成正式 pin 和完整产品验收。

2026-10-12，托管探针分别设置 1 秒和 120 秒 timeout。
客户端连续 90.001 秒不向 Worker 发请求，也不调用 `ctx.abort()`。
此后，1 秒容器返回 `running = false` 和 `inspect() = null`；120 秒对照容器仍运行。
探针未测定准确停止时刻，也不证明本地行为相同。
探针已删除临时 Worker 和 application；三项库存与测试前相同。
本地新回归复现了自动 monitor 延迟 inactivity stop 的问题。
R12 源码修复此问题；9 项原生 Owner 回归通过。
真实 P23 场景另用独立的原生 DO 验证同一静默窗口和正向清理。
该场景尚未执行，完整成员资格保持 `blocked`。

host 成功启动新的 private execution 后，才重建 tenant Actor 及其子 facet，并发送原生启动通知。
该重建保留 Durable Object 的 SQLite storage，让新 Actor 的构造函数按实际 running 状态注册出站拦截。
启动失败或 execution 已在运行时，host 不执行该重建。
固定 `@cloudflare/containers@0.3.7` 的构造函数在 `container.running` 为 true 时注册拦截，
来源为官方 [Containers SDK](https://github.com/cloudflare/containers)，于 2026-10-11 核对固定 package。

## 恢复与容量

SQLite 记录 effective image、resources、startup ciphertext 和单调 lease generation。
每个 lease 应包含绑定 instance、object owner 和 provider key 的 AEAD startup envelope。
缺失或空 envelope 被 SQLite 拒绝；损坏或上下文不匹配的 envelope 在恢复时拒绝，不回退为空启动配置，也不释放容量。
code-only deploy 保留已有执行的 effective target；image 或 resource rollout 使用新的 target。
startup env、entrypoint 和 label 不进入 image artifact、公开 GET、日志或诊断。

`admitting`、`starting`、`running`、`stopping` 和 `lost` 都占用容量。
provider timeout、未知 create/start 或 daemon restart 不释放对象身份，也不自动创建替代执行。
平台只有在确认进程终止、资源清理和迟到操作 fencing 后，才把 lease 标为 `stopped` 并释放容量。

Broker 与 daemon 重启后，平台从冻结的 lease 恢复清理权限。
Broker 核对 owner、镜像、resources、startup 和现存 Engine 对象；该登记只允许终止，不创建执行或颁发新凭据。
maintenance 继续处理持久停止意图，无需新的 tenant 请求。
清理失败保留 `lost` 与容量；同一 Broker 上的重试继续使用原 lease，并要求正向清理证明。

当前 application API 只接受 `rollout_active_grace_period = 0`，拒绝非零值。
rollout 将 grace 0 传给 Broker；停止意图持久化，重启后继续同一 execution。
平台确认旧 execution 已终止、清理和迟到操作 fencing 后，才保留新的替代 lease。

rolling rollout 请求在 `steps` 与 `step_percentage` 中选一项，同时提供或同时省略均拒绝。
`step_percentage` 只接受 5、10、20、25、50、100；平台将每步增量规范为累计百分比，终点为 100。
例如 25 产生 `[25, 50, 75, 100]`，100 产生 `[100]`。
固定 `cf@1.0.0-beta.12` 的 deploy producer 在 immediate 模式发送 `step_percentage: 100`。
字段语义见 [Rollout create API](https://developers.cloudflare.com/api/resources/containers/subresources/applications/subresources/rollouts/methods/create/)，于 2026-10-11 核对。
规范后的步骤仍由同一事务匹配 live target、冻结 cohort；不增加 manual 或 new_instances rollout。

当前本地控制面在 `running` rollout 期间拒绝发布新 target，返回 HTTP 409。
配置内容与物理 spec 均相同的 code-only 更新可复用 target；metadata-only 更新继续。
平台不自动取消 rollout，也不改写 frozen target、cohort 或历史。
该限制是本地控制面合同，不声明托管 Cloudflare 的并发 rollout parity。

应用容器可以访问宿主允许的 IP 网络。单机部署不承诺阻止 provider 网络直连容器，也不承诺每个容器独立 VM。
平台控制 API、原生回调和 engine socket 仍各自验证权限；出站 IP capability 继续拒绝宿主 Unix socket。

## 公开管理面与 SDK

Container 管理路由由 `openapi/cloudflare-subset-manifest.json` 的 `clientObservedOperations` 登记。
生成器将固定 cf 使用的 13 条路由投影到 `ocd capabilities --json`，并标注 `OC-CONTAINERS-MGMT-001`。
路由包含 application、账号上限、镜像准备、registry credential、rollout 与实例观察。
这些观察路由不属于固定上游 OpenAPI 子集；不能据此声称完整 Cloudflare 管理面兼容。
固定 `cloudflare@7.2.0` SDK 没有对应方法，生成的 facade 不提供 Container 管理方法。
`packages/sdk/surface.json` 登记排除项；应用使用固定 cf 部署流程。
Dashboard 尚无独立的 Container 管理页。65 项 tenant runtime API 与上述管理面分别验收。
