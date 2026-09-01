# open-compute

可单机部署的 Cloudflare Workers Platform 兼容基础设施（`platformd` + pinned `workerd`）。

目标是在一个容器、systemd/launchd service 或前台进程中，提供常用 Workers 编程模型与产品
binding：Workers、Static Assets、Service Binding、KV、R2、D1、Durable Objects、Queues、Cron、
Workflows、Workers Cache、Cache API、Images 和 Version Metadata。平台不复刻 Cloudflare 的全球边缘
网络、跨地域复制、多副本高可用、计费或完整管理面；所有差异必须显式进入 capability/deviation，
不能用产品名称相同推导兼容。

结构化 authority 只使用本机 SQLite；R2、Worker bundle、Static Assets 和大对象使用一个外接
S3-compatible provider 的隔离前缀。不依赖 Redis、Postgres、Kafka、Kubernetes、独立网关或
Node.js production sidecar。

唯一发行物是一个按 OS/CPU 构建的 `platformd`：内嵌正式 pinned workerd 压缩包、
Cap'n Proto、生成的系统 Worker、默认配置、许可证和运维手册。没有外部 runtime 模式、
安装目录资源查找或启动时下载。进程边界仍是一个 platformd 管理一个 workerd child。

Worker、KV、R2、D1、Durable Objects、Queues、Cron、Workflows、Cache 和 Images 的具体支持面与限制以
`platformd capabilities --json` 为准；维护中的偏差见
[capability deviations](docs/references/p1-deviations.md)。部署步骤见
[单二进制分发与部署](docs/references/single-binary.md)。

## Platform contract

平台兼容性按固定 Cloudflare API/config、正式 runtime lock 的唯一 effective compatibility date、正式 stock workerd 和真实产品
Gate 判定。高风险行为使用同一 portable fixture 对比 open-compute 与真实 Cloudflare Workers；
第三方框架只是组合 workload。固定 vinext 可以检验 SSR/RSC、Assets、Service、KV、Cache 与
Images，但 vinext/Next.js 自身缺口不进入平台 schema 或专用分支，vinext 全绿也不能替代 Platform
verdict。完整规则见[平台总方案](docs/open-compute-workerd-platform.md)；P3.4 的 single-latest
types、2,097 个 stable member inventory、完整产品实现和本地 conformance 已闭环，`blocked=0`。
真实 Cloudflare differential 已覆盖 Workers、Cache API、KV、D1、R2、Durable Objects 和 Queues；
Workflow 仍因当前 Wrangler OAuth 对 inventory API 返回 code 10000 而等待独立远端资格。

## Architecture

```text
client / operator
       │
       ▼
platformd ── control/data plane ── SQLite authorities
    │  └────────────────────────── external S3-compatible storage
    │
    └── supervised verified stock workerd
           ├── trusted system Workers
           └── WorkerLoader tenant isolates + declared bindings
```

`platformd` 是唯一 public/control listener 和本地 authority，拥有 data-dir lock、SQLite、S3、
deployment、scheduler、runtime generation 与 child lifecycle。tenant 只获得 immutable deployment
中声明的 capability，不能访问 SQLite path、S3 credential、internal Fetcher/token 或其他租户资源。

## Current status

| 范围 | 当前文档结论 |
| --- | --- |
| P0：Workers/KV/R2/D1/DO/Alarms | 已实现并有归档 Gate；精确支持面仍以 capability/deviation 为准 |
| P1：兼容性/可靠性/单文件 | 核心与本机 Gate 已完成；跨平台发行和长时 soak 仍有 active acceptance |
| P2：Queues/Cron/Workflows | 已实现声明子集并通过 P2 Exit；DO output-gate 等限制仍保留 |
| P3.1：Static Assets | 平台核心与本地 contract/product Gate 完成；当前 remote fixture 只覆盖 Cache API，Assets 直接 differential 与应用 qualification 未评估 |
| P3.2：Service Binding | 本地 hard/product/event-source/SIGKILL recovery 与 contract Gate 完成；当前 remote fixture 只覆盖 Cache API，Service 直接 differential 与应用 qualification 未评估 |
| P3.3：Cache/Images | [声明的单节点支持面已实现并通过最终验收](docs/implemented/p3-3-workers-cache-images.md)；Cache API portable differential 已受控通过，完整 Cloudflare conformance 与应用 qualification 不在该结论内 |
| P3.4：Cloudflare conformance | [全量 Day1 实现与本地 conformance 已完成](docs/implemented/cloudflare-runtime-compatibility.md)：2,097 个 stable members 全部有 evidence，1,585 个 `supported`、512 个 `supported_with_deviation`、`blocked=0`；七项 portable differential 已通过，Workflow 的 Cloudflare 远端资格见[剩余验收](docs/cloudflare-runtime-compatibility-acceptance.md) |

