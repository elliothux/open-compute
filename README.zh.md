<p align="center">
  <a href="https://open-compute.dev">
    <img src="share/brand/open-compute.png" alt="open-compute" width="480" />
  </a>
</p>

<p align="center">
  <strong>单二进制、一键部署的高性能 Cloudflare Workers 兼容基础设施。</strong><br/>
  毫秒级冷启动 · MB 级内存占用 · 零额外依赖。
</p>

<p align="center">
  <a href="https://github.com/elliothux/open-compute/actions/workflows/ci.yml">
    <img src="https://github.com/elliothux/open-compute/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI" />
  </a>
  <img src="https://img.shields.io/badge/license-Apache--2.0-blue" alt="Apache-2.0" />
  <img src="https://img.shields.io/badge/runtime-verified%20workerd%20fork-f38020" alt="verified workerd fork" />
  <img src="https://img.shields.io/badge/API%20inventory-2%2C203%20members-success" alt="2203 stable members and overloads" />
  <img src="https://img.shields.io/badge/rust-1.98-orange" alt="Rust 1.98" />
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey" alt="macOS | Linux" />
</p>

<p align="center">
  <a href="https://open-compute.dev">官网</a>
  · <a href="https://open-compute.dev/docs/zh/">文档</a>
  · <a href="https://open-compute.dev/docs/zh/platform/compatibility/">兼容性</a>
  · <a href="https://open-compute.dev/docs/zh/project/">架构设计</a>
</p>

<p align="center">
  <a href="README.md">English</a> · 简体中文
</p>

---

## Workers 平台，跑在你自己的硬件上

你已经会写 Cloudflare Workers。**open-compute 运行兼容的 Workers 编程模型**——module worker、熟悉的 binding
与 Wrangler 工作流——都在你自己的一台机器上。

**一个二进制。一个数据目录。一个对象 authority。** 默认直接使用 Local 文件系统，也可显式选择 S3-compatible 存储。

没有 Kubernetes。没有 Redis。没有服务网格。没有需要照看的分布式控制面。没有厂商锁定。

```
   别人的方案                            open-compute
   ─────────────                        ────────────
   gateway + router                     ┌──────────────┐
   control plane                        │              │
   scheduler service          ═══>      │  ocd（1 个）  │
   Redis / Valkey 集群                   │              │
   Postgres                             └──────────────┘
   K8s + operators                       + SQLite + Local/S3 objects
```

## 为什么是 open-compute

**workerd 是运行时，不是平台。** 它把 Worker 隔离执行做得极好——然后就到此为止：没有多租户路由、
没有持久状态、没有调度、没有部署生命周期、没有控制 API。任何想在自己基础设施上跑 Workers 的人，
都得自己造这一层。

open-compute **就是这一层**——而且只有**一个文件**。

- **一个二进制，全部在内。** 运行时、控制面、调度器和全部产品 binding。拷到主机上、指向一个目录，就开始对外服务。
- **快，因为它是 workerd。** Worker 代码运行在固定并校验摘要的 workerd fork 中。isolate 毫秒级启动，不需要每个请求创建进程或容器。
- **没有别的要运行。** SQLite 持有平台 metadata；默认由 Local 存储对象字节，也可选择 S3-compatible 存储；两种模式都不需要数据库或缓存 sidecar。
- **固定并校验运行时。** runtime 及其资源在构建和启动时校验，生产启动保持离线。
- **完全属于你。** 你的代码、数据与机器由你拥有；外部服务可选，并且必须显式配置。

## 用证据说话

这里的兼容性是测出来的，不是宣称出来的。只要托管 API 允许直接对照，同一套 fixture 就会同时运行在 open-compute 和真实 Cloudflare 上。

|           |                                                                                       |
| --------- | ------------------------------------------------------------------------------------- |
| **2,203** | 个 stable API 成员和 overload，覆盖 Workers runtime 与产品 binding                    |
| **7 / 7** | 项核心产品与真实 Cloudflare 对照：Workers、Cache、KV、D1、R2、Durable Objects、Queues |
| **1 : 1** | 同一个生产级 Next.js 16 project 与部署产物可运行在 Cloudflare 和 open-compute 上      |
| **90%+**  | 强制行覆盖率下限，验收测试使用真实进程、SQLite 和固定的 workerd runtime               |

## 兼容性

