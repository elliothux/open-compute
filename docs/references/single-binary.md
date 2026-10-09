# 单二进制分发与部署

当前正式 lock 固定用户 fork `e98a3e8433979356047a202d3e0d1b0e2e2c4b8c`，
release 为 `v1.20260930.0-open-compute-r4.e98a3e843`，见[workerd 方案](../workerd/README.md)。
三个正式产品平台加 macOS Intel 手动输入的 archive/binary 摘要、upstream base 与构建输入统一记录于 lock。
四个平台的 gzip 由 workerd fork 的 GitHub Release 托管，lock 固定 URL、archive/binary digest 与构建身份；macOS Intel
仍不进入官方 `ocd` release。Caddy 使用独立的 `elliothux/open-compute-caddy` 源码 submodule、Release 与
[`caddy.lock.json`](../../packages/runtime/caddy.lock.json)，不与 `ocd` 产品 Release 混用。
平台无关的 Pyodide `314.0.6_2026-08-17_6` Cap'n Proto bundle 以固定 gzip 保存在
`share/pyodide/` 并由同一 lock 记录压缩与解压 SHA-256；它同样通过 Git LFS 进入构建输入。

macOS 的文档解析功能完整保留，但解析子进程尚无可强制执行的内存硬上限。
0.1.0 接受该限制；CPU、输入/输出、并发和超时约束继续生效。
该进程复用同一个 `ocd`，不属于 workerd Worker isolate 的额度，也不增加 sidecar 分发文件。
宿主内存压力仍可能影响主服务；这是当前明确接受的支持边界。

Open Compute 只有一种生产发行形式：按平台构建的单个 `ocd` 可执行文件。
不发布 Rust crate，不提供“外部 workerd”“外部资源目录”或自动下载模式。
`runtime.binary`、`runtime.lock_file`、`runtime.assets_dir` 是未知配置项，启动前即拒绝。

产品支持范围见[兼容矩阵](cloudflare-compatibility.md)；本页只维护构建与运行契约。

可选 Browser Run 使用 operator 安装的完整 `chrome-headless-shell`，不嵌入 `ocd`，也不改变正式 workerd/Caddy 的内嵌契约。
managed 使用 Chrome 原生 sandbox 和平台 CDP 权限边界，不叠加外层 sandbox；安装与实例配置见
[首次启动手册](runbooks/install-and-first-start.md)。外部 CDP 模式由 operator 管理浏览器进程及其访问权限。

## 内嵌内容

- 当前目标平台正式 pin 对应的 workerd gzip；
- 当前目标平台正式 pin 对应的定制 Caddy binary，包含 `dns.providers.opencompute`；
- 正式 pin 对应的 Pyodide bundle gzip；
- 静态 Tesseract/Leptonica，以及固定 `eng`、`chi_sim`、`chi_tra` tessdata 的确定性 gzip；
- 完整多平台 lock、Cap'n Proto 模板、生成的系统 Worker JS 和 manifest；
- 默认 TOML、Open Compute/workerd 许可证及运维手册；
- Rust 中已有的 SQL schema、Xberg MIT license 和其他编译期资源。

TS 源码、Bun、Node、Rolldown、用户 bundle、数据库、master key、S3 与凭据不打入二进制。
许可证用 `ocd licenses` 查看；`ocd docs` 列出手册，
`ocd docs install-and-first-start` 输出指定手册。

## 构建与打包

构建机器需要 Rust 1.98、Bun 1.3.14、Git LFS 和锁定的 workspace 依赖。
每个目标嵌入自己的 archive；不能把一个二进制横跨 OS/CPU 使用。
当前正式目标为 Darwin ARM64、Linux GNU ARM64/x64。

### Windows 和 macOS Intel 手动编译

这两个平台不进入官方 `ocd` release 矩阵，也不会出现在 GitHub Release。维护者需要在目标机安装
Rust 1.98、Bun 1.3.14 和 Git LFS，按 lock 显式准备匹配输入，再完成 `ocd` 编译、启动与 Gate 验证。
macOS Intel 可使用 lock 中的 `darwin-x64` archive。Windows 需要使用适用的 Rust target
和本机编译的 workerd；仓库不提供 Windows 的预构建 archive、交叉编译配置或兼容性保证。

