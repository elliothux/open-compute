# workerd 上游能力与待补缺口

核验日期：2026-09-18。当前源码基线为
`679c09e5eea0af8a04062e1875e99c75af532e3b`，位于 [`third_party/workerd/`](../../third_party/workerd/)。
下表所有已合并 PR 的 merge commit 均已确认是该 checkout 的祖先；**已合并不等于 standalone 已执行完整合同**。
issue/PR 状态首次核验于 2026-09-05；2026-09-18 又对最新 upstream source 和 fork-ahead diff 做了逐能力复核。
W1 fork 的新增实现与运行证据见[实施记录](../implemented/w1-dynamic-workers-worker-loader.md)；下表不把 fork 改动算作上游已合并能力。

## 2026-09-18 fork-ahead 审计

最新 upstream 未出现 `HostExtension`、`host-extension-fd`、`StandaloneResourceLimits`、
`StandaloneIsolateLimits` 或 `DynamicWorkerLimiter` 实现。旧 formal fork 到当前 upstream 之间涉及相同上游文件的
改动是 container shutdown、actor map ownership、facet UAF、TCP/UDP socket、coroutine-hostile RAII lint 与 client address 等
独立能力；已直接采用 upstream 版本，没有保留旧文件快照、兼容分支或重复实现。

当前 fork 相对 upstream 只保留四个可独立构建的提交：W1 delegated Loader namespace/invocation accounting、W2 standalone
Standard limits、W3 native host bindings，以及手动四平台 binary workflow。W1 继续复用 upstream 原生 `WorkerLoader`、
`WorkerStub`、module validation 与 RPC 生命周期；W2 继续复用 upstream `ResourceLimits` API，只补 standalone 执行；W3
只在 trusted Loader/server seam 接入私有 Factory/Port 和 inherited broker FD，业务 Provider 留在 fork 外。未发现已被
upstream 完整覆盖而仍应保留的 fork 实现。

本次 upstream 还加入 UDP/datagram 与 `Socket.protocol`，随后由 `4a9561ac7` 限定到
`workerdExperimental`，并由 `981731730` 从 non-experimental types snapshot 移除。open-compute 的固定 stable
types、capability inventory 和公开 Dynamic Worker 环境均不因此扩面；W3 的内部 `--experimental` 进程开关只供受信任
system Worker/fork binding 使用，不能作为 tenant UDP 支持声明。
上游随后只用 `679c09e5e` 发布 `2026-09-18`，改动 release version 与 maximum compatibility date 文件；fork 在该
release commit 上重放相同四个能力提交，没有引入额外冲突或兼容分支。

## Limits