历史设计和结果只证明对应 revision/输入下的范围。当前 dirty working tree、未运行 target 或 active
计划不能从历史 PASS 推导为已验收。

## Workspace

| Path | Role |
| --- | --- |
| `crates/core` | Config, errors, IDs, secrets, clock |
| `crates/storage` | Data dir lock, SQLite, identity, master key |
| `crates/artifacts` | SigV4 S3 store + verified cache |
| `crates/runtime` | workerd lock/verify/compile/supervisor |
| `crates/workers` | WorkerBundle、deployment pipeline、RuntimeSource、dispatch pins |
| `crates/service` | `platformd` CLI、health、control/data plane、workerd bridge |
| `packages/runtime/` | Formal `workerd.lock.json`, Cap'n Proto, system workers |
| `packages/toolchain` | Rolldown + TS7 Worker build/run/deploy CLI |
| `packages/docs` | platformd 运维站点（VitePress；中文默认，英文 `/en/`） |
| `examples/` | Container, systemd, launchd, TypeScript Worker |
| `test/` | Repository test/Gate launchers, coverage, load/soak, fixtures, and fuzz |
| `scripts/` | Local development and release packaging launchers |

未完成的实施与验收方案放在 `docs/`；当前入口包括
[Cloudflare Workflow 远端资格](docs/cloudflare-runtime-compatibility-acceptance.md)、
[P3.1 Static Assets](docs/p3-1-static-assets.md)与
[P3.2 Service Binding](docs/p3-2-service-bindings.md)。已完成的
[Cloudflare Runtime 全量兼容改造](docs/implemented/cloudflare-runtime-compatibility.md)、
[P3.4 conformance](docs/implemented/p3-4-cloudflare-conformance.md)、
[P3.3 Cache/Images](docs/implemented/p3-3-workers-cache-images.md)设计与结果，以及其他已完成阶段见
[docs/implemented](docs/implemented/README.md)，持续维护的 API、测试、部署及运维资料见
[docs/references](docs/references/README.md)。面向 platformd 运维的站点源码在 [packages/docs](packages/docs)。归档不代表对当前工作树重新执行了验收。

## Prerequisites

- Rust 1.98.0 (workspace toolchain and MSRV)
- Bun 1.3.14, Node.js 24, and locked workspace dependencies for TypeScript development/tests (not daemon startup)
- Python 3.11+ for the test scheduler
- macOS or Linux
- 构建时：与目标平台及 `packages/runtime/workerd.lock.json` 匹配的官方 `.gz`，通过绝对路径 `OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE` 显式提供（pin `v1.20260830.1`）
- `rclone` with `serve s3` support for local development
- Local S3-compatible endpoint for real runs (the Gate hosts its own fake S3; protocol is still AWS SDK SigV4)

## Config

`--config` must be an absolute file. Secrets only via `env:` / `file:` / documented `OPEN_COMPUTE_*` and S3 env/file refs. See `share/default-config.toml`.

```sh
platformd config init --data-dir /abs/data > /abs/new-config.toml
# 编辑 S3 endpoint/bucket，提供配置引用的凭据；文件不可覆盖已有配置。
platformd --config /abs/new-config.toml config check
platformd --config /abs/new-config.toml run
# 首次成功运行并停机后，才可执行依赖已有数据库/身份的完整诊断。
platformd --config /abs/new-config.toml doctor --full
```

## Dev / test

Worker 的 TypeScript 用法见 [工具链说明](packages/toolchain/README.md)；
系统 Worker 源码按领域组织，见 [runtime 目录](packages/runtime/README.md)。

本地开发直接运行：

```sh
export OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE=/abs/workerd-darwin-arm64.gz
bun run build
./scripts/dev.sh
```

该脚本启动仅监听 `127.0.0.1:9000` 的 `rclone serve s3`，然后以前台进程启动
`platformd`。所有可持久化开发状态都留在仓库的 ignored `.data/` 中：

```text
.data/
├── s3/                  # rclone S3 root；open-compute/ 是 bucket
├── platform/            # platform data_dir，同时作为 TMPDIR
├── dev-config.toml      # 生成的绝对路径配置
└── rclone-s3.log
```

停止 `platformd` 时脚本同时停止 rclone，但保留上述数据供下次启动复用。
开发构建也必须内嵌正式 archive，不支持 `OPEN_COMPUTE_DEV_WORKERD`。
首次启动完成数据目录初始化后，开发环境检查可运行：

