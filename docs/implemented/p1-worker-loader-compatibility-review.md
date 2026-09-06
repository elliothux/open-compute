# P1 Worker Loader 兼容审查

2026-09-06，使用 `cf-compatibility-check`。**P1 声明子集通过；完整 Dynamic Workers contract 保持部分 blocked。**
范围为 open-compute 工作区实现与 workerd `b3e1a27840299f493d9425dc4d9972381d02ef23`，
upstream base `dd8133e9b9656fb39f1434247a80aa7a249ee204`。
本地 `origin/HEAD -> origin/main` 的 merge base 为 `1279d285aede8351f1723dfac6adcb696579a150`；
该 base 到 root HEAD 的已提交范围为空。工作区另有其他任务的 WebSocket 修改和 `.vscode/`，
不归为本次 Loader 实现。已发现的 P1 阻断问题均修复并验证；P2/实验控制的六个缺口未伪装成支持。

## 合同与证据

公开声明直接来自 `@cloudflare/workers-types@5.20260830.1`，不手写或缩窄 Loader 接口。
Generated Env 仅组合声明的 binding；私有 factory 类型只供 system Workers 使用。
已比较 upstream `e9dda5963aba7ee4323960db795690ec78fec118` 与 fork base 的
`types/generated-snapshot/index.d.ts`：六个 Loader 声明没有变化。完整差异保留在
`.temp/p1-native/upstream-types-delta.diff`。fork revision 与 npm gitHead 是独立来源身份。

当前官方依据：

