# 单二进制分发：实现与本机验收

日期：2026-08-28。状态：内嵌 workerd 的唯一生产分发路径已实现，darwin-arm64 本机验收完成。
这不是正式发布、签名或多平台部署完成记录。

本任务落实“内嵌 workerd，按 Day1 实现，不兼容其他分发方式”。
持续维护的构建、配置和部署步骤见[单二进制分发与部署](../references/single-binary.md)；
该文件是运维参考，不作为已结束的计划归档。本记录只归档本次实现与验证，
不将 [Day1 全仓清理](../day1-architecture-cleanup.md)或
[Runtime 包与测试入口迁移](../runtime-and-test-layout.md)标为完成。

## 已实现契约

| 要求 | 当前实现与证据 |
| --- | --- |
| 一个原生平台可执行文件 | [构建脚本](../../crates/runtime/build.rs)内嵌当前目标的官方 gzip、formal lock、Cap'n Proto 模板和完整系统 Worker 文件集；校验目标、摘要、大小及 manifest |
| 不保留外部 runtime 方式 | [配置](../../crates/core/src/config.rs)删除 `runtime.binary`、`runtime.lock_file`、`runtime.assets_dir`；这些旧字段直接被拒绝，生产下载/安装实现及旧 `package-release` CLI 已删除 |
| 离线、安全物化 | [embedded runtime](../../crates/runtime/src/embedded.rs)只消费编译期 payload；取得 data-dir 排他锁后，私有 staging、校验、fsync、原子无覆盖发布；已有缓存损坏时拒绝，不自动修复或下载 |
| 保留真实进程边界 | workerd 保持官方未修改二进制和受监督子进程；保留编译、readiness、退出、重启、孤儿回收及身份验证；分发单文件不等于运行时没有独立文件 |
| 配置与运维资源随程序提供 | [CLI 资源](../../crates/service/src/resources.rs)内嵌默认配置、许可证和 11 份运维手册；`config init` 输出配置，信息命令不物化 runtime |
| 单文件发行工具 | [发行脚本](../../scripts/package-release.ts)要求干净源码、显式绝对目标及指定 archive 或显式 download，构建并原子发布一个文件；container/systemd/launchd 使用相同运行契约 |
| 临时目录收敛 | 工具缓存、运行目录和失败证据统一到 `.temp/<purpose>/`，只有一条 `/.temp/` ignore；已迁移内容保留，`target/`、依赖目录和持久化 `.data/` 不搬迁 |

生产启动不依赖 Bun、Node、Rust 工具链、源码 checkout 或 PATH 中的 workerd。
配置、业务数据、master key、凭据及 S3 服务仍由部署者提供，不打入发行物。

## 验收基线与产物

- 源码基线：`1f63fd7128d9b56e63f6e78341ee4fed57d735fa` 加本任务所在的未提交工作树。
  不把该 commit 本身或其他并行文档整理视为已包含本次全部实现。
- 最后覆盖率及产品 Gate 期间核对的 583 个构建/测试输入文件保持不变，
  路径与内容摘要汇总为 `edf0917501aca9128f38f4795bed3264e31011147f836d9c30dec3c03aea8ed0`。
- 主机目标：`aarch64-apple-darwin`，运行时目标 `darwin-arm64`。
- 本地 release 文件：`target/release/platformd`，版本 `platformd 0.1.0`。
  大小 **49,175,504 字节（46.90 MiB）**，SHA-256：
  `40d27fbadbdb6429edc41b4fc6ac51121a340c3e52a9c987bb98159651f6cf92`。
- 此本地诊断构建的 `git_revision` 为 `unknown`，不是正式发行身份。
  正式发行脚本注入干净 checkout 的实际 revision，但本任务没有执行该脚本。

| 内嵌输入 | 身份 |
| --- | --- |
| workerd release / version | `v1.20260826.1` / `workerd 2026-08-26` |
| 官方 archive SHA-256 | `22657ec7045a3677b7f52e97f106fe0493add57810687e755e8c6f4fba4b1dba` |
| 官方 binary SHA-256 | `2d17da54d2671d6e9e7c776d56b934f60be8c140b9bac35ddf22f60d6cff9403` |
| formal lock SHA-256 | `d3614a6394cf85e24704954d9e7a9585fb38a2107e8eb73a02519e39add14d2e` |
| runtime assets SHA-256 | `1d24d9292b709ea41482650b26dea9957f79acbe715b0a11b2445f951514f8d5` |

