# W4：workerd fork 收薄

状态：**planned**。2026-10-04 完成源码差异与调用方复核，尚未实施收薄，也未完成替代路径的运行时资格化。

目标：让 fork 只承担上游 standalone 缺少的执行器和宿主接入能力。Python、原生 binding、RPC、Queue、DO 的公开行为尽量由上游实现；open-compute 的部署身份、授权、路由、持久化与私有协议由平台拥有。衡量收薄既看 fork 差异，也看平台与 fork 的合计复杂度，不能通过把同一套模拟实现搬到 TypeScript 来达标。

本方案覆盖整个 fork，包括 W1、W2、W3、R3 和 P21 候选改动。历史验收说明当时行为通过，不自动证明每个补丁都不可替代。P21 的产品交付仍由 [P21 Python Workers](../implemented/p21-python-workers.md) 拥有；W4 负责重新约束其原生实现边界。

## 1. 审查输入与结论边界

| 输入                     | 本次固定值                                                                                      |
| ------------------------ | ----------------------------------------------------------------------------------------------- |
| fork 源码                | `e98a3e8433979356047a202d3e0d1b0e2e2c4b8c`，审查时子仓库 clean                                  |
| 已合入的 upstream base   | `d99bc6b777e35d72d71c2f1fe2fd1db53284528a`                                                      |
| 只读核对的 upstream main | `92e6468fc4ec8f22a70cb55f93dd9c850de3e584`，2026-10-04；比上述 base 多 29 个提交                |
| 审查时 formal pin        | `v1.20260930.0-open-compute-r3.e3bdb07f5`，revision `e3bdb07f52affc6a618f02ed2b731a581b0b2f69`  |
| 平台调用方               | 审查时 open-compute 工作树中的 `packages/runtime/`、宿主实现和活动 P21 方案；包含尚未提交的修改 |

formal pin 的唯一实时 authority 是 [workerd.lock.json](../../packages/runtime/workerd.lock.json)。以上是本次审查输入，不是让后续开发回退到这些版本的指令，也不是新的发行 pin。

完整差异为 **117 个文件，新增 10,433 行、删除 911 行**。按文件路径划分：

| 类别                         | 文件数 | 新增 / 删除 |
| ---------------------------- | -----: | ----------: |
| 生产代码、头文件及协议       |     62 | 5,141 / 313 |
| 测试、fixture、测试 provider |     46 |   4,918 / 6 |
| 构建、CI、README             |      9 |   374 / 592 |

统计取 `git -C third_party/workerd diff --numstat <base> <fork>`：`/tests/`、`-test.`、`test-fixture`、`host-extension-test-provider` 归入测试，`.github/`、`BUILD.bazel`、`README.md` 归入构建文档，其余归入生产。此口径不逐行拆分内嵌测试。不能把全部新增行都当作运行时实现，也不能删回归测试来制造收薄数字。

