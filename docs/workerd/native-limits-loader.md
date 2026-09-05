# 原生 Dynamic Worker Loader 与 Standard limits

日期：2026-09-05。状态：**待实现、未验收**。源码位置和 fork 身份见[目录索引](README.md)。
本文是 P1/P2 的原生实现方案，不把源码接口、类型声明或已有 stock probe 当成产品完成证据。

## 1. 架构决策

- 保留一个 `ocd` 和一个受监督、正式固定的 workerd child；普通 Worker 继续由 system Loader 动态装载。
- 在 `third_party/workerd/` 中实现 native enforcer 与 Loader capability delegation，编译用户维护的 fork。
- 尽量将执行器写成独立 C++ 模块，在 standalone server 做必要接线；缺少生命周期或权限接口时才扩展核心。
- 复用原生 `load/get/WorkerStub/Fetcher/DurableObjectClass`，不增加 JS Loader facade、全对象代理层或租户源码重写。
- 当前没有必须修改 V8 引擎的证据；先使用 workerd/V8 的宿主接口。完整 CPU、内存、同步 native 调用和恢复行为
  仍需实现验证，不能预先承诺现有钩子足以覆盖全部路径。
- 保持 Day1 单一实现，不增加 stock/fork 双执行引擎、启动 fallback、历史数据兼容或运行时下载。

“通过接口实现”指编译期实现 C++ 虚接口，不是向官方二进制装入插件。当前
[`Server`](../../third_party/workerd/src/workerd/server/server.h) 没有注入完整执行器的公共工厂；
[`server.c++`](../../third_party/workerd/src/workerd/server/server.c++) 创建空执行器，仍须修改装配和配置传递。

## 2. Standard limits：接口与修改边界

主要接口位于 [`io/limit-enforcer.h`](../../third_party/workerd/src/workerd/io/limit-enforcer.h)。
`IoContext` 已在运行 JS、创建 subrequest 和等待超限通知时调用这些接口。

| 能力 | 可复用接口／原生路径 | 必须新增或修改的部分 |
| --- | --- | --- |
| invocation CPU | `LimitEnforcer::enterJs()`、`topUpActor()`、超限状态与通知 | CPU 计量、独立于被执行 JS 的终止机制、请求预算和宿主装配 |
| startup CPU | `enterStartupJs/Python()`、`enterDynamicImportJs()` | 启动预算、终止与验证错误传递；各语言入口单独证明覆盖 |
| isolate memory | `getCreateParams()`、`customizeIsolate()`、`exitJs()`、buffering limit | heap、ArrayBuffer、Wasm 与相关 external memory 计量；淘汰与在途请求恢复策略 |
| subrequest 数量 | `newSubrequest(isInHouse)` | 计数和调用前拒绝；审计产品 facade、RPC、redirect 路径，缺失时补计量入口 |
| drain／事件时限 | `limitDrain()`、`limitScheduled()`、`getAlarmLimit()` | 实现计时策略并核对 fetch/scheduled/queue/alarm/DO 的实际入口 |
| outcome／metrics | `getLimitsExceeded()`、`onLimitsExceeded()`、observer | 传递真实 exceededCpu/exceededMemory 和用量，接入平台 collector |
| 等待响应头的连接数 | channel、`WorkerInterface`、HTTP/TCP client 路径 | 新增排队、释放和顶层请求共享预算；没有现成的完整配额接口 |

新增执行逻辑按 request budget、isolate budget、连接 admission 的职责组织。`server.c++` 只做创建与绑定，
避免继续把大段策略堆入该文件；不要为每个 API 重复实现限额算法。

### 2.1 CPU 与内存

CPU 按 invocation 累计实际执行时间，排除异步 I/O 等待；不可用整进程 CPU 或 wall timeout 冒充。
JS/Wasm 无限循环、微任务循环必须能终止，watchdog 不能依赖被阻塞的同一事件循环。
执行切换、终止信号和 isolate 销毁必须有身份与生命周期保护，不能误终止下一次请求或访问已释放 isolate。
DO 的共享 IoContext 和 top-up 规则需单独资格化；线程 CPU 计量与终止机制分别覆盖 Linux/macOS。

