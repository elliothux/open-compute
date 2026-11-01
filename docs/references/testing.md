# 测试

只运行改动影响到的测试。完整 workspace Gate 只在用户要求或发布时运行。只报告实际执行并成功退出的检查。

## 按输入选择测试

| 改动                                                                      | 运行                                                                                                  |
| ------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| 文档、release notes、规则文件、skill                                      | `git diff --check`、`bun scripts/check-docs.ts`；改了摘要输入时同步 `sourceDigest`（见下文）          |
| SDK、Dashboard、Website、JS 工具                                          | 所属包的 typecheck、test 和 build                                                                     |
| Rust 生产代码                                                             | 受影响 crate 的单元测试、受影响的 Gate 目标、`cargo fmt --all --check`、`./test/check-rust-clippy.sh` |
| workerd/Caddy lock、submodule、runtime loader、Cap'n Proto、兼容 baseline | 重建 runtime，运行 `p3-contract` 和受影响目标；换 pin 另按 [workerd](../workerd/README.md) 执行       |

SDK 消费者读取 `packages/sdk/dist/`。SDK 生成面变化时先运行 `bun run --filter @open-compute/sdk build`。

`sourceIdentity()` 只计算 Git 跟踪的文件，排除规则文件、`.agents/**`、`.claude/**`、非嵌入文档（保留 `docs/references/runbooks/**`）、`__pycache__` 和 `test/conformance/baseline.json`；输入变化后更新 baseline 的 `sourceDigest`：

```sh
bun -e 'import {sourceIdentity} from "./test/conformance/checks/context.ts"; console.log(sourceIdentity())'
```

把输出写入 `test/conformance/baseline.json` 的 `sourceDigest`。CI 的路径与产品 Gate 映射在 `test/ci_scope.py`；PR 只执行静态检查和受影响产品，未知或共享路径执行全部产品 Gate。
完整 workspace、90% coverage、打包和安装升级只在 release workflow 执行一轮，CI 不定时运行。

## 准备输入