```sh
./scripts/dev.sh config check
./scripts/dev.sh doctor --full
```

测试与 Gate 仍使用测试进程内的 SigV4 S3 provider。开发、审查和修复期间只跑一轮相关 Gate；
实现收尾、源码冻结后才跑最终三轮验收和完整 coverage。单轮命令与并发隔离规则见
[Gate 验证节奏](docs/references/testing.md)。下面列出完整检查和最终 Gate 入口，不是每次中间改动都要重跑的清单：

```sh
export OPEN_COMPUTE_TEST_WORKERD=/abs/verified/workerd
export OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE=/abs/pinned/workerd-platform.gz
export RUSTFLAGS='-D warnings'
bun install --frozen-lockfile --ignore-scripts
bun run build
bun run check:generated
bun run test:js
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --keep-going -- -D warnings
./test/gate.py --workspace
./test/coverage.sh
RUSTFLAGS='-D warnings' cargo check --workspace --no-default-features
cargo metadata --no-deps --format-version 1
./test/check-boundaries.sh
./test/check-production.py
OPEN_COMPUTE_GATE_ROUNDS=3 ./test/gate.py all --jobs 2
```

Rust 覆盖率使用
[`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov)；macOS 可运行
`brew install cargo-llvm-cov`，其他环境可运行
`cargo install cargo-llvm-cov --locked`。`./test/coverage.sh` 使用
`--all-targets --all-features --test-threads=1` 口径，要求提供与正式 lock 匹配的 pinned
`workerd`，并执行真实 P0.1-P0.8 Rust Gate 路径。脚本在 `target/llvm-cov/` 生成终端摘要、
HTML、LCOV 和 JSON summary；workspace 生产 Rust 行覆盖率低于 90.00% 时失败。独立的
`test/**`、`tests/**`、`src/tests.rs`、`src/**/*_tests.rs`、`src/mock_s3.rs` 和测试 supervisor fixture
不计入分母，生产代码不得放入这些排除路径。coverage 每个测试本体只执行一次，
不替代最后的三轮独立进程验收。G0 一次性探测已移除，历史证据保留在 `docs/implemented/`。

`./test/gate.py p0-2 p2-3 --jobs 2` 默认单轮，并将两个名字指向的同一 Worker/Queue/Cron
矩阵只执行一次。`p0`、`p1`、`p2`、`all` 选择对应目标集合，不递归调用其他 Gate。
所有消费运行时资产的命令必须先显式 `bun run build`；`packages/runtime/dist/` 不提交 Git。
缺少 binary/archive、摘要不符或资产过期直接失败，不跳过、不下载。

Linux release CI 另以 `test/test-p0-2-egress-linux.sh` 创建短期受控 dual-stack 网络夹具，补齐
public IPv4/IPv6/DNS allow 与 redirect/DNS-to-private deny；该脚本会要求显式 sudo 授权并在退出时
清理地址和 hosts 项。

## 单文件发布

在干净源码上生成宿主平台的一个可执行文件；目标文件必须不存在：

```sh
./scripts/package-release.sh --dest /abs/releases/platformd --archive /abs/pinned-workerd.gz
# 仅在显式允许下载时，使用 --download 替代 --archive。
```

运行端只需要这个文件、用户配置/凭据、可写 data-dir 和 S3 authority。
不需要 Rust、Bun、Node、TypeScript 或相邻资源目录。运行时会在
`data/runtime/packages/<payload-sha256>/` 物化校验过的 workerd 和系统资源；
这不是“运行时磁盘上也只有一个文件”。详情见 [分发契约](docs/references/single-binary.md)。

## Operations

- Container: `examples/container/` — non-root, `platformd` as PID 1, writable data volume, read-only runtime.
- systemd: `examples/systemd/open-compute.service` — `KillMode=control-group`; restart on process/liveness failure, not on readiness 503.
- launchd: `examples/launchd/io.opencompute.platformd.plist`.

Never embed credentials in units/images; use env or file refs from config.

## Security / platform boundaries

- One `platformd` per data dir (`DATA_DIR_IN_USE`).
- Internal workerd token is not on argv/env/logs/status/metrics.
- `/health/live` is liveness only; `/health/ready` is admission and must not restart the process.
- tenant 仅能访问部署中明确声明的产品 binding；平台不提供 multi-node HA。
- tenant outbound 仅为 HTTP(S) fetch，并由 pinned workerd 的 `Network(allow=["public"])`
  拒绝 private/local/metadata 网络目标。
- Next-start orphan recovery uses a secret-free child lease (PID + start identity + binary digest) and will not signal a reused PID.

## License

Apache-2.0. Packaged `workerd` remains upstream Cloudflare workerd.