V8 终止并不保证任意长时间运行的 C++ 操作立即停止。同步 SQL、crypto 和其他 native 工作应核对其现有
取消／计量入口；若某条支持路径缺少入口，补该组件的钩子或明确记录未完成，不能仅通过 JS 死循环测试宣称覆盖。

内存是 per-isolate，包含 JS heap 和 Wasm 等合同内分配，不能用 workerd RSS 替代。已有 CreateParams
可提供 allocator，customize hook 可接 V8 回调；这只是接入能力，不证明总内存计量和可恢复 OOM 已实现。
避免同一 backing store 重复计量；不能让一个租户的资源超限直接使整个 workerd fatal OOM。
超限 isolate 的停止新 admission、在途请求处理和冷启动替换须按官方合同验证。

### 2.2 subrequest 与连接

每个租户可见的逻辑调用进入对应 invocation 的 budget；产品内部 backend/provider fan-out 不再次扣该租户预算。
按 [P2](p2-workers-standard-limits.md) 的 inventory 检查 global fetch、redirect、Service Binding 和各产品入口，
不能只统计 Rust HTTP transport，也不能假设所有调用都自动经过 `newSubrequest()`。

连接配额有自己的 scope：顶层请求的 Service Binding 调用链共享等待响应头的配额。第七个调用应排队，
在收到响应头、失败或取消时准确释放，不能等 response body 读完才释放，也不能用全局 semaphore 代替。
在原生 channel/HTTP/TCP 层包装请求，并补齐必要的共享预算传递；租户 header/props 不能伪造预算身份。

### 2.3 保留在 open-compute 的工作

配置校验、immutable Version、vars/secrets 数量与大小、upload/ingress structural limits、整机 admission、
capability 状态和持久化继续由现有 Rust/TS 所有者处理。workerd 接收经过 authority 验证的有效预算。
系统 Worker 与租户 Worker 的计量归属必须明确，不能让内部加载和 collector 消耗混入租户额度。

## 3. 原生 Worker Loader：必须补齐的路径

```text
ocd：Version / Script identity / policy authority
  → static system loader-host：内部 Loader
  → ordinary tenant Worker：原生、独立 namespace 的 public Loader
  → Dynamic Worker：原生 WorkerStub / entrypoint / class
```

| 能力 | 实现位置与要求 |
| --- | --- |
| `load/get`、同步 stub、模块编译、entrypoint 调用 | 复用 `api/worker-loader.*` 与现有 WorkerStubChannel，不另写一套 API |
| Loader capability 委派 | 在 JSG 对象序列化、I/O channel 类型、动态 env rewrite/注册之间补完整链路 |
| 独立 namespace 的创建 | 在宿主增加受信任创建／委派接口；复用 namespace 实现，不能授予 system namespace |
| code／entrypoint lower limits | 接通 `DynamicWorkerSource.limits` 与 resolved channel 参数，创建 invocation budget |
| distinct in-flight | 在原生调用生命周期按 caller IoContext 和 child identity 计数、去重、释放 |
| named cache 与 namespace 回收 | 在宿主实现 bounded eviction、tombstone/drain、启动失败清理；内存不是 authority |
| parent 自有 capability 交给 child | 核对动态 `ctx.exports` 等 transfer 限制；按实际合同补受约束委派，不能全局放开检查 |

主要改动面：[`worker-loader.h`](../../third_party/workerd/src/workerd/api/worker-loader.h)、
[`worker-loader.c++`](../../third_party/workerd/src/workerd/api/worker-loader.c++)、
[`io-channels.h`](../../third_party/workerd/src/workerd/io/io-channels.h)、Frankenvalue/capability serialization、
standalone server 的 WorkerDef/channel linking/WorkerLoaderNamespace 及对应原生测试。
具体接口名在实现时确定；本文不预先声明一个未实现的公共 JS API。

### 3.1 权限与 namespace

namespace 由平台根据不可复用的 Script identity 与 binding identity 派生，tenant 只能使用被授予的能力。
system key 与用户 `get(id)` key 必须处于不同 namespace，不能依赖用户 key 前缀自觉避让。
授予的能力应限制继续委派的权限，不允许租户选择其他账号/Script、创建任意平台 namespace 或提高预算。
Version 回滚、Script 删除重建与 namespace 生命周期按 [P1](p1-dynamic-workers-worker-loader.md) 处理。