按[单二进制构建](single-binary.md#构建与打包)准备已验证的 workerd、Caddy、Git LFS 资产和生成资产。输入缺失或校验失败时测试失败；
下载需要用户授权。真实运行时测试需要 `OPEN_COMPUTE_TEST_WORKERD`。`p0-1` 需要 `lsof`。

| 目标               | 额外输入                                                                                                                                                                   |
| ------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `p5-search`        | `OPEN_COMPUTE_TEST_EMBEDDING_API_KEY`、`OPEN_COMPUTE_TEST_EMBEDDING_BASE_URL`，指向本机 OpenAI-compatible fixture                                                          |
| Browser Run（P22） | `bun scripts/prepare-browser.ts --source <chrome-headless-shell 安装> --dest <新目录>`，再设置 `OPEN_COMPUTE_TEST_BROWSER`；CI 版本固定在 `test/browser-fixture.lock.json` |
| `p23-containers`   | `OPEN_COMPUTE_TEST_CONTAINERS_INPUT` 指向 JSON：`containers` 实例配置和两个 OCI 镜像（`manifest` 与 `blobs` 的 `path`、`sha256`）                                          |
| Python（P21）      | 静态输入在 `test/applications/python-*` 和 `test/fixtures/python-*`，Gate 不构建也不下载                                                                                   |

CI 用 `test/containers-fixture/with-ci-linux-engine.sh` 为 Containers 启动独立 Docker Engine。它只能在 GitHub Actions Linux runner 上运行，
需要 44 GiB 空闲空间。细节见脚本注释和 [Containers 运维](containers.md)。

开发时的 Python 构建由 `scripts/build-python.ts` 调用用户安装的 PyWrangler（先找 `.venv/bin/pywrangler`，再找 `PATH`）。

Dashboard 的 Browser Run 绑定测试：启动配置了 Browser Run 的隔离 ocd，设置 `OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL` 为其 `/operator/` 地址
和 `OPEN_COMPUTE_TEST_BROWSER`，运行 `bun run --filter @open-compute/dashboard test:e2e worker-browser-binding.spec.ts`。

## 运行 Gate

`test/gate.py` 需要 Python 3.11+。macOS 上它自动经 `caffeinate -is` 运行。

```sh
./test/gate.py p0-2 --list                 # 查看目标、用例和进程数，不构建
./test/gate.py p0-2 p2-3 --keep-going      # 收集全部失败
./test/gate.py workflow --jobs 2
./test/gate.py --workspace --jobs 2
```

- 修复后只重跑失败和直接受影响的目标。
- `--jobs N` 默认 `min(4, CPU 数)`，每个测试进程内 `--test-threads=1`。生命周期类用例由独占屏障串行执行。
- `test/gate_cases.py` 按完整 Rust 测试名登记用例，必须与实际 discovery 一致。新增、删除或改名测试时同步它。
- 每个目标使用独立 `TMPDIR`。`TMPDIR` 必须在本地磁盘上，不能在 FUSE 挂载上。

| 目标                                                               | 内容                                                                                    |
| ------------------------------------------------------------------ | --------------------------------------------------------------------------------------- |
| `p0-1` … `p0-8`、`p0-exit`                                         | 平台基础、Workers runtime、KV、R2、D1、Durable Objects、调度器                          |
| `p1-conformance`、`p1-security`、`p1-crash`、`p1-snapshot`、`p1-8` | 合同、隔离、崩溃恢复、快照；`p1-8` 是 WebSocket hibernation                             |
| `p2-1`、`p2-2`、`p2-3`、`p2-exit`                                  | 调度器、Queue producer、Queue consumer 与 Cron、产品链                                  |
| `workflow-runtime`、`workflow-recovery`、`workflow-product`        | Workflow 的运行时、恢复和大结果；合称 `workflow`                                        |
| `p21-python-*`                                                     | Python Workers：main、frameworks、services、queues、durable-objects、workflows、runtime |
| `p3-contract`                                                      | baseline、catalog 和能力清单一致性；不需要 workerd 和网络                               |
| `p3-assets`、`p3-services`、`p3-cache-images`                      | 静态资产、Service binding、Cache 与 Images                                              |
| `p3-isolation`、`p3-recovery`                                      | 跨账户隔离、跨产品崩溃恢复                                                              |
| `p5-search`                                                        | AI Search：上传、索引、检索、rerank、Chat、重启                                         |
| `p23-containers`                                                   | Containers 完整 API 与快照恢复                                                          |
| `runtime`、`single-binary`                                         | supervisor、单文件离线启动、单 daemon 双实例                                            |
| `p0`、`p1`、`p2`、`p3`、`all`                                      | 对应集合；多个选择取并集                                                                |

## Python Workers

七个 `p21-python-*` 目标共九个用例。每个用例启动真实 daemon，使用正式 pin 的 workerd、SQLite 和 SigV4 fixture，对比 Python 与
JavaScript 在同一产品上的行为。Main 集中验证 admission、package/import、prepared artifact 完整性、重启和 rollback。
框架只验证 HTTP/WSGI/ASGI；产品用例保留 Python/JS binding、FFI、异常、流和取消，以及 Queue、DO 和 Workflow 的 Python 持久重放。Flask 的 request/app-context 流式响应受上游
[workers-py#287](https://github.com/cloudflare/workers-py/issues/287) 阻塞，Flask 普通 HTTP 仍在测试中。

## 显式资格测试

下列目标不属于 `all` 和 `--workspace`，只在授权后单独运行：

| 目标或命令                                                                       | 前提                                                                                                                     |
| -------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| `./test/gate.py p3-cf-diff`                                                      | 写入真实 Cloudflare 账号。需要授权、专用账号和 cf 凭据；资源名为随机 `oc-p34-*`，结束时逐个删除并复查                    |
| `./test/gate.py p5-ai-provider-qualification --jobs 1`                           | `BAILIAN_API_HOST`、`BAILIAN_API_KEY`、`DEEPSEEK_API_KEY`、`COHERE_API_KEY`；CI 用 `ai-search-qualification` environment |
| `./test/gate.py s3-provider-qualification --jobs 1`                              | 六个 `OPEN_COMPUTE_TEST_R2_S3_*` 变量和 `OPEN_COMPUTE_TEST_R2_S3_MUTATION_ACK=s3-provider-qualification`                 |
| `bun test/conformance/applications/browser-soak.ts <v4 url> <account-id> 600000` | 本机 loopback 实例，至少 2 个并发 action；token 放在 `OPEN_COMPUTE_BROWSER_SOAK_TOKEN`                                   |
| `./test/test-p0-2-egress-linux.sh`                                               | `OPEN_COMPUTE_EGRESS_FIXTURE_ALLOW_SUDO=1`；会修改 loopback 和 `/etc/hosts`                                              |
| `./test/gate.py single-binary` 加 `OPEN_COMPUTE_TEST_OCD`                        | 已授权构建的正式发行文件                                                                                                 |

### 安装与升级

`test/install-lifecycle/` 在 Ubuntu 24.04、Debian 13（x64、ARM64）和 macOS ARM64 上测试真实 systemd/launchd 安装、user 和 system scope、
重启，以及从上一个正式版本升级。输入检查不安装任何东西：

```sh
python3 -B -m unittest discover -s test/install-lifecycle
```

真实场景见 `test/install-lifecycle/run.py --help`，只能在一次性主机上设置 `OPEN_COMPUTE_INSTALL_TEST_DISPOSABLE=1` 后运行。
升级必须成功。有意不兼容的 pre-1.0 版本在 workflow 中设置 `UPGRADE_REJECT_CODE` 和 `UPGRADE_REJECT_REASON`，并在版本说明里写明。

## 完整检查与最终验收

Rust 命令直接使用 `cargo`。

```sh
cargo fmt --all --check
./test/check-rust-clippy.sh
RUSTFLAGS='-D warnings' cargo check --workspace --no-default-features
cargo +1.98.0 check --workspace --all-targets
./test/check-boundaries.sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s test -p 'test_gate.py'
./test/check-production.py
./test/coverage.sh
./test/gate.py --workspace
```

- `./test/coverage.sh` 要求 Rust 行覆盖率不低于 90.00%，报告在 `target/llvm-cov/{html/index.html,lcov.info,summary.json}`。
  缺少 cargo-llvm-cov 时运行 `brew install cargo-llvm-cov` 或 `cargo install cargo-llvm-cov --locked`。
- coverage 排除 `tests/**`、`src/tests.rs`、`src/**/*_tests.rs`、`src/mock_s3.rs` 和 supervisor fixture。不要把生产代码移进排除路径。
- `./test/check-production.py` 构建不含 `test-support` 的二进制并扫描测试标记。

## 验证磁盘预算

- 长构建和 Gate 前后检查空闲空间和 `docker system df -v`。
- 本任务的可丢弃数据不超过 32 GiB。
- Durable Objects 在磁盘用量达到 95% 时拒绝新对象，因此测试数据所在文件系统要保留超过总容量 5% 的空闲。
- Docker 容器用 `--rm`，卷要有明确归属，结束后删除。不要运行全局 `docker prune`。

## 日志与保留证据

报告在 `.temp/gate-run/<run-id>/report.json`，失败的整次运行保留在 `failed/<run-id>/`。P3 运行另写 `contract-report.json`，
`p3-cf-diff` 另写去除凭据的 `diff-report.json`。CI 实时输出 `CI_GATE_PROGRESS` 记录。

先检查失败现场再重试。定位后删除本任务不再需要的 fixture 数据、二进制副本、profile 和详细日志。保留本次运行的 `report.json`。
检查结束后不能留下 workerd 进程、监听端口、临时文件或 secret。

## 本地 smoke

`./scripts/dev-test.sh smoke` 构建资产和 `test-support` 二进制，在 `.temp/r1/` 启动临时 OCD 作用域并创建 `dev` 实例。
端口冲突时设置 `OPEN_COMPUTE_DEV_PORT` 和 `OPEN_COMPUTE_DEV_OCD_ROOT`。`./scripts/dev.sh` 用同一流程，但数据放在 `.data/`。
