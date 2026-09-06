# 单二进制分发与部署

2026-09-06 正式 lock 已固定用户 fork `b3e1a27840299f493d9425dc4d9972381d02ef23`，
release 为 `v1.20260905.0-open-compute-p1.b3e1a278`，见[workerd 方案](../workerd/README.md)。
四平台优化产物的 archive/binary 摘要、upstream base 与构建输入统一记录于 lock。macOS ARM64 产品验收已通过；四平台 native workerd 证据单列于 P1 实施记录。
四平台原始二进制作为固定依赖保存在 `share/workerd/`，由 Git LFS 管理。
构建工具从这些字节确定性生成正式 gzip；`archiveUrl` 为 `null`，构建不依赖单独发布的 archive。

macOS 的文档解析功能完整保留，但解析子进程尚无可强制执行的内存硬上限。
0.1.0 接受该限制；CPU、输入/输出、并发和超时约束继续生效。
该进程复用同一个 `ocd`，不属于 workerd Worker isolate 的额度，也不增加 sidecar 分发文件。
宿主内存压力仍可能影响主服务，后续工作见 [macOS 内存限制 TODO](../macos-document-parser.md)。


Open Compute 只有一种生产发行形式：按平台构建的单个 `ocd` 可执行文件。
不发布 Rust crate，不提供“外部 workerd”“外部资源目录”或自动下载模式。
`runtime.binary`、`runtime.lock_file`、`runtime.assets_dir` 是未知配置项，启动前即拒绝。

产品支持范围见[兼容矩阵](cloudflare-compatibility.md)；本页只维护构建与运行契约。

## 内嵌内容

- 当前目标平台正式 pin 对应的 workerd gzip；
- 完整多平台 lock、Cap'n Proto 模板、生成的系统 Worker JS 和 manifest；
- 默认 TOML、Open Compute/workerd 许可证及运维手册；
- Rust 中已有的 SQL schema、Xberg MIT license 和其他编译期资源。

TS 源码、Bun、Node、Rolldown、用户 bundle、数据库、master key、S3 与凭据不打入二进制。
许可证用 `ocd licenses` 查看；`ocd docs` 列出手册，
`ocd docs install-and-first-start` 输出指定手册。

## 构建与打包

构建机器需要 Rust 1.98、Bun 1.3.14、Git LFS 和锁定的 workspace 依赖。
每个目标嵌入自己的 archive；不能把一个二进制横跨 OS/CPU 使用。
当前目标为 Darwin ARM64/x64、Linux GNU ARM64/x64。

```sh
git lfs pull --include="share/workerd/**"
bun run build
bun run check:generated
cargo build --locked --release -p open-compute-service --bin ocd
```

根 build 先校验四个平台的 LFS 二进制，用固定 Bun 压缩器生成
`.temp/workerd-build/<target>/<archive-sha256>/<archive-name>`；已存在但损坏的缓存直接拒绝。
Cargo 默认选择编译目标对应的路径；可选的 `OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE` 必须是
同一正式 pin 的绝对路径。它不是运行时覆盖选项。
Cargo build script 检查仓库二进制、目标、压缩包与解压二进制的 SHA-256、大小上限、生成 manifest
、文件集合及源码/锁文件摘要，再把同一批已验证字节编入程序。
检出目录使用 `packages/runtime/`，离线物化仍使用内部 `runtime/`；`dist/` 必须显式构建。没有已校验构建输入时直接报错，不搜索 PATH 或其他缓存。

在干净 checkout 中显式打包当前宿主目标：

```sh
./scripts/package-release.sh --dest /abs/releases/ocd
```

`--dest` 是一个必须不存在的文件，不是目录。默认读取仓库固定二进制；也可通过
`--archive /abs/pinned-workerd.gz` 指定同一 pin 的压缩包。只有明确选择 `--download` 且
formal lock 记录可用发布 URL 时才允许下载；当前未发布的 fork 会直接失败。
生产程序没有 archive 下载代码。
脚本使用原生目标 release 构建，验证 workerd 版本、ocd 版本、源码 revision 和内嵌
release identity，再 fsync、原子无覆盖发布单文件，输出大小与 SHA-256。
不生成相邻资源目录、安装脚本、launcher、兼容布局或第二个服务。

公开发版不靠 maintainer 手工拼装文件。合并 version PR 后，annotated `vX.Y.Z` tag push 触发
GitHub Actions；所有资格校验成功后，四个平台分别运行同一打包脚本和正式单文件测试，聚合生成
`release.json`/`SHA256SUMS`，以 Draft 上传并回读验证，最后才公开 GitHub Release。版本规则、
精确 asset 名称、权限边界和失败处理见[版本与发布流程](releasing.md)。

本地开发、测试和 CI 的输入准备可显式运行
`bun scripts/prepare-workerd.ts --dest /abs/new-build-input`；
它输出构建和底层运行时测试所需的两个环境变量。此工具不分发、不由 ocd 调用。
下载和正式包装属于显式运维操作，不能用作默认本地检查。

