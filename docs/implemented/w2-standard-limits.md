# W2：Workers Standard Resource Limits

状态：**implemented and verified，2026-09-14**。W2 直接建立当前 Day1 合同，删除旧的
`OC-WKR-LIMIT-001` 功能缺口，不保留曾经拒绝 `limits` 的配置分支、旧类型名、双 wire schema 或运行时
fallback。正式源码、四平台二进制和 digest 以
[`packages/runtime/workerd.lock.json`](../../packages/runtime/workerd.lock.json) 为唯一 authority。

## 交付合同

| 限制                              |                         Standard 合同 | authority 与执行位置                            |
| --------------------------------- | ------------------------------------: | ----------------------------------------------- |
| invocation CPU                    | 默认 30,000 ms；可配置上限 300,000 ms | immutable Version；workerd request enforcer     |
| invocation subrequests            |    默认 10,000；可配置上限 10,000,000 | immutable Version；workerd request enforcer     |
| isolate memory                    |                               128 MiB | 固定 Standard profile；workerd isolate enforcer |
| startup CPU                       |                              1,000 ms | 固定 Standard profile；workerd startup enforcer |
| simultaneous outbound connections |                        6 / invocation | workerd request accounting；只计到响应头到达    |

Wrangler JSONC/TOML 与 framework output 使用官方 snake-case `limits.cpu_ms` / `limits.subrequests`。
边界拒绝未知字段、camelCase、`null` 维度、非整数、零、负数和超上限值；省略整个对象、`null` 配置或
空对象都在 Version 创建时一次性物化 Standard 默认值。仓库内部只保留
`EffectiveResourceLimits`，持久化 Version、signed runtime-source snapshot、Loader 与 workerd 沿一条链传递，
读取非 canonical 或越界的持久化值会 fail closed。

Cloudflare v4 Script Settings `GET` 返回两个 effective limits；multipart `PATCH` 支持逐维更新。PATCH 克隆
active Version、切换 active pointer 并保留未声明维度，绝不修改历史 Version。Version response 的
`resources.script_runtime.limits` 按官方 shape 只公布 `cpu_ms`，service metadata 同时公布 CPU 与
subrequest limits。启动阶段触发 CPU/memory 限额会拒绝 candidate、保持旧 active 不变，并映射为官方上传
错误 `10021`。

Dynamic Worker 的 `WorkerLoaderWorkerCode.limits` 与 `WorkerStubEntrypointOptions.limits` 使用官方
camelCase `cpuMs` / `subRequests`。Version、WorkerCode、entrypoint 及继续委托的 Loader capability 都逐维取
最小值；省略下层值继承父级 ceiling，child 和 descendant 不能放宽包含它们的 Worker 限额。

## 执行、错误与恢复

- CPU 以执行 tenant JS/Wasm 的线程 CPU 时间计量，I/O await 不收费；watchdog 的最大已声明 overshoot 为
  5 ms。CPU、memory 或 startup fatal limit 会 condemn 并摘除所属 isolate，后续 invocation 从 immutable
  Version 重建；邻居 Worker 和 workerd 进程不重启。
- 第 N+1 个 subrequest 在任何外部 side effect 前失败。公开 HTTP/KV/R2/D1/DO/Queues/Cache/Service 等已
  支持通道经过同一原生 subrequest hook；CONNECT 也先取得 connection slot。
- 第 7 个 outbound connection 排队；HTTP、WebSocket 和 CONNECT 在响应头/握手结果到达时释放 slot，
  response body 或已建立 socket 的存活期不继续占用该槽。
- CPU 和 memory 终止返回 HTTP 500、Cloudflare `1102`、`cf-error-type: 1102` 以及
  `exceededCpu` / `exceededMemory` outcome。普通 subrequest 超限是 uncaught exception：HTTP 500、`1101`、
  `cf-error-type: 1101`、`exception`。响应和日志不暴露源码、Loader key、内部 URL/token 或原始异常。
- 若 workerd 退出或仍存活但 generation-scoped `/internal/live` 不推进，supervisor 经过同 generation 确认后
  撤销 credential、bounded teardown/reap 并按既有 budget 重启。普通 tenant 超限不会消耗 restart budget。

## 测试与资格

workerd 单元/WD tests 覆盖 CPU clock、budget/min composition、subrequest side-effect 顺序、响应头阶段 6-slot
排队、memory/startup condemnation、isolate rebuild、邻居隔离和 delegated Loader ceiling。产品真实进程用例
覆盖 Wrangler deploy → v4 upload → immutable Version → snapshot → Loader → workerd、Settings 全量/部分
PATCH、历史 Version 不变、restart 后 limits 保留、CPU/subrequest 公开错误和 startup validation。

最终验收在冻结的正式 pin 上执行 Rust format/Clippy/no-default-features/MSRV/metadata/boundaries、TypeScript 与
generated conformance checks、coverage，以及恰好一轮 workspace Gate。精确命令、case 数、覆盖率和报告路径
记录在完成本项的最终交付报告中；历史 W2 Gate 不替代本轮验收。

## 兼容性边界

官方公开且属于当前支持面的配置、API、错误与执行合同已实现。`OC-WKR-LIMIT-001` 只保留以下稳定差异：

- Cloudflare 未公开的 CPU grace、精确调度/采样和 hosted analytics/billing；
- Cloudflare 多机 isolate placement/eviction 与 memory-pressure draining；本项目是单机、单 workerd 的确定性
  恢复，fatal memory breach 会终止被摘除 isolate 的在途调用，而不承诺 Cloudflare 在正常负载下优先让其完成；
- open-compute 的通道 wrapper 对所有 dynamic outbound channel 统一计数，可能比 Cloudflare 私有的产品计费
  分类更保守；嵌套 Service target 当前使用自己的 invocation pool，不宣称 Cloudflare top-level shared-pool
  内部实现等价；
- Dynamic Python Loader cold boot 未资格化：本地没有 Cloudflare hosted deploy-time Python 预计算，不能
  稳定在官方 1 秒 startup CPU limit 内完成 Pyodide bootstrap；该限额保持 fail closed，不为此添加例外；
- 本地 JSON error extension 保留稳定 open-compute code；它同时提供官方 `1101`/`1102` 与
  `cf-error-type`，但不仿制 Cloudflare hosted 品牌错误页或私有 `cf-error-origin` topology。

这些差异不允许扩大成缺失字段、静默忽略、弱化安全边界或另一个兼容实现。官方依据见
[Workers limits](https://developers.cloudflare.com/workers/platform/limits/)、
[Dynamic Workers limits](https://developers.cloudflare.com/dynamic-workers/usage/limits/)、
[Script Settings GET](https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/script_and_version_settings/methods/get/)、
[Script Settings PATCH](https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/script_and_version_settings/methods/edit/)和
[Workers errors](https://developers.cloudflare.com/workers/observability/errors/)。

返回[已实现索引](README.md)。