```sh
cargo install mbx --version 1.22.0 --locked
git submodule update --init --depth 1 third_party/gitserver third_party/workerd
git lfs pull --include="share/pyodide/**,share/tessdata/**,share/xberg-tesseract-cache/**"
bun install --frozen-lockfile --ignore-scripts
runtime_inputs=$(mktemp -d "$PWD/.temp/runtime-inputs.XXXXXX")
eval "$(bun scripts/prepare-workerd.ts --dest "$runtime_inputs/workerd" --download)"
eval "$(bun scripts/prepare-caddy.ts --dest "$runtime_inputs/caddy" --download)"
bun run build
bun run check:generated
mbx build --locked --release -p open-compute-service --bin ocd
```

准备工具只在显式 `--download` 时获取 lock 指定的 GitHub Release asset，并校验 URL、大小、archive/binary digest、版本与
Caddy module；已存在目标拒绝覆盖。根 build 校验 host 输入、四目标 lock、平台无关 Pyodide gzip和静态 OCR source inputs。
Cargo 要求 `OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE` 与 `OPEN_COMPUTE_BUILD_CADDY` 指向同一正式 pin 的绝对路径；它们不是运行时覆盖选项。
runtime 的 INTERNAL host policy 在构建时复用 `third_party/workerd` 中 canonical Workers、Workflows 与 AsyncLocalStorage
wrapper source；这些源码也进入 manifest 与 Cargo 的输入摘要检查。CI 和干净 checkout 的构建须显式初始化固定 gitlink，
不在 runtime build 或 Cargo 中自动获取源码。它们是构建输入，不是生产启动时的依赖。

Cargo build script 检查目标、workerd/Pyodide/Caddy 压缩包与解压字节的 SHA-256、大小上限、生成 manifest
、文件集合及源码/锁文件摘要，再把同一批已验证字节编入程序。
检出目录使用 `packages/runtime/`，daemon 离线物化到 `<OCD_DIR>/cache/packages/`；`dist/` 必须显式构建。没有已校验构建输入时直接报错，不搜索 PATH 或其他缓存。

在干净 checkout 中显式打包当前宿主目标：

```sh
./scripts/package-release.sh --dest /abs/releases/ocd
```

`--dest` 是一个必须不存在的文件，不是目录。打包脚本显式准备 lock 固定的 workerd 与 Caddy Release asset；也可通过
`--archive /abs/pinned-workerd.gz` 指定同一 pin 的 workerd 压缩包。只有明确选择 `--download` 才允许下载。
生产程序没有 archive 下载代码。
脚本使用原生目标 release 构建，验证 workerd 版本、ocd 版本、源码 revision 和内嵌
release identity，再 fsync、原子无覆盖发布单文件，输出大小与 SHA-256。
不生成相邻资源目录、安装脚本、launcher、兼容布局或第二个服务。

公开发版不靠 maintainer 手工拼装文件。合并 version PR 后，annotated `vX.Y.Z` tag push 触发
GitHub Actions；所有资格校验成功后，三个正式平台分别运行同一打包脚本和正式单文件测试，聚合生成
`release.json`/`SHA256SUMS`，以 Draft 上传并回读验证，最后才公开 GitHub Release。版本规则、
精确 asset 名称、权限边界和失败处理见[版本与发布流程](releasing.md)。

本地开发、测试和 CI 的输入准备可显式运行
`bun scripts/prepare-workerd.ts --dest /abs/new-workerd --download` 与
`bun scripts/prepare-caddy.ts --dest /abs/new-caddy --download`；
它们输出构建和底层运行时测试所需的环境变量，不由 ocd 调用。
下载和正式包装属于显式运维操作，不能用作默认本地检查。

CI 的 `lfs: true` 只检出 Pyodide/OCR 等保留的固定依赖；setup action 从两个 lock 显式下载、验证并导出 host
workerd/Caddy。缺失 asset、摘要不符或过期生成资产均失败，不退回 stock。生产启动不联网。
更换依赖必须同步对应 submodule gitlink、正式 lock、Release asset 与验证证据。

## 运行契约

1. 把匹配平台的文件安装到固定绝对路径，例如 `/opt/open-compute/ocd`。
2. 用 `ocd setup --yes` 初始化用户级 OCD_DIR、共享 listener 和唯一全局 admin 凭证；或由 operator 创建等义的 `ocd.toml`。系统级作用域须显式 `--system`。
3. 用 `ocd instance setup --config /abs/compute.toml --data-dir /abs/data --yes` 创建并登记实例；已有配置先执行 `ocd --config /abs/compute.toml config check`，再用 `ocd instance add --config /abs/compute.toml` 登记。`compute.toml` 必须显式包含 `[data].path`，Local 或 S3 对象后端及实例凭证属于该文件；共享监听和 admin 凭证只属于 `ocd.toml`。S3 模式还需 endpoint/bucket 和凭据。
4. 执行 `ocd run`（系统作用域使用 `ocd --system run`）。`run` 不从 cwd、`--config` 或 `instances/` 扫描实例。

