<p align="center">
  <a href="https://open-compute.dev">
    <img src="share/open-compute.png" alt="open-compute" width="480" />
  </a>
</p>

<p align="center">
  <strong>单二进制、一键部署的高性能 Cloudflare Workers 兼容基础设施。</strong><br/>
  毫秒级冷启动 · MB 级内存占用 · 零额外依赖。
</p>

<p align="center">
  <a href="https://open-compute.dev">官网</a>
  · <a href="docs/README.md">文档</a>
  · <a href="packages/docs">运维站点</a>
  · <a href="docs/open-compute-workerd-platform.md">架构设计</a>
</p>

<p align="center">
  <a href="README.md">English</a> · 简体中文
</p>

---

## Workers 模型，跑在你自己的硬件上

你已经会写 Cloudflare Workers。open-compute 让同样的 module worker、同样的 binding、同样的 API
跑在一台你自己的机器上。

一个二进制。一个数据目录。一个 S3 endpoint。这就是整个平台。

没有 Kubernetes。没有 Redis。没有服务网格。没有厂商锁定。

## 核心亮点

- **单二进制。** 一个可执行文件装下运行时、控制面和全部产品 binding。拷到主机上、指向一个数据目录——完成。
- **快，因为它是 workerd。** Worker 代码跑在 stock workerd 上——Cloudflare 开源的 V8 运行时。isolate 毫秒级启动、MB 级驻留——不是容器，不是 GB。
- **零额外依赖。** SQLite 存状态，任意 S3-compatible 存储存字节。没有别的要安装，没有别的要运维。
- **天然 self-host。** 数据不出你的机器；部署后完全离线运行。
- **经过验证的兼容。** 同一套测试 fixture 分别跑在 open-compute 和真实 Cloudflare 上。结果不一致，就不发布。

## 用证据说话

- **2,097 个 stable API 成员**覆盖 Workers runtime 和全部产品 binding——已实现、已测试、零缺口。
- **与真实 Cloudflare 行为一致。** Workers、Cache、KV、D1、R2、Durable Objects、Queues 在两个平台上返回相同结果。
- **真实的 Next.js 16 应用两端都能跑。** 同一构建产物，在 open-compute 和 Cloudflare 上行为一致。

## 兼容性

编写标准 module worker（`export default { fetch }`），使用你已熟悉的 binding：

| 模块 | 进度 |
| --- | --- |
| Workers | █████████░ 95% |
| KV | █████████░ 95% |
| R2 | █████████░ 95% |
| D1 | █████████░ 95% |
| Durable Objects | █████████░ 95% |
| Queues | █████████░ 95% |
| Cron | █████████░ 95% |
| Workflows | █████████░ 95% |
| Static Assets | █████████░ 95% |
| Service Bindings | █████████░ 95% |
| Cache | █████████░ 95% |
| Images | █████████░ 95% |
| Version Metadata | ██████████ 100% |
| WebSocket Hibernation | ██████████ 100% |
| Operator API · SDK · Dashboard | 设计中 |

精确支持面：[兼容矩阵](docs/references/cloudflare-compatibility.md) · `ocd capabilities --json`

## 快速开始

本地启动平台（需要 Rust 1.98、Bun 1.3、Node 24 和 pinned workerd 压缩包——见[文档](docs/references/single-binary.md)）：

```sh
export OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE=/abs/workerd-darwin-arm64.gz
bun run build
./scripts/dev.sh
```

部署你的第一个 Worker：

```sh
bun run oc run --config examples/hello-worker/open-compute.json
```

就这样——类型检查、打包、部署、对外服务，一条命令。生产环境中，整个平台就是一个可执行文件加
一个配置文件和一个数据目录；主机上不需要任何构建工具。

## 架构

<p align="center">
  <img src="share/open-compute-architecture.png" alt="open-compute 架构图" width="880" />
</p>

| 组件 | 职责 |
| --- | --- |
| `ocd` | 整个控制面：路由、控制 API、调度器、supervisor |
| `workerd` | 运行时，pin 并校验——你的代码跑在 stock V8 上 |
| SQLite | 本地权威状态——不依赖外部数据库 |
| S3-compatible | 你选的对象存储——bundle、assets、R2 字节 |

租户只能看到自己部署声明的能力。没有 SQLite 路径、没有 S3 凭据、没有内部 token、没有其他租户——永远没有。

## 它不是什么

- **不是 Cloudflare 全球边缘。** 单节点，没有 Anycast，没有跨地域复制。
- **不是所有场景的 drop-in。** 兼容性逐 surface 跟踪，缺口都有文档。
- **不是多副本 HA 集群。** 一个数据目录、一个进程、一台机器。

## 文档

| 目标 | 从这里开始 |
| --- | --- |
| 理解设计 | [架构设计](docs/open-compute-workerd-platform.md) |
| 查看 API 支持 | [兼容矩阵](docs/references/cloudflare-compatibility.md) |
| 构建与部署 Worker | [工具链指南](packages/toolchain/README.md) |
| 生产部署 | [单二进制指南](docs/references/single-binary.md) · [容器 / systemd / launchd](examples/) |
| 运维与恢复 | [运维手册](docs/references/README.md#运维手册) · [运维站点](packages/docs) |
| 参与贡献 | [AGENTS.md](AGENTS.md) · [测试策略](docs/references/testing.md) |

## 安全

- 每个 data dir 只允许一个 `ocd`——由锁强制。
- 内部 token 永不出现在 argv、环境变量、日志或 metrics 中。
- 租户出站仅限公网；私有、回环、metadata 地址在网络层直接拒绝。

## 赞助

本项目由 **[Lynx AI](https://lynxai.work)** 赞助。

## License

Apache-2.0。打包的 `workerd` 仍遵循 upstream Cloudflare workerd 许可证。