本次依据是上述两份源码、当前调用方、所属测试源码和官方资料。没有重新构建 binary、下载运行时、部署远端资源或执行产品 Gate。最新 upstream 的 [29 个提交差异](https://github.com/cloudflare/workerd/compare/d99bc6b777e35d72d71c2f1fe2fd1db53284528a...92e6468fc4ec8f22a70cb55f93dd9c850de3e584) 没有补齐本方案关注的 dynamic Python dedicated snapshot 宿主入口、standalone limits 或 Loader delegation；其中 DO snapshot 更新不能当作 Python interpreter snapshot 支持的证据。

## 2. 首先纠正能力判断

**workerd 上游本来就支持 Python，也已经实现 Python 快照和原生 bindings。** Cloudflare 的 [Python 执行说明](https://developers.cloudflare.com/workers/languages/python/how-python-workers-work/) 描述了 Pyodide 与快照机制。上游的 `ArtifactBundler_State` 已有 `existingSnapshot`、`storedSnapshot`、validation 状态；上游 Worker Loader 也已支持 Python 模块。P21 不能再以“上游没有 Python／没有快照／没有原生 binding”为整组补丁的理由。

真正需要分别判断的是：上游能力是否已经向 **standalone + 动态加载 + 可信宿主** 暴露了我们所需的接口。

- [上游 ArtifactBundler](https://github.com/cloudflare/workerd/blob/d99bc6b777e35d72d71c2f1fe2fd1db53284528a/src/workerd/api/pyodide/pyodide.h) 明确区分 dynamic worker，并注明尚不支持 dedicated snapshot；standalone server 默认使用 disabled bundler。当前补丁接通的是已有机制的准备、提取和恢复入口。
- 上游 KV、R2、Queue、Fetcher、DO namespace/stub、WrappedBinding 已存在。动态 `env` 接受可传递值，不等于宿主已能用任意 backend fetcher 构造所有这些原生对象。需要补的是受限构造与 channel 接线，不能据此重写其公开方法。
- 上游有 `ResourceLimits` 类型和执行器抽象，但本次 base 的 standalone 使用 `NullIsolateLimitEnforcer`，不能由“已有类型”推导为 CPU／内存限制已经执行。

因此，最薄的正确结果仍可能包含少量 native 接口；每个接口必须说明具体的 standalone 缺口。

## 3. 全部补丁的去留账本

“保留”指当前产品合同下有明确原生职责；“收薄”指保留能力、缩小实现边界；“优先替换”指现状成本明显偏高，必须先验证替代路径再删除；“条件保留”指只在列明的合同或缺口仍成立时保留。以下覆盖生产差异的功能分组，构建与测试随所属能力处理。

| 补丁组                                                                            | 主要落点（通常相对 `third_party/workerd/src/workerd/`，其余注明）                                                       | 决策及理由                                                                                                                                          |
| --------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| W1 Loader delegation、namespace、缓存、撤销与 in-flight 计数                      | `api/worker-loader.*`、`io/io-channels.*`、`io/dynamic-worker-limiter.*`、`server/server.c++`                           | **保留并收薄**。动态 tenant 获得受限 Loader、隔离缓存和已撤销能力不能复活，都是真实边界；平台部署名称格式不是 native 职责。                         |
| W2 CPU、isolate 内存、startup、subrequest、connection limits                      | `server/standalone-*-limits.*`、`server/thread-cpu-clock.*`、`io/limit-enforcer.h`、`io/io-context.*`                   | **保留**。使用既有抽象补 standalone 执行器；同进程失控 isolate 的中断与摘除不能由 JS timer 或平台统计代替。                                         |
| W3 native extension port 与 FD broker                                             | `api/worker-loader.*`、`io/host-extension.capnp`、`io/io-channels.*`、`server/server.c++`、CLI/config                   | **条件保留**。当前 direct-FD provider 合同需要；只有统一改成上游已有传输且证明等价后，才可整组删除。                                                |
| Rust/Tokio capability FD 接收                                                     | `src/rust/cxx/kj-rs-io/{async-io.*,ffi.rs,lib.rs,stream.rs}`（子仓库根目录）                                            | **随 W3 条件保留**。属于当前 Rust binary 的 SCM_RIGHTS 缺口，适合独立上游化；不恢复旧 C++ 主程序或两套 I/O 后端。                                   |
| R3 compatibility catalog                                                          | `io/compatibility-date.*`、Rust CLI、`server/cli-main.*`                                                                | **低优先级保留**。小型只读能力，提供与 binary 一致的 catalog；不能为了少量行数另建更复杂的生成链。                                                  |
| Python dedicated snapshot 准备／恢复                                              | `api/worker-loader.*`、`io/io-channels.h`、`server/server.c++`                                                          | **收薄到 ArtifactBundler 接线**。现有部署准备、离线恢复和 startup 合同需要该入口；Python 引擎和 snapshot 算法继续由上游拥有。                       |
| host-only native binding construction、内部 extension、private env 与 export hook | `api/worker-loader.*`、`api/wrapped-binding.*`、`io/worker.*`、`server/server.c++`、`server/workerd-api.*`              | **保留隔离能力，重审策略入口**。不能让 tenant 经导入 `env` 读到内部 transport；删除 C++ 对具体平台 policy module/export 名称的依赖是目标。          |
| dynamic Cache API backend                                                         | `api/worker-loader.*`、`io/io-channels.h`、`server/server.c++`                                                          | **保留小接口**。上游该位置仍是 cache outbound TODO；只接已有原生 Cache backend，不新增 Cache API 实现或把 backend 放进 tenant env。                 |
| Service／DO 私有 Fetcher 与 RPC policy                                            | `api/open-compute-service.*`、`api/open-compute-durable-object.*`、`api/worker-rpc.*`、`api/http.*`、serialization tags | **优先替换**。平台 JS policy 已侵入原生 stub、promise、属性调用、传递与 dispose；目标是复用原生 RPC，只留下必要的 channel/admission/lifetime 接口。 |
| DO namespace policy 与自定义 ID holder                                            | `api/actor.*`、`api/wrapped-binding.*`、`server/actor-id-impl.c++`                                                      | **优先收薄**。上游已有 namespace 与 ID factory；逐方法转回 JS policy 成本高。jurisdiction 和当前数据身份必须单独处理，不能机械换回 stock factory。  |
| Queue native construction、V8 codec、publication policy                           | `api/queue.*`、`api/wrapped-binding.*`、`server/workerd-api.*`、config/tag                                              | **拆开处理**。保留必要构造和受限 codec；优先删除覆盖原生 send/sendBatch/metrics 的 policy 分支，前提是发布与恢复合同得到等价证明。                  |
| private control quota、waitUntil observer、actor budget top-up                    | `api/http.*`、`api/wrapped-binding.*`、`api/{basics,global-scope,workers-module,actor-state}.*`、`io/io-context.*`      | **随生命周期方案收薄**。控制流量不重复收费、后台任务不提前释放是必要行为；不能把修补私有 RPC 协议的全部 hook 都认作永久原生需求。                   |
| HostFacets grant、logical depth                                                   | `api/worker-loader.*`、`api/actor-state.*`、`io/worker.h`、`server/server.c++`                                          | **保留最小 grant，验证后移除扁平化补偿**。动态 actor class 不能随意跨 RPC；但逻辑深度 override 应与上游 nested facet 修复一起重新判断。             |
| host tails 注入与传播                                                             | `api/worker-loader.*`、`io/{io-channels,io-context,worker-entrypoint}.*`、`api/http.c++`、`server/server.c++`、config   | **条件收薄**。先证明已有 `WorkerCode.tails` 能否承载每次 admission 的正确归因；不能直接把动态 collector 固定成首次加载时的身份。                    |
| Python helper 的 DO waitUntil、queue／scheduled 参数修正                          | `src/pyodide/python-entrypoint-helper.ts`（子仓库根目录）与对应 Python 测试                                             | **按独立 upstream bugfix 保留**。每项需 stock 最小复现和固定 SDK 合同；不修改 SDK 来适配私有 runtime。                                              |
| startup／eviction／取消／退出时的 owner 与内存安全修正                            | `server/server.c++` 及 owning tests                                                                                     | **保留或独立上游化**。包括 header table owner、namespace/cache 回收后的启动任务与旧 stub 生命周期；不能随策略删除一并回滚。                         |
| build、CLI/config/tag 接线与测试                                                  | 各 `BUILD.bazel`、`workerd.capnp`、`worker-interface.capnp`、`server/workerd-api.*`、测试文件                           | **逐所属能力收敛**。删除能力时同批删除失效接线，保留公开行为和安全回归；协议 tag 的调整遵循当前实际持久化合同。                                     |
| fork README、大面积删除上游 workflows、手动发行 workflow                          | 根 README、`.github/workflows/`                                                                                         | **减少无关 diff**。保留必要且显式触发的 fork 构建；减少品牌改写与无关 CI 改动。恢复上游 workflow 前必须确认不会在 fork 自动发布。                   |

早期功能提交为 W1 `540983b18`、W2 `5465cdfd9`、W3 `19046b1d2`、private grants `1c7b89bea`、R3 `2d7bada20`；P21 原生主体为 `0177c203f`，其后 `a692581eb`、`c92952f47`、`e98a3e843` 修正 Python handler 与测试。应按最终行为拆解，不以提交为单位盲目保留或 revert。

## 4. 优先收薄的具体边界

### 4.1 移出可以直接确定归属的平台规则

`server.c++::revokeNamespacePrefix()` 直接要求 prefix 长度为 65 或 82，在第 64／81 位放 `/`，其余位置为小写十六进制。这是 open-compute 部署身份编码。平台的 [namespaces.ts](../../packages/runtime/src/loader/namespaces.ts) 已拥有这套验证，native 应只接收有界 opaque namespace／撤销句柄并执行权限和生命周期约束。

优先删除这类编码知识，并保留撤销单调性、旧引用拒绝新调用、有界内存和活跃请求清理。不能仅清空缓存后允许旧 Loader 再建同名 namespace；也不为替代两个 map 引入通用授权框架。

同类审查包括 C++ 中的固定 `cloudflare-internal:open-compute-host-policy`、`createServiceBinding`、`registerServiceBinding` 和平台专用错误码。内部 module 本身可以由平台维护，但 native 接口不应知道某个平台的部署模型。先消除不必要的策略分支，再决定确实需要的配置入口，不能只改名掩盖耦合。

### 4.2 Service／DO 使用原生 RPC，平台在能力接入处授权

当前 [open-compute-service.c++](../../third_party/workerd/src/workerd/api/open-compute-service.c++) 的 `PolicyOnlyFactory` 拒绝原生 channel，`fetch`、`connect` 和 RPC member 转交 JS policy。反序列化时再查找平台固定 module 重建 policy。DO stub 采用类似路径。

为承载这条路径，[worker-rpc.c++](../../third_party/workerd/src/workerd/api/worker-rpc.c++) 增加 `PrivatePolicy`、`servicePolicy`，扩展 promise 的 then/catch/finally、属性访问、stub 生命周期与 dispose。这已经超出“给现有原生对象接 backend”，是本次最应优先替换的核心补丁。

目标边界：平台用持久 authority 决定身份、权限、不可变部署 pin 和有效期；native 用上游 Fetcher／RPC 对象完成公开调用、序列化、pipeline 和 disposal。先在 `SubrequestChannel`、`ActorChannel`、现有 outgoing factory 等接入处验证最小实现，只补不可由现有接口表达的能力生命周期。

**替代尚未证实。** 上游 dynamic entrypoint／facet stub 存在传递限制，简单把对象返回给 tenant 不能证明权限、callback 或跨请求寿命正确。必须覆盖 callback、返回 capability、pipeline、dup/dispose、源 IoContext 结束、部署切换、撤销和重启；通过后同批移除原生 policy 分支及平台重复 RPC 实现。把现有 RPC Proxy 全部搬回 JS 不算完成。

### 4.3 Queue 复用原生 wire 与 output gate

上游 [queue.c++](https://github.com/cloudflare/workerd/blob/d99bc6b777e35d72d71c2f1fe2fd1db53284528a/src/workerd/api/queue.c++) 的发布路径已经调用 `waitForOutputLocksIfNecessary()`。官方 [DO storage 文档](https://developers.cloudflare.com/durable-objects/api/legacy-kv-storage-api/) 也定义了写入确认前约束输出的行为。

当前新增 `publicationPolicy` 将 send/sendBatch/metrics 转到平台 JS；平台又维护 [publisher.ts](../../packages/runtime/src/queues/publisher.ts) 和 [output-gate.ts](../../packages/runtime/src/durable-objects/output-gate.ts) 中的持久 outbox、事务结果与 flush 状态。同时 [native-adapter.ts](../../packages/runtime/src/queues/native-adapter.ts) 已能处理原生 Queue wire。这条替代路径应先验证。

目标是原生 Queue → 原生 output gate → 平台 backend adapter → 平台持久 Queue。`QueueWireCodec` 仅在 backend 必须解码 V8 wire 时保留，保持尺寸限制、格式校验和 host-only 权限，不扩成通用反序列化入口。

**output gate 不等于跨存储的原子提交，也不等于持久 outbox 或 exactly-once。** 必须分开列明官方顺序要求与平台已经承诺的故障恢复行为，再验证 storage 失败、显式 rollback、crash、重复发送、配额及 metrics。不能因为原生已有 gate 就直接删除 outbox，也不能因为平台已有 outbox 就声称原生缺 gate。

### 4.4 DO ID 和 facets 优先使用已有扩展点

上游 [ActorIdFactoryImpl](https://github.com/cloudflare/workerd/blob/d99bc6b777e35d72d71c2f1fe2fd1db53284528a/src/workerd/server/actor-id-impl.c++) 已实现随机 ID、name 派生、namespace MAC 校验。当前平台 [namespace.ts](../../packages/runtime/src/durable-objects/namespace.ts) 另有 ID 编码和 policy，C++ 再以 `PlatformActorId` 包装并在 namespace 的多个方法转回 JS。

优先验证复用 `ActorIdFactory`／`ActorChannelFactory`，把 namespace authority 与路由接在该边界，避免重写每个公开方法。但 stock factory 对非空 jurisdiction 明确报未实现，不能用它直接替换已声明的行为。当前 ID、存储路径和重启身份也必须作为数据完整性输入处理。Day1 不要求保留旧格式兼容分支；这不授权删除数据、静默转换或修改已发布 migration。

facets 需单独处理。上游已有 facet tree index 和 clone/delete；本次 base 的 `getFacetTreeIndexIfNotEmpty()` 仍要求 root，而 nested clone/delete 会走到它。这个源码条件支持继续调查一个小型上游修复，不支持未经运行验证就宣称 nested 路径已可用。平台的扁平物理命名和 native `logicalDepth` override，应在修复后重新证明是否仍有独立必要性。

保留动态 actor class 到真实 facet manager 的受限 HostFacets grant，直到现有接口能替代。验证真实树的 clone/delete、目标替换、活跃 actor abort、SQLite/WAL、重启和深度约束后，才决定删除扁平化补偿；不能仅把一个 `parent == none` 断言删掉。

## 5. 应保留但必须限制范围的能力

### 5.1 standalone limits 与内存安全

保留原生 thread CPU 计时、V8 中断、isolate 内存限制、request budget、嵌套 limits 收紧、超限 isolate 摘除及正确的恢复行为。平台配置与用户错误映射留在平台；执行器使用 workerd 已有抽象，避免在通用 JSG/RPC 类中增加产品策略。

JS 无法可靠中断同 isolate 中的死循环；进程级 kill 会影响同实例其他 Worker。通过每 Worker 新建独立进程规避 native limits 会改变当前单实例 workerd 模型，不能当作低成本收薄。

独立保留必要 owner 修复，包括 `GlobalContext` 持有 header table、缓存／namespace 回收时启动任务仍有 owner、取消后的计数清理、旧 stub 不误删新对象。actor event 的 budget top-up 若是 W2 执行器所需，继续属于执行器，而非可随 P21 回滚的临时逻辑。

### 5.2 Python 与语言无关 bindings

最小 Python 接口职责限于：可信宿主进入已有 validation 流程、取出上游生成的 dedicated snapshot、加载经过验证的 snapshot、约束大小与准备预算、可靠清理。源代码、包与 runtime 身份、摘要、权限、持久化和准备状态由 open-compute 拥有。继续使用上游 Python entrypoint、SDK 和 snapshot 算法。

dedicated snapshot 是当前部署准备／冷启动设计的要求，不是“Python 能否运行”的定义。如果上游以后开放等价 standalone 接口，删除本地接线补丁。如果打算直接冷启动，必须重新证明 startup 和离线包合同，而不能悄悄放宽限制。

`openComputePrivateEnv`、bindings、snapshot、cache 等 host 参数目前混入公共 `WorkerCode` shape。收薄时应把必要的 host-only 构造集中在可信接口，tenant Loader 继续使用官方 shape；不能靠文档约定阻止 tenant 传入私有参数。宿主 grant、内部 module 可见性、不可二次转授与 `cloudflare:workers` 导入 env 的隔离都必须保留。

KV/R2/Queue 等接回现有 native 对象，D1 等使用上游 internal wrapped binding；不要创建另一套 Python-only facade。已有 export interception 是否仍需要，必须结合原生 RPC 替代结果决定。若保留，应是有边界的宿主 hook，不是 C++ 内置平台策略工厂。

Python helper 的三个小修正应拆成独立、可向上游提交的 bugfix：DO state 的 coroutine waitUntil，WorkerEntrypoint.queue 的 batch 参数，scheduled 的短签名。依据固定 SDK 和最小 stock 复现决定去留，不能由某个框架样例的 PASS 推广为完整 Python 资格。

### 5.3 后台任务、配额与观测

导入的 `cloudflare:workers.waitUntil` 不经过平台的 `ctx.waitUntil` wrapper，因此当前 `withWaitUntilObserver` 有具体接入理由。优先判断能否用原生 invocation 生命周期通知统一拥有后台任务；若仍须 observer，只保留内部、正确 AsyncContext 的小 hook，不让它变成第二套 promise 调度器。

`createPrivateTransport` 与 `admitSubrequest` 用于避免控制协议重复消耗租户预算。随私有 RPC 路径替换重新计算需求；保留时只能由宿主获得，公开操作仍恰好经过应有的原生限额。不能用 unmetered transport 绕开 tenant 配额。

上游 `WorkerCode.tails` 已存在，但当前 [collector.ts](../../packages/runtime/src/observability/collector.ts) 的 `observedEntrypoint()` 按 admission 附带身份，缓存 isolate 不随每次调用重建。固定 deployment collector 加事件身份是否足够，需要验证跨 Service、DO、子 Loader、并发 admission 的归因与隔离。不能把首次调用的身份留在缓存中，也不能让 tenant tail 阻塞平台 collector。只有这些条件满足，才删跨多个核心类型传播 `dynamicWorkerTails` 的补丁。

### 5.4 W3 与 catalog 的取舍

[W3](../implemented/w3-user-extensible-native-bindings.md) 当前选择 native provider direct FD／Cap'n Proto 通道，并要求 `ocd` 负责准入而不代理业务 payload。在这个设计下，HostExtensionPort 与 Tokio capability FD 支持有必要。

可替代方向是上游已有 Service／external server 传输配合平台鉴权与流式处理。但这是 W3 传输方案的统一替换，不是删几个调用点：需证明二进制 unary/stream、取消、背压、失败断开、大小限制、scope 隔离和无令牌泄漏；评估数据路径与维护成本后再选。私有 ABI 本身不创造历史兼容义务，若改就同步 provider 与消费者，不留双协议。没有证明整体更简单前，W3 保持独立、受限的补丁组。

compatibility catalog 暂不优先删除。改成构建期生成只有在降低总复杂度，且仍能证明 schema、源码、binary 和所有目标 catalog 一致时才值得做。不得退化成平台手写 flags allowlist，也不得因收薄丢失兼容日期／flag 的官方行为。

## 6. 实施顺序与删除条件

| 阶段                       | 工作与产物                                                                              | 完成条件                                                                                                                           |
| -------------------------- | --------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| W4.1：确定归属、收紧边界   | 移出 namespace 编码规则；整理必要 host-only 接口和每个补丁的上游依据；隔离独立 bugfix   | 公开 Loader 不扩大权限；撤销、隔离和失败路径覆盖仍在；没有新建兼容分支                                                             |
| W4.2：原生 Service／DO RPC | 用现有 channel/factory 做最小能力与生命周期验证，然后同步替换平台策略路径               | 正确处理权限、target pin、callback、pipeline、capability 传递与释放；移除相应 `PrivatePolicy`／自定义 stub 和重复 JS RPC 逻辑      |
| W4.3：Queue 与 DO          | 验证原生 Queue wire/output gate；拆清 outbox 合同；验证 ActorIdFactory 与 nested facets | 公开顺序、事务失败、恢复、ID/jurisdiction 和存储边界成立；满足条件的 publication policy、namespace 转发、logicalDepth 补偿同批删除 |
| W4.4：生命周期与观测       | 基于前述结果缩小 waitUntil、private control 与 tails 接入                               | 配额不重复／漏计，后台工作不提前释放，缓存命中和跨调用观测不串身份                                                                 |
| W4.5：依赖与发行收敛       | 确认 W3 传输是否值得统一替换；处理 catalog/CI/README 低收益差异；协调正式 pin           | 所有保留补丁都有具体缺口与回归所有权；源码、资产、四目标输入及正式 pin 验收一致                                                    |

各阶段优先使用现有 owning tests 和最小一次性调查。调查产物进入 `.temp/w4-thin-fork/`；只把产品必要回归并入维护中的测试，不再保留一套长期 POC runner。实施中的上游升级需先刷新 base 和差异账本，避免基于旧 revision 判断缺口。

每次替换同时更新生产者、消费者、配置／协议、类型、fixtures、构建与文档。过渡实现可以在开发分支中暂存验证，但最终交付只有一条权威路径，不用版本开关长期保留旧实现。不要用整提交 revert 丢掉同提交中的独立修复。

本方案不以固定删行比例为验收标准。交付时重新统计生产／测试／构建差异，并报告：移除了哪些公共核心类中的策略分支、缩小了哪些宿主接口、平台总实现是否更少、还有哪些补丁不能删除及准确原因。

每个最终保留的补丁都要记录：当前需求、上游 source/issue 依据、现有接口为何不足、最小修改位置、所属回归、上游何种变化允许删除。涉及公开行为时，补齐官方合同、支持的 date/flag 或版本范围与正式 pin 约束；私有宿主接口则明确权限、可传递范围和生命周期。单纯“以前实现过”或“测试依赖它”不是保留依据。

## 7. 验收与未决问题

必须保留的产品不变量：单 daemon／单实例受监督 workerd、离线启动、正式 pin 验证、不可变部署、权限与 secret 边界、SQLite／对象数据完整性、取消和进程崩溃后的可解释恢复。应用构建／部署继续使用当前单轨 cf 入口。

| 需要证明的行为                                                  | 现有测试入口／归属                                                                                                 |
| --------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| Loader 隔离、撤销、缓存并发与回收，native limits 和邻居可用性   | fork `worker-loader-{delegation,limits,private-policy}-test`、standalone limits owning tests；平台 P0.1/P0.2       |
| 原生 binding、RPC lifecycle、DO ID/facets、Queue wire 与 output | fork `worker-loader-service-test`、DO native policy/ID、host facets、queue codec；平台对应 Service/DO/Queue suites |
| Python prepare/restore、包、事件与 import waitUntil             | fork Python prepare/entrypoint tests；P21 主部署链与语言间相同行为回归                                             |
| 退出 owner、FD provider 与监督恢复、tail 身份                   | fork server/host-extension/tails tests；平台 runtime/observability/process suites                                  |

测试名称用于定位已有责任，删除旧实现时可合并或重命名；保留行为覆盖，不冻结实现测试数量。替代路径先通过相关单轮检查，再依 [测试节奏](../references/testing.md) 完成静态检查、覆盖率与源码冻结后的单轮最终 Gate。产品和发行验收必须使用协调更新后、通过正式校验的 binary；stock 只提供上游比较证据，开发 binary 的定向 PASS 不代替正式资格。

未决事项必须在实施时回答，不能在当前文档标为已通过：

1. 原生 channel 能否完整承载现有 Service／DO 的准入、返回 capability 与持久 target pin？具体还缺哪个最小 native hook？
2. 当前 Queue outbox 的每项恢复保证分别来自哪条公开合同或明确的平台承诺？使用原生 gate 后仍需多少平台状态？
3. DO jurisdiction 与身份如何在现有 factory 边界实现？数据格式变化如何显式拒绝不支持状态并保护现存数据？
4. nested facet 的上游局部修复能否消除扁平化模型，还是平台还有独立的所有权需求？
5. native invocation 完成语义和静态 tails 能否覆盖当前 observer／动态 collector 的实际要求？
6. W3 使用上游传输是否降低整体复杂度，并保持当前受支持 provider 行为？

以上均是可继续调查的设计问题，不是外部阻塞。当前仅完成收薄方案，未执行 runtime 改动或发行操作，也未产生新的 runtime／产品验收 PASS。