CI 构建 job 使用 `actions/checkout` 的 `lfs: true` 检出固定依赖，setup action 离线准备
宿主目标并导出两个环境变量；随后根 build 校验所有目标并生成资产。LFS pointer、摘要不符、
缺少二进制或过期生成资产均失败，不退回 stock。LFS hydration 属于开发依赖获取，生产启动不调用它。
更换依赖必须同步四个平台文件、正式 lock 和验证证据，详见 [固定二进制](../../share/workerd/README.md)。
向 GitHub 推送 LFS 对象、发布 archive 与 release packaging 仍是单独授权的操作。

## 运行契约

1. 把匹配平台的文件安装到固定绝对路径，例如 `/opt/open-compute/ocd`。
2. 用 `config init --data-dir /abs/data` 生成 TOML 到 stdout，保存到新的配置文件，
   选择 Local 或 S3 对象后端，配置监听地址、密钥引用与 admin auth；S3 模式另需 endpoint/bucket 和凭据。
3. `ocd --config /abs/config.toml config check`，然后执行同一路径的 `run`。

对象 authority 是配置选定的 Local 目录或 S3 bucket；S3 模式需要预置 provider。
首次运行自动初始化当前 schema、身份和 key。不要在首次初始化前要求 `doctor --full` 成功。
初始化后的完整 doctor 必须在服务停机时运行，它持有数据目录排他锁并执行 canary/临时 runtime。
`--help`、`--version`、`capabilities`、`docs`、`licenses`、`config init/check`
都不物化 runtime；普通 doctor 只检查内嵌身份和已有缓存。

## 磁盘与进程

```text
ocd（用户下载的唯一文件）
  ├─ data/runtime/packages/<payload-sha256>/
  │    ├─ workerd
  │    └─ runtime/{workerd.lock.json,config.capnp,dist/...}
  ├─ workerd                                  # 常驻、受监督
  └─ ocd __document-parser-v1                 # 每个转换文件一个瞬时自派生 child
```

必须先取得 data-dir 排他锁，才能物化、清理中断的私有 staging、编译与启动。
资源通过同文件系统私有 staging 写入、逐项校验、fsync、原子发布；已有包每次检查，
损坏时拒绝启动且不悄悄覆盖。编译器再次校验实际读取的模板/Worker 字节与内嵌摘要一致。
这些可重建缓存不属于 snapshot authority，业务数据仍在 SQLite/DO 及所选对象后端的既定位置。

workerd 仍是受监督子进程。Linux 执行已验证 fd；macOS 还会创建受日志追踪的临时 executable。
保持现有 readiness、重启、优雅退出、强制回收和孤儿身份验证。
Markdown Conversion 的 parser child 使用同一 `ocd` 文件的隐藏内部模式：清空环境、独立 0700 OS 临时工作目录、
一个 OCDP frame、固定 CPU/address-space/wall/stdout/stderr budget，并由父进程按 process group 终止和回收。
它不初始化配置、data-dir、SQLite、S3、master key、listener 或 workerd，也不是第二个 daemon；Xberg panic/abort
只使当前文件返回稳定错误。support bundle 不采集输入文档、Markdown、pipe 或 child stderr 正文。
运行时磁盘会产生独立文件；“单二进制”指分发物，不指单进程或零磁盘写入。
data-dir 与 macOS staging 所在文件系统必须允许执行，并为解压文件、执行副本及业务状态留足空间。

当前 Linux fork 使用 Ubuntu 22.04 / LLVM 19.1.7 构建，要求 glibc 2.35+；ocd 同时受实际编译主机的 libc 基线约束。
容器示例与 CI 使用 Ubuntu 24.04，不使用 scratch/Alpine。macOS 与 CPU 要求继承
[当前 upstream base 的要求](https://github.com/cloudflare/workerd/tree/dd8133e9b9656fb39f1434247a80aa7a249ee204#running-workerd)。
服务配置见 examples/systemd、examples/launchd 和 examples/container。
只替换并校验完整 ocd，不单独替换缓存中的 workerd 或 JS。

## 验收

`crates/service/tests/single_binary.rs` 把实际程序复制到隔离目录，清空 PATH/环境，
检查只读命令无物化、首次启动、排他锁、重启复用、孤儿恢复及损坏缓存拒绝。
`OPEN_COMPUTE_TEST_OCD=/abs/ocd` 可让这项测试验证正式发布文件。
它不替代真实产品行为、snapshot/restore 和按改动范围选择的最终产品 Gate。
历史 G0 能力调查已经退役，不作为日常或最终验收的必跑项。
任何目标平台的构建、签名或部署未实际验证时，不能把其他平台的通过结果当成它的证据。

当前实现的本机产物、实际验收结果和未验证边界见
[单二进制分发验收记录](../implemented/single-binary-distribution.md)。