## 已执行验证

构建与测试显式使用现有 `.temp/runtime-cache/v1.20260826.1/` 下的 archive 和 verified binary，
分别设置 `OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE`、`OPEN_COMPUTE_TEST_WORKERD`。
没有为验收下载 runtime、发布文件或执行提权命令。

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --all --check`、`git diff --check` | PASS |
| workspace/all-targets/all-features Clippy，`-D warnings` | PASS；后续测试文件归位的 runtime 定向 Clippy 也通过 |
| `RUSTFLAGS='-D warnings' cargo check --workspace --no-default-features` | PASS |
| Rust 1.98.0 workspace/all-targets MSRV check | PASS |
| Cargo metadata 与依赖边界检查 | PASS |
| `bun run typecheck`、`bun run check:generated`、`bun run test:js` | PASS，JS 测试 44/44 |
| workspace/all-targets/all-features 非插桩测试 | PASS；之后仅作测试文件命名和文档路径调整，相应 5 项 runtime 回归及文档契约测试补检通过 |
| 最新布局的 `./test/coverage.sh` | PASS，退出 0；生产 Rust 行覆盖率 **90.202486%（56,308 / 62,424）**，门槛仍为 90.00% |
| 最新 release 的 `single_binary` 隔离测试 | PASS，2/2，24.54 秒；清空 PATH/环境验证只读命令、首启、锁、重启、孤儿恢复和损坏缓存拒绝 |
| release 的运维资源内容 | 11 份手册与两个许可证文件逐字节匹配当前源文件 |
| 最后执行 `./test/test-p0-1.sh` | PASS，三轮；整个测试目标 529.96 秒 |
| 最后执行 `./test/test-p0-2.sh` | PASS，三轮，测试执行分别为 23.95、23.58、23.32 秒 |
| 收尾进程检查 | 没有遗留本任务的 platformd、workerd 或 Gate 测试进程 |

P0.1 当前仍在测试本体内固定三轮，高级崩溃变体只在第一轮运行；普通 workspace 测试和 coverage
也各自执行了这个三轮目标。这是现有入口的实际重复次数，不是新增覆盖。
最终非插桩 Gate 在完整覆盖率验收之后执行，没有再调用其他递归 aggregate 或退役 G0。

本机日志保留在被忽略的 `.temp/embedded-check.gKNTOd/`，不属于 Git 或发行物：
`workspace-tests-final.log`、`clippy-final.log`、`no-default-features.log`、`msrv.log`、
`embedded-layout-tests.log`、`doc-layout-contract.log`、`release-doc-layout-build.log`、
`single-binary-release-doc-layout.log`、`coverage-doc-layout-final.log`、
`final-p0-1.log`、`final-p0-2.log`。覆盖率报告在 `target/llvm-cov/`。

## 失败记录与边界

- 首次 workspace 运行暴露测试观察窗口问题：冷启动新增解压与验证阶段后，等待 compiled config
  不能只使用编译子进程的 20 秒预算。改用既有的整个平台首启 90 秒预算；生产版本探针、编译和
  子进程超时未放宽。修复后全套测试通过，原现场仍在 `.temp/p0-1-run/failed/`。
- 较早的 release 验证和首轮 coverage 曾出现 workerd 版本探针超时。已有采样观察到 `_dyld_start`
  等待；系统日志显示 XProtect 在对应 staging 身份时间约 23.5 秒后仍访问已被超时清理的文件。
  这支持本机执行安全检查延迟的判断，不等同于证明所有 macOS 超时都由同一原因造成。
  没有停用安全机制、重签 workerd、跳过断言或改超时。诊断及失败日志保留；最新 release、
  无并行构建的完整 coverage 和最后三轮 Gate 均通过。
- 验收期间另有文档目录整理，运维手册移至 `docs/references/runbooks/`。
  保留新布局，更新后的 release 和文档契约均重新验证；最后完整 coverage 使用新路径。
- 本次没有验证 Linux、macOS x64 或容器实际运行，也没有执行需要 sudo 的 Linux egress fixture。
  不能用 darwin-arm64 的结果替代这些平台的验收。
- 本地 platformd 与官方 workerd 的 `codesign --verify --strict` 通过，但只有 ad-hoc 签名；
  没有 Developer ID 签名、公证、正式 release packaging、上传、发布或线上部署。
