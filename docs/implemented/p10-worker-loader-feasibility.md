# P10 Worker Loader 可行性复核

日期：2026-09-05。结论：**No-Go；P10 未实现。** 本文只归档一次已结束的上游能力调查，
[P10 设计](../workerd/p1-dynamic-workers-worker-loader.md)与 DW1–DW5 仍未完成。

## 输入与范围

- 仓库 revision：`2c36a0d52108bbf1f85f58f3fa305180057d5b71`；开始时 working tree clean。
- `refs/remotes/origin/HEAD` 指向 `origin/main`，其与 HEAD 的 merge base 就是上述 revision；没有待审查的
  committed branch diff。此次 tracked 改动仅为文档，未变更 Rust、runtime、types、正式 pin 或 capability。
- 正式输入：[workerd.lock.json](../../packages/runtime/workerd.lock.json)，`v1.20260830.1`，
  revision `e9dda5963aba7ee4323960db795690ec78fec118`，effective date `2026-08-30`，
  `@cloudflare/workers-types@5.20260830.1`，Wrangler `4.127.1`。
- Darwin ARM64 binary SHA-256：`60f972c2b208ad6ab9db09f770396d6d9f38b663d91e62b9a1166e93b51d7675`；
  archive SHA-256：`845ee71f74e821a6085ed506361dd57894f5a416af05396efdc9fd43bc8f69fc`。
  两者均实测匹配正式 pin，binary `--version` 输出 `workerd 2026-08-30`。
- 使用现有 `.temp/workerd-fetch/workerd`，未下载或升级 runtime。探测只调用 `workerd test`，无 HTTP listener、
  外部网络请求、账号写入、数据库或用户数据变更。调用间通过静态 fixture service binding 通信。

## 实测

| 行为 | 输入与实际结果 | 判定 |
| --- | --- | --- |
| static native Loader 正向控制 | `load(code)` 同步得到有 `getEntrypoint` 的 stub；fetch 返回 `ok` | primitive 可执行；不是 tenant nested Loader 证据 |
| 原生 Loader 转移 | `load({…code, env:{LOADER:env.LOADER}})` 抛 `DataCloneError`，报告 WorkerLoader 不支持 serialization | 当前 ordinary Worker 无法接收原生 public Loader |
| code-level subrequests | code `limits:{subRequests:1}`；child 串行执行 3 次 service fetch，均成功并返回 `3` | 限额被接受但没有执行 |
| entrypoint-level subrequests | `getEntrypoint(undefined,{limits:{subRequests:1}})`；相同 child 成功执行 3 次 service fetch | 限额被接受但没有执行 |

首次探测在完成前两个观察后，因 fixture 调用 `ctx.exports.Sink()` 未提供所需 Options 对象退出 `1`。
该失败原样保存在
`.temp/p10-feasibility/failed/20260905T130604Z/{probe.js,probe.capnp,stdout.log,stderr.log,report.json}`。
将 fixture 修正为 `ctx.exports.Sink({})` 后，仅执行此前尚未到达的两个限额观察，退出 `0`；证据在
`.temp/p10-feasibility/20260905T130632Z/{probe.js,probe.capnp,stdout.log,stderr.log,report.json}`。
两个目录都保留原始输入和精确 command；没有把首次失败改写为成功。

第二次探测的 PASS 仅表示成功复现“限额无效”，**不表示 Cloudflare 合同通过**。
这些是一次性 upstream feasibility probes，没有加入 recurring Gate，没有恢复已退役的 POC。
最终 workspace Gate 与 coverage 均未运行，P10 产品验收次数为零。

## 固定源码与官方合同

当前 tenant 路径为 [`loader/host.ts`](../../packages/runtime/src/loader/host.ts) 中
`env.LOADER.get(runtimeKey, callback)`，callback 以 `tenantEnv(...)` 构造普通 Worker。
[`config.capnp`](../../packages/runtime/config.capnp) 的 `open-compute` Loader 属于平台内部 namespace。
它是当前普通 Worker 执行引擎，删除它不能实现 P10；直接向 tenant 传递它既不支持 serialization，
也无法提供 P10 要求的 namespace 隔离。