| 上游讨论                                                                                           | 核验状态与实际内容                                                                              | 对 W2 的约束                                                                                      |
| -------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| [#49 CPU/Memory limits](https://github.com/cloudflare/workerd/issues/49)                           | closed / not planned；维护者说明生产限制依赖未开放的、平台相关的 Linux 设施，并建议外部 sandbox | 不能把 OSS 接口当作生产执行器；这是历史立场，不能推断未来永不接受。完整执行器按长期 fork 维护预算 |
| [#1627 Configurable local v8 heap limits](https://github.com/cloudflare/workerd/pull/1627)         | 已关闭、未合并；面向本地调试的 heap limit / snapshot 原型                                       | 可参考 V8 接线，不能直接当成租户 OOM 隔离方案或用其行数估算完整补丁                               |
| [#6399 Custom limits for dynamic workers](https://github.com/cloudflare/workerd/pull/6399)         | 已合并，`8765a37c37f8`；新增 ResourceLimits 类型及 source/channel 参数传递                      | 复用已有 JS API 与参数；standalone 仍须消费预算并执行，不能再做一套 limits API                    |
| [#6894 Cross-worker fetch native memory growth](https://github.com/cloudflare/workerd/issues/6894) | open；报告接收 isolate 包装对象、GC 与 Linux allocator 导致的 native 内存增长                   | 上游报告尚不是本项目复现；增加有界跨 Worker 请求回归，区分 heap、native live allocation 与 RSS    |

[#1627 的评审](https://github.com/cloudflare/workerd/pull/1627#discussion_r1481679101)指出，
`TerminateExecution()` 不会自动驱逐并重建 isolate。W2 必须设计中止、在途请求结算、缓存摘除和新 isolate 恢复，
不能以整个 workerd 退出完成单租户限额。其[配置评审](https://github.com/cloudflare/workerd/pull/1627#discussion_r1481676282)
也支持保持配置面小：避免把 V8 generation、倍率等内部调优项变成公共持久化合同。

upstream 基线的 null enforcer 和 `WorkerStubImpl::getEntrypointResolved()` / `getActorClassResolved()`
未执行收到的 limits。W1 fork 的公开 Loader 原生拒绝所有显式 limits（包括空对象），
当时默认 CPU/内存/subrequest enforcement 尚未实现。
W2 已补齐 invocation/isolate 执行、宿主接线、公开配置/API 与产品资格；预算定义和证据见
[W2 实施记录](../implemented/w2-standard-limits.md)。

## Loader

| 上游 PR                                                                                                                                                                                    | 已合并基线           | 可直接复用的范围与边界                                                                          |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------- | ----------------------------------------------------------------------------------------------- |
| [#4383 Dynamic worker loading](https://github.com/cloudflare/workerd/pull/4383)                                                                                                            | `20eb99f1cef1`       | 原生 get、WorkerCode、entrypoint/actor class；并不创建独立执行线程                              |
| [#4579 Production loader interfaces](https://github.com/cloudflare/workerd/pull/4579)                                                                                                      | `fac362eff344`       | 生产接入所需接口调整；CF 生产宿主实现与 standalone 不同，不能据此声称 OSS 含完整生产实现        |
| [#4834 Service bindings in dynamic env](https://github.com/cloudflare/workerd/pull/4834)                                                                                                   | `7483d40e90ab`       | ctx.exports entrypoint 与 props 中 binding 的传递；不代表 Loader 对象可传递                     |
| [#5693 Fetcher over RPC](https://github.com/cloudflare/workerd/pull/5693)                                                                                                                  | `8f3a2c11e6dd`       | Fetcher 与 DurableObjectClass 的 channel 序列化；不能推导所有 DO instance stub 或对象均支持     |
| [#6316 load()](https://github.com/cloudflare/workerd/pull/6316)                                                                                                                            | `87395c3a68df`       | 原生 one-off load，复用 get(null, callback) 路径及引用计数 source                               |
| [#6553 Dynamic loader UAF fix](https://github.com/cloudflare/workerd/pull/6553)                                                                                                            | `696113ef503e`       | 保留请求 channel 和 actor startup 所需强引用；新 delegation/eviction 必须维持这些生命周期       |
| [#6822 Upstream changes](https://github.com/cloudflare/workerd/pull/6822) 中的 [RpcStub env commit](https://github.com/cloudflare/workerd/commit/b720d551a5f4a761bb0da91f4beb766f90d473f0) | merge `7d71003a12a2` | persistent RpcStub 的动态 env channel 支持；不等于所有 transient RPC 对象均可传递               |
| [#6997 WebAssembly.Module](https://github.com/cloudflare/workerd/pull/6997)                                                                                                                | `9cdf38052e16`       | 直接 Module 和 {wasm: Module}，共享编译结果，覆盖新旧 module registry；避免代理序列化后重新编译 |

upstream 基线的 WorkerLoader 缺少可转移的 JSG capability，动态 env rewrite 没有 Loader channel；
2026-09-05 的公开检索未找到补齐该链路的上游 PR。W1 fork 已通过受约束的一次 env 委派补齐这一能力，
并加入 namespace、缓存、in-flight、collector 与 private host-facet 接线；未放宽成任意 RPC 转移。

[#5681 WorkerCode validation](https://github.com/cloudflare/workerd/issues/5681) 仍 open：未知 dictionary 字段通常被忽略，
类型检查负责提示拼写错误。W1 应保留原生语义，不能把管理面严格 metadata 校验移植到 WorkerCode。
module 的“恰好一种类型”则由已知 type 字段计数实现；未知附加键不等于第二种类型。

## 补丁与升级审查边界

- **复用**上述已合并实现，不重复 cherry-pick、不新增 JS facade、RPC 对象全集代理或第二套 module loader。
- **新增**原生 Loader capability 的受约束委派、namespace 接线及 limits 执行器；具体所有权与退出条件见
  [W1 原生实施方案](../implemented/w1-native-limits-loader.md)，API 与安全验收见
  [W1 结果](../implemented/w1-dynamic-workers-worker-loader.md)。
- **保留**上游 GC/UAF 强引用关系；评审新缓存驱逐与 OOM 路径是否会提前释放 service、channel 或 IoContext。
- **核验**每次 pin 升级的 compatibility date/flags、transfer 规则与 module registry；PR 当年的 experimental flags
  不是当前支持承诺。普通 RPC 可传递、动态 env 可接收、动态 entrypoint 可转移要分别验证。
- **贡献**优先讨论通用 Loader 委派与最小执行器接口，产品预算/权限留在 open-compute。#49/#1627 不支持承诺
  完整执行器一定会被上游接收；先交付 fork，按独立组件维护补丁，已被上游取代的实现随协调升级删除。

验收需同时覆盖单租户超限后的邻居可用性、有界 native 内存增长、GC 与异步启动安全。
CPU watchdog、完整内存计量和跨平台恢复尚待实现与实测，不能用本次源码审查宣称 100% Cloudflare 兼容。
