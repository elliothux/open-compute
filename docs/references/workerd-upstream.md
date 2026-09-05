# workerd 上游能力与待补缺口

核验日期：2026-09-05。状态来自 GitHub issue/PR API，源码基线为
`dd8133e9b9656fb39f1434247a80aa7a249ee204`，位于 [`third_party/workerd/`](../../third_party/workerd/)。
下表所有已合并 PR 的 merge commit 均已确认是该 checkout 的祖先；**已合并不等于 standalone 已执行完整合同**。
当前正式运行时 pin 与 fork checkout 不同，见[基线与更新流程](../workerd/README.md)。这里记录源码能力，未新增运行验收。

## Limits

| 上游讨论 | 核验状态与实际内容 | 对 P2 的约束 |
| --- | --- | --- |
| [#49 CPU/Memory limits](https://github.com/cloudflare/workerd/issues/49) | closed / not planned；维护者说明生产限制依赖未开放的、平台相关的 Linux 设施，并建议外部 sandbox | 不能把 OSS 接口当作生产执行器；这是历史立场，不能推断未来永不接受。完整执行器按长期 fork 维护预算 |
| [#1627 Configurable local v8 heap limits](https://github.com/cloudflare/workerd/pull/1627) | 已关闭、未合并；面向本地调试的 heap limit / snapshot 原型 | 可参考 V8 接线，不能直接当成租户 OOM 隔离方案或用其行数估算完整补丁 |
| [#6399 Custom limits for dynamic workers](https://github.com/cloudflare/workerd/pull/6399) | 已合并，`8765a37c37f8`；新增 ResourceLimits 类型及 source/channel 参数传递 | 复用已有 JS API 与参数；standalone 仍须消费预算并执行，不能再做一套 limits API |
| [#6894 Cross-worker fetch native memory growth](https://github.com/cloudflare/workerd/issues/6894) | open；报告接收 isolate 包装对象、GC 与 Linux allocator 导致的 native 内存增长 | 上游报告尚不是本项目复现；增加有界跨 Worker 请求回归，区分 heap、native live allocation 与 RSS |

[#1627 的评审](https://github.com/cloudflare/workerd/pull/1627#discussion_r1481679101)指出，
`TerminateExecution()` 不会自动驱逐并重建 isolate。P2 必须设计中止、在途请求结算、缓存摘除和新 isolate 恢复，
不能以整个 workerd 退出完成单租户限额。其[配置评审](https://github.com/cloudflare/workerd/pull/1627#discussion_r1481676282)
也支持保持配置面小：避免把 V8 generation、倍率等内部调优项变成公共持久化合同。

当前 [`server.c++`](../../third_party/workerd/src/workerd/server/server.c++) 的 null enforcer 和
`WorkerStubImpl::getEntrypointResolved()` / `getActorClassResolved()` 仍未执行收到的 limits。
P2 补 invocation/isolate 执行与宿主接线；预算定义和产品验收归 [P2](../workerd/p2-workers-standard-limits.md)。

## Loader

| 上游 PR | 已合并基线 | 可直接复用的范围与边界 |
| --- | --- | --- |
| [#4383 Dynamic worker loading](https://github.com/cloudflare/workerd/pull/4383) | `20eb99f1cef1` | 原生 get、WorkerCode、entrypoint/actor class；并不创建独立执行线程 |
| [#4579 Production loader interfaces](https://github.com/cloudflare/workerd/pull/4579) | `fac362eff344` | 生产接入所需接口调整；CF 生产宿主实现与 standalone 不同，不能据此声称 OSS 含完整生产实现 |
| [#4834 Service bindings in dynamic env](https://github.com/cloudflare/workerd/pull/4834) | `7483d40e90ab` | ctx.exports entrypoint 与 props 中 binding 的传递；不代表 Loader 对象可传递 |
| [#5693 Fetcher over RPC](https://github.com/cloudflare/workerd/pull/5693) | `8f3a2c11e6dd` | Fetcher 与 DurableObjectClass 的 channel 序列化；不能推导所有 DO instance stub 或对象均支持 |
| [#6316 load()](https://github.com/cloudflare/workerd/pull/6316) | `87395c3a68df` | 原生 one-off load，复用 get(null, callback) 路径及引用计数 source |
| [#6553 Dynamic loader UAF fix](https://github.com/cloudflare/workerd/pull/6553) | `696113ef503e` | 保留请求 channel 和 actor startup 所需强引用；新 delegation/eviction 必须维持这些生命周期 |
| [#6822 Upstream changes](https://github.com/cloudflare/workerd/pull/6822) 中的 [RpcStub env commit](https://github.com/cloudflare/workerd/commit/b720d551a5f4a761bb0da91f4beb766f90d473f0) | merge `7d71003a12a2` | persistent RpcStub 的动态 env channel 支持；不等于所有 transient RPC 对象均可传递 |
| [#6997 WebAssembly.Module](https://github.com/cloudflare/workerd/pull/6997) | `9cdf38052e16` | 直接 Module 和 {wasm: Module}，共享编译结果，覆盖新旧 module registry；避免代理序列化后重新编译 |

当前 [`worker-loader.h`](../../third_party/workerd/src/workerd/api/worker-loader.h) 中 WorkerLoader 没有可转移的
JSG capability；server 动态 env rewrite 接受 subrequest、actor class、RPC channel，缺少 Loader channel。
本次公开检索未找到补齐这一嵌套 Loader 委派链路的 PR，当前源码也仍缺失。已存在的 service/RPC 支持不能替代它。

[#5681 WorkerCode validation](https://github.com/cloudflare/workerd/issues/5681) 仍 open：未知 dictionary 字段通常被忽略，
类型检查负责提示拼写错误。P1 应保留原生语义，不能把管理面严格 metadata 校验移植到 WorkerCode。
module 的“恰好一种类型”则由已知 type 字段计数实现；未知附加键不等于第二种类型。

## 补丁与升级审查边界

- **复用**上述已合并实现，不重复 cherry-pick、不新增 JS facade、RPC 对象全集代理或第二套 module loader。
- **新增**原生 Loader capability 的受约束委派、namespace 接线及 limits 执行器；具体所有权与退出条件见
  [原生实施方案](../workerd/native-limits-loader.md)，API 与安全验收见 [P1](../workerd/p1-dynamic-workers-worker-loader.md)。
- **保留**上游 GC/UAF 强引用关系；评审新缓存驱逐与 OOM 路径是否会提前释放 service、channel 或 IoContext。
- **核验**每次 pin 升级的 compatibility date/flags、transfer 规则与 module registry；PR 当年的 experimental flags
  不是当前支持承诺。普通 RPC 可传递、动态 env 可接收、动态 entrypoint 可转移要分别验证。
- **贡献**优先讨论通用 Loader 委派与最小执行器接口，产品预算/权限留在 open-compute。#49/#1627 不支持承诺
  完整执行器一定会被上游接收；先交付 fork，按独立组件维护补丁，已被上游取代的实现随协调升级删除。

验收需同时覆盖单租户超限后的邻居可用性、有界 native 内存增长、GC 与异步启动安全。
CPU watchdog、完整内存计量和跨平台恢复尚待实现与实测，不能用本次源码审查宣称 100% Cloudflare 兼容。