- [Loader API](https://developers.cloudflare.com/dynamic-workers/api-reference/)：同步 stub、模块、env、outbound、tails。
  同一缓存 ID 的回调必须提供相同代码，缓存命中不是公共保证。
- [in-flight limits](https://developers.cloudflare.com/dynamic-workers/platform/limits/)：Worker invocation 为 4，
  DO 共享 I/O context 为 10；同一 Dynamic Worker 的并发调用只算一个身份。
- [custom limits](https://developers.cloudflare.com/dynamic-workers/usage/limits/)：代码与入口级限制属于公开合同。
- [observability](https://developers.cloudflare.com/dynamic-workers/usage/observability/)：用户 tails 在执行完成后接收事件。
  Cloudflare 不自动把 child 日志写入 parent Workers Logs；本地受保护 collector 必须明确归为平台能力。

同源 portable fixture 在独立 Cloudflare Worker 与 nested fork 路径的基础 JSON 结果一致。
临时资源已删除并验证 absent，证据位于 `.temp/p1-native/cloudflare/` 与 `.temp/p1-native/portable-native/`。
macOS arm64/x64 优化产物均通过 15 个 delegation、5 个 tails、8 个 limits case；x64 在 Rosetta 2 下执行。
这些结果不替代正式 pin 下的 open-compute 产品路径。

当前 `b3e1a27840299f493d9425dc4d9972381d02ef23` 已生成 macOS ARM64/x64、Linux ARM64/x64
优化 archive 与 build.json，存于 `.temp/p1-native/artifacts/<revision>/<target>/`。其中 macOS x64
和 Linux ARM64 各通过 15 delegation、5 tails、8 limits、3 host-facets，共 31 个原生用例，
并通过 `KJ_CLEAN_SHUTDOWN=1`。证据为 `.temp/p1-native/final-native-darwin-x64/` 与
`.temp/p1-native/final-native-linux-arm64/`。Linux x64 也已构建成功，并通过同一 31 个原生用例与严格退出检查，证据为
`.temp/p1-native/final-native-linux-x64/`。正式 lock 已记录四平台真实摘要与构建输入，
用户进一步要求把四平台二进制放入仓库：现位于 `share/workerd/`，Git LFS OID 与正式 binary SHA 一致。
根 build 以固定 Bun 1.3.14 生成确定性 gzip，正式 archive SHA 已同步；原始构建 archive/build.json
作为实际构建记录保留，不能与后来的规范压缩包混淆。macOS 与 Linux ARM64 的四份规范 gzip 摘要一致。
默认 `prepare-workerd.ts --dest` 已通过真实宿主离线校验，CI 通过 LFS checkout 获取依赖，不要求发布 archive。
Git LFS fsck 通过；尚未向远端推送对象或提交。

runtime 单元测试一轮为 159 pass / 1 fail；唯一失败是既有 connect-lifecycle 测试依赖仓库工作目录。
改为相对测试文件解析路径后，该 case 定向通过。后续新增的动态 facet ID admission 用例也通过。
Clippy 首轮以当前 stock 正式 archive 编译，使用独立 `target/p1-worker-loader`，全 workspace /
all-targets / all-features 无告警；这是源码静态检查，不是 fork 产品验收。Loader 的专用 TS compile fixture
覆盖 module 类型与公开 stub/props/能力字段并已通过，实际 Python、user tail 和 facet 路径已通过正式 pin 的专用产品用例。


## 发现与修正

1. 原 capability 分类把 Loader 放入 Workers for Platforms。新分类使用独立 `dynamic_workers` target，
   新增 25 个 stable members；19 个已取得编译与产品证据，4 个 custom-limit 与 2 个 experimental-control
   成员仍 blocked，分别归明确的 gap；binding 可用不表示整个 API 完全兼容。
2. ordinary Worker 原缓存 key 含 route/observability generation，会在重新 promote 时重建 isolate。
   当前按 immutable Version 与入口模式缓存，每次调用单独注入 collector。DO binding 在 SQLite authority
   验证 active Version 并签发当前 route generation，后续 generation fence 保留。
3. 新产品回归最初为同一 child ID 返回不同 Version 的代码，违反官方回调合同。已改为 immutable child code，
   只改变 parent Version，验证本地 namespace 与生命周期。
4. **动态 DO facet 接入已通过平台定向验收。** 原 descriptor 只接受 loopback metadata，
   因而拒绝 `WorkerStub.getDurableObjectClass("Child")`。现在由 host-only factory 从宿主原生 facets
   生成可撤销的创建能力，只交给可信包装层。包装层先经已认证 manager 注册逻辑路径，再直接交给原生
   接口本地类引用；RPC 只传 `{ native: true, id }`。租户 env 不暴露创建能力，能力禁止再次委派、RPC
   传输与持久化，宿主结束或 generation 撤销后拒绝创建。既有 SHA 物理名称与 SQLite registry 继续拥有
   static/dynamic facet，原生逻辑深度覆盖保证 flattening 后仍受四层限制。
   [官方 facet 合同](https://developers.cloudflare.com/dynamic-workers/usage/durable-object-facets/)
   的动态类、存储保留、clone/delete、静态/动态类切换已通过 native 回归；产品路径已通过正式 pin 的动态类、跨 Version 存储、回滚和重启回归。
5. 独立托管探针确认动态 ActorClass 与 entrypoint 都禁止经 RPC 转移；未通过放宽转移规则解决 facet 接入。
   探针 Worker `oc-p1-facet-446258940c2a` 已删除，Cloudflare API 10007 证明不存在，证据位于
   `.temp/p1-native/cloudflare-facet-transfer/`。
6. 新原生回归暴露 Server 退出时取消延迟清理任务，遗漏动态服务 unlink 的问题。清理改为 promise attachment，
   任务完成与取消均解除服务引用。`KJ_CLEAN_SHUTDOWN=1` 的 native facet、delegation、tails、limits
   均正常退出。新增 facet 在默认日期和 GC pressure/2026-09-05 日期下均通过；宿主结束后拒绝旧能力及
   持久化拒绝另有定向回归。日志见 `.temp/p1-native/host-facets-*.log`，失败诊断不计为验收通过。

## 限制与剩余资格

P1 按活动设计拒绝所有显式 custom limits，包括 `{}`。这与托管端支持该选项存在合同差异；它属于 P2
尚未实现的功能，不能以单机拓扑为由标记 fully supported，也不能静默忽略。非空 streaming tails 是 P1
明确不开放的实验 surface。默认 CPU、内存与 subrequest enforcement 仍属 P2。

Python 与 Wasm 的实际平台路径已通过专用产品用例。固定 workerd 的 `Server::preloadPython()` 会在需要时获取与 release
绑定、经 integrity 校验的 Pyodide bundle；它不是 tenant `globalOutbound` 的网络请求。不能把 ocd 启动
离线契约误写成首次 Python child 加载也离线。产品用例实际执行 `.py` 模块与 RPC，不以先前 native Python 用例通过代替平台证据。

protected collector 不受用户 tail 替换，HTTP/RPC fan-out 已在 native 层验证。local Workers Logs 自动收集
child 日志不应被描述成 Cloudflare 默认管理面行为。新产品 Gate 通过固定 Wrangler/v4/ocd 路径覆盖两个
binding、两个 Script、重新部署、回滚、同 Version promote、child 日志、删除重建与重启，定向产品执行已通过。

| 改变的 surface | 当前判断 | 证据或缺口 |
| --- | --- | --- |
| Public Env 类型 | aligned | 直接组合上游 WorkerLoader；配置和生成类型定向测试 |
| load/get、七类模块、env、null/scoped outbound | aligned（native/portable/product） | 独立 Cloudflare 基础结果与 fork 一致；正式 pin 产品用例通过 |
| namespace、转移、撤销、缓存回收 | aligned（native） | delegation、GC、共享 factory、活跃引用及 stream release 回归 |
| 4/10 distinct in-flight | aligned（native） | limiter 单元、HTTP stream、DO 与 facet 回归 |
| user tails 与 protected collector | aligned（native） | HTTP/RPC/failure fan-out；hosted 日志默认行为的差异如上 |
| explicit custom limits | mismatch，由 P2 负责 | 原生明确拒绝，不宣称资源限制兼容 |
| Dynamic `getDurableObjectClass()` → tenant `ctx.facets` | aligned（native/product） | 私有原生能力；跨 Version、回滚、重启后的同一存储计数通过 |
| Python/Wasm 平台路径 | aligned（product） | 原生模块与 RPC/实例化返回值通过；首次 Python bundle 下载限制如上 |
| Version 回滚、DO route、Script 删除与重启 | aligned（product）；保守删除限制 | 定向用例通过；已执行 Version 在 generation 退出前 DELETE 返回 409，不绕过 background hold |
| 四平台 pin、coverage、最终 workspace | verified | 四平台原生构建与摘要固定；本机 coverage 90.10%，49 targets / 1,148 cases 最终单轮通过 |

Anycast、全球 placement、跨地域复制和 fleet autoscaling 属于 excluded self-host scope；本变更没有宣称这些能力。
本记录随已完成的 P1 实施归档；P2 仍由活动设计拥有。


## 正式 pin 的产品开发轮

`.temp/p1-native/p6-loader-product-08.log`：唯一新增产品 case 通过（107.18 秒），执行前原生 discovery
与 P6 registry 的两个 case 精确匹配。本轮只选择新增 case；尚未替代最终完整 workspace Gate。
覆盖 JS/CJS/text/data/JSON/Python/Wasm、scoped RPC、私网出口拒绝、user tail、动态 facet、两个 binding、
两个 Script、promote/rollback、重启后持久化、拒绝过早删除、退出后删除及同名重建。

本轮修正了两个真实平台问题：

- 原 `ctx.exports` 命名入口直接使用原始 binding 描述。现在私有 native loopback bridge 复用现有
  env/context wrapper，保留 scoped props，隐藏私有能力，已包装的 cache/selected 入口保持原路径。
  提取 `loader/wrappers/loopback.ts` 后，原 runtime.ts 降至 800 行以下。16 个 wrapper 用例通过，
  真实 user tail 调用 DO binding 并持久记录 child 日志接收。
- 自动创建的 DO namespace 以 class 名作为账号内唯一资源名，导致不同 Script 声明同名 class 时 500。
  backing resource 改用稳定资源 ID，class 名仍由 Worker-scoped namespace authority 持有；rename/rollback
  不改资源名。四个 migration 单元用例与同名类的双 Script 产品路径通过。

Code-level limits 的 native stub 同步返回，第一次调用异步拒绝；产品测试按此时序验证，未改为静默接受。
Script DELETE 仍受既有 generation background hold 保护：响应完成不等于后台工作已经退出。产品用例
先验证 409 与继续调用，再显式结束监督器 generation 后删除；没有为测试去掉 retention 或自动重启其它 Worker。

静态/工具结果：Clippy 新 pin 无告警、format、metadata、dependency boundaries、generated 资产校验通过。
完整 JS 测试为 237 pass / 1 fail；失败为 vinext 历史 qualification 的 rootLockSha256 drift。HEAD 与当前
bun.lock 的摘要均为 `4ada2c4f36529bb060a44f1bea92fcd6e51f12569c2673958cd3575cfe112554`，
HEAD 中 vinext 清单仍记录 `c99e951a642e668e1826f56289b02c03abcb3d084675d3cd117205e25b49d3d0`，
证明该 mismatch 先于本次改动。未改写该历史 Go 记录，也未声称 vinext 在新 fork 上重新取得资格。
普通 workspace Gate 按既有策略不包含单独的 framework application qualification。

固定依赖流程另覆盖并发原子生成、缓存复用、LFS pointer、超大文件、符号链接和损坏缓存拒绝，
7 个工具用例通过。未设置 archive 环境变量时的 Clippy、无默认特性和 MSRV 检查通过。
loopback 的负向 native 探针证实 primitive options 和 null/primitive props 必须 TypeError，
包装层已同步该规则，其他选项仍交给 workerd 原生解析；证据在 `.temp/p1-native/loopback-options-probe/`。

## 固定依赖后的验证与修正

固定依赖的完整覆盖率开发轮已通过：49 targets、1,148 cases、90.10% Rust line coverage，
报告为 `.temp/gate-run/20260906T121235-58b11975/report.json`。此前 Gate archive preflight
仍要求旧环境变量，以及 capability 测试假定全部 target 无 blocked 成员的问题均已修正；
失败证据保留，未绕过测试或把未实现成员标为 supported。

补充审查确认原生 loopback 接受 `Object.freeze({props: ...})`，包装层使用原对象作为 Proxy target
会触发非 configurable 属性 invariant。已改为独立代理目标，并避免对命名 service 的 props getter
重复求值。16 个 wrapper 回归通过，真实 Wrangler Loader fixture 也改用冻结的 user-tail options。
这次修正发生在上述 coverage 之后，因此该轮不作为最终冻结源码的完整验收。
英文/中文配置、runtime bindings 和 deviations 已同步，文档站点构建通过。

## 最终验证

最终输入：fork `b3e1a27840299f493d9425dc4d9972381d02ef23`，
formal lock SHA-256 `5f92b595764892c166b36e61a81ef0b5313178554edefbb0b49a01dad7efb303`，
测试源码摘要 `e8d88c8be7c8f5d8c466566da00b9acdecba72b72649411c839e4113bd4d47e6`。
本机 open-compute acceptance 为 macOS ARM64；其他平台的 native workerd 结果单列，不冒充四平台平台 Gate。

| 验证 | 实际结果 | 证据 |
| --- | --- | --- |
| 四平台优化 workerd | 构建完成，binary SHA 与 Git LFS OID / formal lock 一致 | `.temp/p1-native/bundled-artifacts.json`；各 revision/target 的 build.json |
| 原生回归 | delegation / tails / limits / facets、日期与 GC 变体及严格退出通过 | `.temp/p1-native/test-10.log` 至 `test-16.log`、`host-facets-*.log`、`final-native-*/` |
| Canonical gzip | macOS ARM64、Linux ARM64/x64 生成的四份摘要一致 | `.temp/p1-native/bundled-compression-*.log` 的成功轮 |
| 真实 Loader 定向产品用例 | 通过；冻结 tail options、模块、RPC、facet、Version/删除/重启 | `.temp/p1-native/p6-loader-product-09.log` |
| 静态与工具 | build、generated、fmt、Clippy、no-default、MSRV 1.98、metadata、boundaries 通过 | `.temp/p1-native/*bundled-*.log`、`bun-build-26.log` |
| JS / 工具 | CI JS 239 通过；随后修改的 loopback 16 通过；Gate tooling 25 通过；文档站点构建通过 | `js-ci-bundled-01.log`、`loopback-unit-03.log`、`gate-tooling-bundled-02.log`、`docs-build-01.log` |
| Coverage | 49 targets / 1,148 cases；109,949/122,031 lines，90.10% | `.temp/gate-run/20260906T123444-b5c4a625/report.json`；`target/llvm-cov/summary.json` |
| 最终未插桩 Gate | 49 targets / 1,148 cases 全部通过，单轮 | `.temp/gate-run/20260906T125221-613eb524/report.json`；`.temp/p1-native/workspace-final-01.log` |

Coverage 与最终 Gate 使用相同冻结源码和正式 runtime inputs。两轮都完成精确 discovery、用例计数与
清理核对；没有 ignored case 或自动重试。最终报告之后只归档文档、修正引用并更新 conformance 的文档输入摘要，
未修改运行代码、测试或构建输入。

完整 `bun run test:js` 的独立 vinext 历史 qualification 存在本次改动前的 lock 摘要漂移，
未重写历史 Go 记录。普通 workspace/CI 按既有策略不包含该独立应用 qualification。
Linux 特权 egress fixture、四平台 ocd 发行资格、发布包装和远端 Git/LFS 推送没有执行。

维护入口：[正式 pin](../../packages/runtime/workerd.lock.json)、[固定二进制](../../share/workerd/README.md)、
[兼容矩阵](../references/cloudflare-compatibility.md)、[P2 设计](../workerd/p2-workers-standard-limits.md)。