Loader delegation 不要求把所有原生对象改成可跨任意 RPC 传输。原生 stub 在 caller isolate 内创建，
`getEntrypoint()` 和 `getDurableObjectClass()` 留在原生路径；只扩展受支持 env/props/outbound/tails
确实需要跨越的 capability 边界。普通 RPC 可传递与动态 env 可接收是不同规则，需分别验证。

### 3.2 limits 与生命周期

预算执行属于 P2；P1 完成下述 in-flight 与生命周期，并拒绝显式 limits。

Standard 默认值、可信 parent/Version 约束以及 code/entrypoint lower limits 的组合在宿主集中计算，
按固定官方 fixture 确认默认继承范围；租户只能降低已授予的上限。每次 invocation 有独立 CPU/subrequest
计量；内存归 isolate，连接预算归顶层调用链，distinct child 计数归 caller IoContext。
不要把某次 entrypoint 的 lower limit 写入共享缓存 isolate，影响随后其他调用。

按 2026-09-05 官方文档，普通 Worker 请求为 4 个 distinct Dynamic Workers，DO 共享 IoContext 为 10 个；
同一 child 多个在途调用只计一个。不能以 `load/get` 次数、stub 数或缓存条目数代替在途数量。
各事件、RPC 会话、流完成／取消和 startup 失败的释放时点需要原生 regression。

named cache 只提供机会性复用。保留不可变 code identity，回收时不得取消仍应存活的请求；最后一个 JS
handle 被 GC 不得使 startup/use-after-free。进程重启后从 authority 恢复平台配置，动态缓存可丢失。

## 4. 实施顺序与退出条件

| 阶段 | 工作 | 退出条件 |
| --- | --- | --- |
| N0：固定输入 | 核对 fork HEAD、upstream base、工具链、P1/P2 API fixture 和 native 调用路径 | 两个 Git 仓库的身份与差异可审查；旧 pin 证据与 fork 分开 |
| N1：Loader 委派 | 独立 namespace、原生 env、必要 capability、同步 API | tenant 能原生 load child，越权与非法转移被拒绝 |
| N2：Loader 生命周期 | in-flight、结构大小限制、缓存、回收及显式 limits 拒绝 | cold/warm/GC/failure/stream/restart 行为成立 |
| N3：P1 平台交付 | formal pin、types、Version/binding、manifest、docs 与产品测试 | Loader 声明子集完成 native/兼容检查及最终单轮验收 |
| N4：P2 基础 enforcer | CPU/subrequest/startup，outcome 与请求生命周期接线 | 普通与动态 Worker 的预算和 lower limits 实际执行 |
| N5：P2 完整资源边界与验收 | memory、native 操作、事件时限、连接配额及平台接入 | 超限恢复、邻居可用性与完整 P2 合同通过验收 |

P1 优先交付，不依赖 P2 完成。P1 只开放不带显式 limits 的原生 Loader 子集，所有接收 limits 的入口
均须明确拒绝该选项（包括空对象），不得继续走 upstream no-op；验证错误类型、发生阶段和脱敏。
默认 CPU/内存/subrequest 限额未执行的偏差必须在 capability/deviation/docs 中明确，不能声称完整资源隔离。
P2 直接将这些入口改为真实预算执行并删除拒绝分支，不保留旧行为开关或兼容路径。

P1 的 namespace、egress、in-flight、结构大小与生命周期检查不可后置。N0 将固定合同写入 fixture，
各阶段只验收自身已声明范围；P1/P2 分别完成约定检查与单轮最终 Gate 后归档。

## 5. 构建、pin 与验证

使用 fork 自己的 Bazel 构建，入口为 `//src/workerd/server:workerd`；示例在 `third_party/workerd/` 执行：

```sh
bazel --output_user_root="$PWD/../../.temp/workerd-bazel" build //src/workerd/server:workerd
```

该命令只是已核对的后续入口，本次没有执行。首次 Bazel/toolchain/dependency 下载与 release packaging
仍按仓库授权规则处理。诊断和失败证据放在 open-compute 根 `.temp/`，保留失败现场；不为省空间删除持久数据。