对象 authority 是配置选定的 Local 目录或 S3 bucket；S3 模式需要预置 provider。
实例创建时初始化当前 schema、身份和 key。不要在首次初始化前要求 `doctor --full` 成功。
初始化后的完整 doctor 必须在服务停机时运行，它持有数据目录排他锁并执行 canary/临时 runtime。
`--help`、`--version`、`capabilities`、`docs`、`licenses`、`config init/check`
都不物化 runtime；普通 doctor 只检查内嵌身份和已有缓存。

## 磁盘与进程

```text
ocd（用户下载的唯一文件）
  ├─ OCD_DIR/cache/packages/<payload-sha256>/
  │    ├─ workerd
  │    ├─ caddy
  │    ├─ pyodide-bundle-cache/pyodide_314.0.6_2026-08-17_6.capnp.bin
  │    └─ runtime/{workerd.lock.json,config.capnp,dist/...}
  ├─ INSTANCE_DIR/runtime/config.*           # 每实例编译配置、lease 与 staging
  ├─ INSTANCE_DIR/tessdata/<contract-sha256>/ # OCR 语言资产，逐项复验
  ├─ 每实例一个 workerd                       # 常驻、独立受监督
  ├─ 一个 caddy                              # Gateway 启用时常驻、受监督
  └─ ocd __document-parser-v1                 # 每个转换文件一个瞬时自派生 child
```

必须先取得 OCD_DIR 排他锁才能物化共享包；每个实例在编译与启动前取得其数据目录锁。共享包只物化一次，实例及 Gateway 复用同一份已验证输入；每实例的编译缓存和进程 lease 仍留在各自数据目录。
workerd 启动参数固定指向上述私有 Pyodide cache；当前认证日期 `2026-09-08` 的 Python child 首次执行
直接加载该 bundle。其它官方 child 日期/flag 组合仍由 workerd 原生兼容规则处理。
资源通过同文件系统私有 staging 写入、逐项校验、fsync、原子发布；已有包每次检查，
损坏时拒绝启动且不悄悄覆盖。编译器再次校验实际读取的模板/Worker 字节与内嵌摘要一致。
这些可重建缓存不属于 snapshot authority，业务数据仍在 SQLite/DO 及所选对象后端的既定位置。

workerd 仍是受监督子进程。Linux 执行已验证 fd；macOS 的临时 executable 位于所属实例的私有 staging。其 cwd、HOME、XDG cache 和 TMPDIR/TMP/TEMP 都指向本实例目录；受管 Provider 同样使用本实例内的私有输出根。
保持现有 readiness、重启、优雅退出、强制回收和孤儿身份验证。
Markdown Conversion 的 parser child 使用同一 `ocd` 文件的隐藏内部模式：清空环境、`INSTANCE_DIR/tmp/` 下独立 0700 工作目录、
一个 OCDP frame、固定 CPU/address-space/wall/stdout/stderr budget，并由父进程按 process group 终止和回收。
它不初始化配置、data-dir、SQLite、S3、master key、listener 或 workerd，也不是第二个 daemon；Xberg panic/abort
只使当前文件返回稳定 `DOCUMENT_PROCESS_FAILED`。Xberg cache resolution 被限制在临时工作目录，Tesseract result cache
明确关闭，且 `RLIMIT_FSIZE=0` 不放宽。父进程日志只保存失败类、exit code/signal、bounded byte count 和 stderr digest；
support bundle 不采集输入文档、Markdown、pipe 或 child stderr 正文。
运行时磁盘会产生独立文件；“单二进制”指分发物，不指单进程或零磁盘写入。
data-dir 与 macOS staging 所在文件系统必须允许执行，并为解压文件、执行副本及业务状态留足空间。

当前 Linux fork 使用 Ubuntu 24.04 runner 与 Bazel `release_linux` 的 LLVM 22 toolchain 构建，二进制符号基线要求 glibc 2.38+；ocd 同时受实际编译主机的 libc 基线约束。
容器示例与 CI 使用 Ubuntu 24.04，不使用 scratch/Alpine。macOS 与 CPU 要求继承
[正式 pin upstream base 的要求](https://github.com/cloudflare/workerd/tree/cb26acc62e64f64487a78e3f73d0a08e9926c690#running-workerd)。
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
[单二进制分发验收记录](../implemented/p2-6-single-binary-distribution.md)。
