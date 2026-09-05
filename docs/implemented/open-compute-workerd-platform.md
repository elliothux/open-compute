# 单机 Cloudflare Workers 平台架构

本页说明已实现平台的职责与数据边界。产品 API 和差异统一见[兼容矩阵](../references/cloudflare-compatibility.md)；待实现能力见[文档索引](../README.md)。

## 运行与信任边界

```text
客户端 / Wrangler / SDK / Dashboard
                │
             ocd（公开入口）
                ├── /client/v4 管理面、认证与授权
                ├── 路由、部署身份、请求准入
                ├── SQLite authority、调度、对象后端
                └── 受监督的 workerd child
                       ├── 静态 system Workers / Loader
                       └── 动态 tenant Workers / 原生 DO facets
```

一个 `ocd` 独占数据目录和 master key 生命周期，并监督一个正式固定的 workerd child。
内部 listener 只在 loopback，generation capability 与内部服务不可作为租户 `env` 暴露。
一般出网由 workerd 的地址层 public-only Network capability 管理。

普通 Worker 从不可变部署内容动态加载；runtime cache 可丢失，authority 不能依赖进程内 registry。
版本切换和回滚改变部署／路由指针，in-flight 请求与事件继续持有其执行内容和资源引用。
原生 limits 与 Loader 委派在 [workerd fork 方案](../workerd/README.md) 中推进，源码 submodule 不自动改变正式 runtime pin。

## 所有权

| Owner | 职责 |
| --- | --- |
| `crates/core` | 配置、ID、错误、secret 引用、时钟与健康基础类型 |
| `crates/storage` | 目录与锁、SQLite schema／authority、持久状态、加密、快照 |
| `crates/artifacts` | 对象后端、artifact／backup 等 domain store、验证与 cache |
| `crates/runtime` | 正式 workerd pin、配置编译、进程监督与 runtime 物化 |
| `crates/workers` | Bundle／Version／Deployment、资源与引用生命周期、RuntimeSource snapshot |
| `crates/search` | 搜索、tokenization、chunking 与 AI Search 数据处理 |
| `crates/document-parser` | 有界文档解析与 OCDP 帧协议；child 监督归 `service` |
| `crates/images` | 有界图像解码、变换与输出 |
| `crates/service` | `ocd` CLI、HTTP、鉴权、scheduler 与上述能力的组合 |
| `packages/runtime` | System Workers、Loader、各产品的 tenant facade 与私有协议 |
| `packages/toolchain` | 开发时编译、bundle、Wrangler 输入及部署工具 |
| `packages/dashboard` | 可选管理界面，通过正式管理 API 操作资源 |

依赖方向由 [`test/check-boundaries.sh`](../../test/check-boundaries.sh) 校验；SQL、协议和配置的字段定义以拥有它们的源码为准。

## 数据与恢复

| 数据 | Authority 与恢复边界 |
| --- | --- |
| 账号、资源、版本、路由与 secret metadata | `control.sqlite`；不可变内容与引用需一致验证 |
| KV / D1 | 各资源独立 SQLite；单资源损坏隔离，在线 backup／restore-as-new |
| Durable Objects | workerd 原生 facet storage；平台管理对象身份、generation 和宿主生命周期 |
| Alarm / Queue / Cron / Workflow | 领域持久状态与 scheduler 投影；租约、generation、reconciler 跨重启收敛 |
| R2 / Assets / artifact / backup / 搜索文档 | 唯一 Local 或 S3 对象后端；各 domain store 管理 key、metadata、完整性与生命周期 |
| Vectorize / AI Search | SQLite authority、持久 indexing 状态和可重建搜索加速数据 |
| Worker 日志 | 独立有界日志 authority；retention、权限及重启恢复见 P7 |

所有 SQLite transaction callback 保持同步；网络、文件和异步执行放在事务外。
跨库、对象提交与响应丢失通过持久 operation／generation 判断，不靠无条件重试掩盖未知结果。
GC 只回收可证明无引用且属于本平台的对象；读取不自动修复损坏状态。

整机快照与恢复需要独占数据目录，恢复到空目标 staging；master key 独立保管。
Local 对象后端不自动提供异机备份。操作命令与故障处置只维护在[运维手册](../references/README.md#运维手册)。

## 产品实现入口

| 领域 | 文档 |
| --- | --- |
| Worker / Binding | [装载与部署](p0-2-workers-runtime.md)、[资源与引用](p0-3-resource-binding-framework.md) |
| KV / R2 / D1 | [KV](p0-4-kv.md)、[R2](p0-5-r2.md)、[D1](p0-6-d1.md) |
| Durable Objects | [宿主与存储](p0-7-durable-objects.md)、[Alarms](p0-8-scheduler-do-alarms.md) |
| Queue / Cron / Workflow | [Scheduler](p2-1-scheduler-hardening.md)、[Producer](p2-2-queue-producer.md)、[Consumer/Cron](p2-3-queue-consumer-cron.md)、[Workflow](p2-4-workflow-core.md)、[持久等待](p2-5-workflow-durable-waiting.md) |
| Assets / Service / Cache / Images | [Assets](p3-1-static-assets.md)、[Service Binding](p3-2-service-bindings.md)、[Cache/Images](p3-3-workers-cache-images.md) |
| 搜索与解析 | [Vectorize/AI Search](p5-vectorize-ai-search.md)、[文档解析](p5-7-xberg-document-parsing.md) |
| 管理与日志 | [v4 管理合同](p6-cloudflare-v4-wrangler-compatibility.md)、[Dashboard](operator-api-dashboard.md)、[Logs/Tail](p7-workers-logs-realtime-tail.md) |
| 对象后端 | [Local/S3](p8-local-s3-object-backend.md) |

## 验证与交付

API 类型直接消费固定上游声明；具体支持、偏差、用例证据由 conformance inventory 关联。
平台与应用 qualification 分别判定，某个应用可运行不能代替平台合同验收。

历史实际验收见[完成索引](README.md)，剩余外部／长时／跨平台资格见[验收索引](../acceptance/README.md)。
当前测试流程见[测试手册](../references/testing.md)，构建、正式 archive 和离线发行契约见[单二进制指南](../references/single-binary.md)。