固定 [`worker-loader.h`](../../third_party/workerd/src/workerd/api/worker-loader.h) 的 WorkerLoader 只有 native
channel 和 `get/load`，无 capability serialization；固定
[`server.c++`](../../third_party/workerd/src/workerd/server/server.c++) 的 `WorkerStubImpl::start()` 只处理
subrequest、actor-class 和 RPC capabilities。`getEntrypointResolved()` 与 `getActorClassResolved()`
接受 limits 后未向 resolved channel 传递；standalone `LimitEnforcer::newSubrequest()` 是空实现。
named isolates map 仅展示 abort erase，没有一般 eviction 或 namespace cleanup API；这项是源码缺口，
本次未用负载测试推断 OOM，也未执行 CPU 无限循环。

[官方 API](https://developers.cloudflare.com/dynamic-workers/api-reference/)与固定 generated types 的
`WorkerLoader` / `WorkerStub` 声明要求同步 stub；
[官方 custom limits](https://developers.cloudflare.com/dynamic-workers/usage/limits/)要求超过 code 或
entrypoint 的较小限额时终止 invocation。RPC Promise facade 和接受后忽略 limits 无法满足这些合同。

2026-09-05 查询的[最新官方 release](https://github.com/cloudflare/workerd/releases/tag/v1.20260905.1)
是 `v1.20260905.1`。只读核对其
[WorkerLoader header](https://github.com/cloudflare/workerd/blob/v1.20260905.1/src/workerd/api/worker-loader.h)
和 [server implementation](https://github.com/cloudflare/workerd/blob/v1.20260905.1/src/workerd/server/server.c%2B%2B)
仍看到同样的无 serialization、env capability 分派、limits 丢弃与 named-map erase 路径。
这是源码证据；未下载或执行该新 release，不能将本机旧 pin 的实测冒充新版本实测。

## cf-compatibility-check 复核

使用仓库 [.agents/skills/cf-compatibility-check/SKILL.md](../../.agents/skills/cf-compatibility-check/SKILL.md)，
并读取其指定的合同、维护矩阵、deviation、pin、manifest 和 Workers runtime checklist。
skill 所列 `docs/cloudflare-runtime-compatibility.md` 已归档，本次使用实际的
[归档合同](cloudflare-runtime-compatibility.md)，没有创建旧路径 stub。

此次无生产实现 diff，因此没有由新代码引入的兼容缺陷；下表记录阻止实施的既有前置缺口，
不能解读为全平台通过兼容性审查。

| P10 surface | 状态 | 证据与边界 |
| --- | --- | --- |
| pinned public types | aligned | 直接消费 upstream generated WorkerLoader/WorkerStub declarations；未手写接口 |
| native tenant Loader / 同步 stub | mismatch | static 控制成功；转移至 dynamic env 时 DataCloneError；缺 upstream primitive |
| custom code/entrypoint limits | mismatch | 两个 `subRequests:1` case 各完成 3 次 fetch；不是可豁免的 hosted quota 差异 |
| named cache bounded cleanup | unverified | 源码只有 abort 移除；没有可用的一般清理接口或有界回收证据 |
| 4 distinct in-flight、CPU enforcement、tail fan-out、restart/isolation | unverified | 前置 primitive 未满足，未伪造产品实现与回归 |
| upload fail closed | aligned（源码） | `WorkerUploadBinding` closed enum 不含 `worker_loader`；multipart 解码先于 Version mutation |
| Cloudflare hosted differential | unverified | 本地硬阻塞已确定；未创建测试资源，不涉及已有 Cloudflare 服务 |

## 恢复实施条件

继续遵守 P10 DW-G0：upstream 正式 stock release 必须先提供可隔离 namespace 的原生 Loader delegation、
有效限额与有界缓存生命周期。之后协调更新正式 pin/types/runtime assets，并实施 DW1–DW5、补齐产品回归，
在源码冻结后执行一次最终 Gate。当前不能以 JS facade、私有 fork、全进程重启或 placeholder binding
缩短这条依赖链，也不能把 upstream 发布视为本仓库内可完成的动作。

文档变更采用 `git diff --check` 和新增相对链接检查，不因本次调查重复执行整个 Rust acceptance loop。
