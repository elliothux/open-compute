# Static Assets 与 Service Binding 远端资格计划

状态：Active，2026-09-01。本文只追踪 P3.1 Static Assets 与 P3.2 Service Binding 尚未完成的
Cloudflare 托管端 direct differential。两项 Day1 核心实现和本地最终验收已经完成，对应设计与证据
归档在 [P3.1](implemented/p3-1-static-assets.md)和
[P3.2](implemented/p3-2-service-bindings.md)。

## 已完成前提

- P3.1 已实现 Worker-only、Worker + Assets、Assets-only、上传恢复、不可变 deployment、默认资源路由、
  显式 Assets binding、S3 对象生命周期和 crash/restart 边界；本地最终三轮策略 Gate 已通过，Rust
  行覆盖率为 90.11%。
- P3.2 已实现默认/命名 fetch 与原生 RPC、同账户目标解析、调用预算、deployment pin、事件源调用、
  generation 回收和 SIGKILL 恢复；本地最终验收共 834/834 case 通过，Rust 行覆盖率为 90.11%。
- P3.4 已固定 contract catalog、capability/deviation 与本地产品证据；当前 stable tenant API inventory
  为 2,097 个成员、`blocked=0`。
- 后续固定 vinext workload 已取得 [Application Go](implemented/p4-nextjs-vinext-results.md)，证明选定
  应用的 Assets、SSR/RSC 和浏览器路径可在 Cloudflare 与 open-compute 运行。该应用结果没有覆盖完整
  Assets routing/binding contract，且明确排除了产品 Service Binding 组合，因此不替代本计划。

## 尚未完成的外部证据

### Static Assets

为同一 portable fixture 建立 Cloudflare 与 open-compute adapter，至少直接比较：

- Assets-only 与 Worker + Assets 的默认 HTTP 路由；
- assets-first、`run_worker_first` 与显式 Assets binding；
- GET/HEAD、ETag/304、HTML handling、404/SPA、`_headers` 与 `_redirects`；
- 路径编码、缺失资源和配置拒绝等可移植失败行为。

S3 生命周期、上传恢复、deployment pin、GC 和平台进程崩溃属于单机 authority 证据，没有等价的
Cloudflare 托管端观察面，继续由本地产品 Gate 与公开 deviation 负责。

### Service Binding

使用唯一命名的同账户调用方/目标 Worker，至少直接比较：

- 默认与命名入口的 `fetch()`；
- 默认与命名 RPC 的参数、返回值、异常、stream 和公开成员边界；
- self binding、A→B 调用和目标部署切换后的新调用行为；
- 能在两端稳定触发且不依赖私有控制面的预算或失败行为。

Queue/Cron/DO/Workflow 事件源、workerd SIGKILL、generation handle 回收、删除 fence 和本地
deployment pin 是 open-compute 的产品生命周期保证。若 Cloudflare 没有等价、可观测且可重复的托管端
接口，应保留为本地 Gate/deviation，不能伪造 differential。

## 执行边界

- 先冻结 Cloudflare account、Wrangler、workers-sdk、workerd lock、compatibility date、fixture 和
  open-compute source identity；输入不完整时停止。
- Cloudflare 部署和删除属于外部写操作，执行前需取得当次明确授权。资源使用唯一前缀，preflight
  必须确认不会覆盖账号已有 Worker、route、service 或 binding。
- 每项只创建 fixture 自有资源，按精确 name/ID 清理并复查 absent；不做账户级批量删除。
- 两端运行同一公开输入和断言，只归一化已记录的 provider identity、时间或拓扑字段；不得为使
  open-compute 通过而修改应用逻辑、降低断言或新增生产特判。
- 失败后保留脱敏 evidence；credential、token、内部 URL、signed URL 和账号既有资源不得进入报告。

## 完成条件

1. 两项各有固定 manifest、case registry、Cloudflare/open-compute adapter 和逐项结果。
2. Cloudflare 通过而 open-compute 失败的 declared-supported contract 为零；差异均映射到已有或经审查
   新增的稳定 deviation。
3. Cloudflare 资源精确清理并复查 absent；本地进程、listener、SQLite/S3 fixture 和临时文件有界回收。
4. 结果写入独立完成报告，更新 capability/deviation、
   [Cloudflare 兼容参考](references/cloudflare-compatibility.md)、总方案与归档索引。
5. 未运行、账号阻塞和没有托管端等价观察面的项目明确保留，不把本地 Gate 改写成 hosted PASS。

本计划完成与否不改变已经归档的核心实现事实；它只决定能否扩大 Static Assets / Service Binding 的
Cloudflare 托管端资格结论。