开发二进制不得伪装成现有正式 archive。进入平台 acceptance 前，一次协调更新唯一
`packages/runtime/workerd.lock.json` 及其校验器/准备工具/打包器/CI，记录 fork source commit、upstream base、
可重现构建输入、四个平台 archive/binary SHA-256、版本输出、日期和 flags；具体 schema 在该实现中直接修订。
保留上游 types 的独立来源身份，不能把 fork commit 冒充 upstream npm 包的 gitHead。
不新增另一个运行时 pin authority，不接受来源不符、只改 version string 或跳过 checksum 的测试输入。

保持单 `ocd` 内嵌匹配 archive 的离线启动契约。fork pin 切换前，现有准备与包装工具仍按当前官方 pin 工作，
本文不表示它们已经能接受 fork archive。

测试分工：

- fork 中维护 C++/`.wd-test` 回归；沿用原有 BUILD target 与组件规则，新 case 注册到其拥有的 target。
- open-compute 中维护真实产品路径：可信预算、租户隔离、Version 回滚、进程恢复与供应链校验。
- 通过 `cf-compatibility-check` 复核公开合同。stock workerd 是 upstream 行为基线；fork acceptance 必须执行
  正式固定的 fork 二进制。Cloudflare differential 使用独立测试资源，不影响账号现有服务。
- 用户要求最终完成前 Gate 只跑一轮：实现阶段以源码审查和必要的定向测试收敛；冻结后每个选定原生 target
  和产品 Gate target 各执行一次，不做三轮、重复 aggregate 或相同输入的无意义复跑。Bazel 的 compat variants
  按 target inventory 审查，不把框架生成的不同配置误当作相同 case 的重复。
- 平台接入后先 `bun run build` 准备 runtime assets，再按仓库规则完成静态检查、一次 coverage，最后一次
  `./test/gate.py --workspace`。要求使用绝对路径的正式 archive 与 `OPEN_COMPUTE_TEST_WORKERD`；本次文档修改只做
  diff/link/路径与命令核对，不运行 Rust、Bazel、coverage 或 Gate。

## 6. 上游更新与贡献

[上游 issue / PR 核验](../references/workerd-upstream.md)是能力状态的统一记录。当前 fork 已含原生 load、
custom limits 参数、service/RPC env 与编译后 Wasm 支持；新增补丁集中在执行器、Loader 委派与宿主生命周期。
#6399 不包含 standalone enforcement；#1627 未合并且不解决 isolate 重建；#6553 的 GC/UAF 修复必须保留。
P2 负责超限恢复与 native 内存回归，P1 负责类型化 capability 委派、输入语义和异步生命周期回归。


将执行器、宿主接线、Loader delegation 分成可独立审查的改动；业务账号、SQLite/S3、部署流程留在 open-compute。
升级时从已知 upstream revision 审查相关 changes，再在同一 fork 目录更新；既检查 patch 冲突，也检查 CPU scope、
capability token、GC 和缓存生命周期的行为变化。源码合并成功不等于兼容验证通过。

通用 Loader 委派与执行器接口补全适合向 upstream 提案。先准备复现、权限模型和小范围设计，遵循
[上游贡献规则](../../third_party/workerd/CONTRIBUTING.md)讨论非简单变更；上游是否合并不再是本地交付前置条件。
发布 issue/PR、push 和 release 是外部写入，按用户明确授权执行。合并上游后通过下一次协调升级消除已被取代的
fork 实现，不长期保留两条相同功能路径。

## 7. 证据与限制

- [Cloudflare Workers limits](https://developers.cloudflare.com/workers/platform/limits/)：CPU、memory、连接与 structural 合同。
- [Dynamic Workers API](https://developers.cloudflare.com/dynamic-workers/api-reference/)：同步 stub、env、outbound 与 tails。
- [Dynamic custom limits](https://developers.cloudflare.com/dynamic-workers/usage/limits/)：code 与 entrypoint lower limits。
- [Dynamic in-flight limits](https://developers.cloudflare.com/dynamic-workers/platform/limits/)：Worker/DO context 与 distinct identity。
- [旧 pin 可行性记录](../implemented/p10-worker-loader-feasibility.md)：仅证明当时 stock 的缺口。

尚未完成：fork 编译、执行器、委派、平台 pin 切换、Cloudflare differential 和完整验收。接口存在只证明可接入，
尤其 CPU/native 阻塞、完整内存计量、OOM 恢复及跨平台行为必须以新测试结果收敛，不能预先声明 100% 兼容。