编写标准 module worker（`export default { fetch }`），使用你熟悉的 binding。准确行为和单机差异见[兼容性指南](https://open-compute.dev/docs/zh/platform/compatibility/)。

### 运行时与 binding

| 模块                  | 状态               |
| --------------------- | ------------------ |
| Workers               | ██████████ 100% ✅ |
| KV                    | ██████████ 100% ✅ |
| R2                    | ██████████ 100% ✅ |
| D1                    | ██████████ 100% ✅ |
| Durable Objects       | ██████████ 100% ✅ |
| Alarms                | ██████████ 100% ✅ |
| Queues                | ██████████ 100% ✅ |
| Cron                  | ██████████ 100% ✅ |
| Workflows             | ██████████ 100% ✅ |
| Static Assets         | ██████████ 100% ✅ |
| Service Bindings      | ██████████ 100% ✅ |
| Cache                 | ██████████ 100% ✅ |
| Images                | ██████████ 100% ✅ |
| Version Metadata      | ██████████ 100% ✅ |
| WebSocket Hibernation | ██████████ 100% ✅ |
| Vectorize             | ██████████ 100% ✅ |
| Markdown Conversion   | ██████████ 100% ✅ |
| AI Search             | ██████████ 100% ✅ |
| Artifacts             | ██████████ 100% ✅ |

### 管理面

| 表面                         | 状态                                                                 |
| ---------------------------- | -------------------------------------------------------------------- |
| Cloudflare v4 API            | █████████░ 90% — 本地 `/client/v4` 可与 Wrangler 及官方 SDK 配合使用 |
| Wrangler                     | ██████████ 100% ✅ — Wrangler `4.127.1` 可部署和管理已支持产品       |
| Dashboard                    | ████████░░ 80% — 基于同一套 `/client/v4` API 的 operator UI          |
| Workers Logs / realtime tail | █████████░ 90% — 单机 logs、query、`wrangler tail` 与 live tail      |

### 部分支持

| 模块                    | 状态                                                 |
| ----------------------- | ---------------------------------------------------- |
| Dynamic Workers         | ████████░░ 76% — Worker Loader 核心 API 已可用       |
| Workers Standard limits | ██░░░░░░░░ 20% — 规划中                              |
| Workers AI              | ██░░░░░░░░ 20% — 仅 Markdown Conversion 与 AI Search |

### 规划中

设计进行中，尚不可部署对应 binding / API。

| 模块        | 状态                      |
| ----------- | ------------------------- |
| Browser Run | ██░░░░░░░░ 20% — 规划中。 |
| Containers  | ██░░░░░░░░ 20% — 规划中。 |

### 尚未支持

依赖这些能力的上传或配置会 fail closed。

| 模块                            | 状态                       |
| ------------------------------- | -------------------------- |
| 通用 Workers AI inference       | ░░░░░░░░░░ 0% — 尚未支持。 |
| Hyperdrive                      | ░░░░░░░░░░ 0% — 尚未支持。 |
| Analytics Engine                | ░░░░░░░░░░ 0% — 尚未支持。 |
| Workers for Platforms           | ░░░░░░░░░░ 0% — 尚未支持。 |
| Pipelines                       | ░░░░░░░░░░ 0% — 尚未支持。 |
| Rate Limiting                   | ░░░░░░░░░░ 0% — 尚未支持。 |
| mTLS certificates               | ░░░░░░░░░░ 0% — 尚未支持。 |
| Tail Workers / traces / Logpush | ░░░░░░░░░░ 0% — 尚未支持。 |

100% ✅ 表示文档列出的 Worker 或产品 API 没有缺失方法。单机差异见[兼容性指南](https://open-compute.dev/docs/zh/platform/compatibility/)。运行中能力：`ocd capabilities --json`。

## 快速开始

### 让 AI coding agent 完成安装

把下面这段提示复制到 Codex、Claude Code 或其他 coding agent：

```text
阅读 https://open-compute.dev/llms.txt，在这台机器上安装 open-compute 当前正式版本并配置一个本机 instance。先检查已有安装，默认保留现有配置和 instance 数据；使用 sudo 或执行破坏性操作前先询问我。最后运行 ocd status，并报告结果。
```

[`llms.txt`](https://open-compute.dev/llms.txt) 只包含最基本的 setup 和使用方式，需要时再按链接读取详细文档。

### 手动安装

安装正式 release binary，创建推荐的 system config，并启动 managed service：

```sh
curl -fsSL https://open-compute.dev/install.sh | sudo sh
sudo ocd setup --yes
ocd status
ocd dashboard
```

普通 Worker project 保持 Wrangler 为项目内 dependency；本地开发使用 Wrangler，真实 open-compute target 使用 `ocd wrangler`：

```sh
npm install --save-dev wrangler@4.127.1
npx wrangler dev
ocd wrangler deploy
```

生产环境保持**一个 release executable、一个 config、一个 data-dir**。runtime payload 内嵌并校验；daemon 启动不会下载 workerd，也不会搜索 `PATH`。

完整安装流程以及 remote target、CI、environment、tail 和 rollback 见[快速开始](https://open-compute.dev/docs/zh/get-started/)与[开发应用](https://open-compute.dev/docs/zh/develop/)。

## 架构

<p align="center">
  <img src="share/open-compute-architecture.png" alt="open-compute 架构图" width="880" />
</p>

| 组件                      | 职责                                                                       |
| ------------------------- | -------------------------------------------------------------------------- |
| `ocd`                     | 控制面：入口、API、调度器、supervisor 与部署 authority                     |
| `workerd`                 | 固定并校验摘要的 Worker runtime                                            |
| SQLite                    | 本机权威状态——无外部数据库，无最终一致性                                   |
| Local / S3 对象 authority | bundle、静态资源、R2、Artifacts、snapshot、backup、cache body 与 AI source |

租户只拿到自己部署声明的东西——别的一个都没有。没有 SQLite 或 Local object 路径、没有 S3 凭据、没有内部 token、
没有邻居租户。这由能力层强制，而不是靠约定。

### Rust 驱动，为热路径而生

宿主是一个单一的异步 Rust 进程——没有 GC 停顿，没有解释器，从 socket 到你的 Worker 之间没有 sidecar 跳数。

- **全链路异步。** `tokio` 多线程 runtime，`axum` + `hyper` 同时承载两个平面。请求体以 `bytes` 流式穿过，不缓冲整个 payload。
- **`unsafe_code = "forbid"`。** 全 workspace 生效——整个平台是 safe Rust。外加 `missing_docs = "deny"`、`unused_must_use = "deny"`，以及全 target/全 feature 的 Clippy `-D warnings`。
- **release 为速度而编。** 完整 LTO、`codegen-units = 1`、`panic = "abort"`、strip 符号表——一个致密的静态链接产物。
- **进程内状态。** `rusqlite` 将 SQLite 内嵌到 `ocd`；事务是函数调用，不是网络往返。外键保持开启，WAL 由本机持有。
- **该省的拷贝都省掉。** 校验过的运行时 payload 按内容寻址、只物化一次，跨重启复用。

### 分层 crate，边界由 CI 强制

依赖方向在 CI 中校验——不会悄悄腐化的架构：

```
core ── storage ── artifacts ── runtime      （同级，底层）
                    └── workers              （可用 core/storage/artifacts，绝不用 runtime）
                          └── service        （组装根：CLI、HTTP、workerd bridge）
```

`ocd` 编译 runtime config，把 workerd 作为受监督 child 拉起，并通过**仅监听回环**的通道通信。它负责 readiness、
优雅停止、重启退避与恢复。

部署是**不可变且内容寻址**的。`workerLoader` 的 key 就是部署身份，因此 promote 与 rollback 只是移动
指针——绝不修改已在运行的东西。

## 它不是什么

在生产里，坦白的边界胜过意外：

- **不是 Cloudflare 全球边缘。** 单节点、跑在你自己的基础设施上——没有 Anycast、没有跨地域复制、没有 POP 网络。而正是这个取舍换来了强本地一致性。
- **不是万能 drop-in。** 兼容性逐 surface 跟踪，每一处偏差都写进文档，而不是含糊过去。
- **不是多副本 HA 集群。** 一个数据目录、一个进程、一台机器——这是设计选择。

## 文档

| 目标              | 从这里开始                                                                                                                                       |
| ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| 理解设计          | [架构与项目指南](https://open-compute.dev/docs/zh/project/)                                                                                      |
| 查看 API 支持     | [兼容性](https://open-compute.dev/docs/zh/platform/compatibility/) · [Worker API 索引](https://open-compute.dev/docs/zh/platform/reference/api/) |
| 查看未支持能力    | [未提供能力](https://open-compute.dev/docs/zh/platform/unsupported/)                                                                             |
| 构建与部署 Worker | [开发应用](https://open-compute.dev/docs/zh/develop/)                                                                                            |
| 下载与发版        | [GitHub Releases](https://github.com/elliothux/open-compute/releases) · [项目指南](https://open-compute.dev/docs/zh/project/)                    |
| 生产运行          | [快速开始](https://open-compute.dev/docs/zh/get-started/) · [运行与运维](https://open-compute.dev/docs/zh/operate/)                              |
| 运维与恢复        | [运行与运维](https://open-compute.dev/docs/zh/operate/) · [事故处理](https://open-compute.dev/docs/zh/ocd/incidents/current-release/)            |
| 参与贡献          | [项目指南](https://open-compute.dev/docs/zh/project/) · [AGENTS.md](AGENTS.md)                                                                   |

## 安全

- 每个数据目录只允许一个 `ocd`——由锁强制，不是靠文档。
- 内部 token 永不出现在 argv、环境变量、日志、status 或 metrics 中。
- 租户出站仅限公网；私有、回环、link-local 和 metadata 地址在地址层直接拒绝。

## 赞助

本项目由 **[Lynx AI](https://lynxai.work)** 赞助。

## License

Apache-2.0。打包的 open-compute workerd fork 仍遵循适用的 upstream Cloudflare workerd 许可证。
