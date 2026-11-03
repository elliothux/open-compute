# 测试

只运行受影响的测试；完整 workspace Gate 只在用户要求或发布时运行。

## 按输入选择测试

| 改动                                                                      | 运行                                                                                                  |
| ------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| 文档、release notes、规则文件、skill                                      | `git diff --check`、`bun scripts/check-docs.ts`；改了摘要输入时同步 `sourceDigest`（见下文）          |
| SDK、Dashboard、Website、JS 工具                                          | 所属包的 typecheck、test 和 build                                                                     |
| Rust 生产代码                                                             | 受影响 crate 的单元测试、受影响的 Gate 目标、`cargo fmt --all --check`、`./test/check-rust-clippy.sh` |
| workerd/Caddy lock、submodule、runtime loader、Cap'n Proto、兼容 baseline | 重建 runtime，运行 `contract` 和受影响目标；换 pin 另按 [workerd](../workerd/README.md) 执行          |

SDK 消费者读取 `packages/sdk/dist/`。SDK 生成面变化时先运行 `bun run --filter @open-compute/sdk build`。

Dashboard builds stage assets in `.temp/dashboard-build/` and copy changed bytes or paths into `apps/dashboard/dist/`.
Identical builds preserve file and directory mtimes, so Cargo keeps the embedded Rust assets fresh.

`sourceIdentity()` 只计算 tracked 文件；排除规则、`.agents/**`、`.claude/**`、非嵌入文档、`__pycache__` 和 baseline，保留嵌入的 runbooks。输入变化后更新摘要：

```sh
bun -e 'import {sourceIdentity} from "./test/conformance/checks/context.ts"; console.log(sourceIdentity())'
```

输出写入 `test/conformance/baseline.json.sourceDigest`。PR 按 `test/ci_scope.py` 运行静态检查与受影响产品；共享或未知路径选全部产品。
完整 workspace、90% coverage、打包和安装升级只在 release workflow 执行一轮，CI 不定时运行。

## 准备输入

按[构建说明](single-binary.md#构建与打包)准备 workerd、Caddy、LFS 和生成资产；下载须授权。真实运行时需要 `OPEN_COMPUTE_TEST_WORKERD`，`daemon` 还需要 `lsof`。

| 目标               | 额外输入                                                                                                                                                                   |
| ------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `search`           | `OPEN_COMPUTE_TEST_EMBEDDING_API_KEY`、`OPEN_COMPUTE_TEST_EMBEDDING_BASE_URL`，指向本机 OpenAI-compatible fixture                                                          |
| Browser Run（P22） | `bun scripts/prepare-browser.ts --source <chrome-headless-shell 安装> --dest <新目录>`，再设置 `OPEN_COMPUTE_TEST_BROWSER`；CI 版本固定在 `test/browser-fixture.lock.json` |
| `containers`       | `OPEN_COMPUTE_TEST_CONTAINERS_INPUT` 指向 JSON：`containers` 实例配置和两个 OCI 镜像（`manifest` 与 `blobs` 的 `path`、`sha256`）                                          |
| Python（P21）      | 静态输入在 `test/applications/python-*` 和 `test/fixtures/python-*`，Gate 不构建也不下载                                                                                   |

CI 的 `test/containers-fixture/with-ci-linux-engine.sh` 只用于 GitHub Actions Linux runner，需要 44 GiB 空闲空间；见 [Containers 运维](containers.md)。

开发时的 Python 构建由 `scripts/build-python.ts` 调用用户安装的 PyWrangler（先找 `.venv/bin/pywrangler`，再找 `PATH`）。

Dashboard 的 Browser Run 绑定测试：启动配置了 Browser Run 的隔离 ocd，设置 `OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL` 为其 `/operator/` 地址
和 `OPEN_COMPUTE_TEST_BROWSER`，运行 `bun run --filter @open-compute/dashboard test:e2e worker-browser-binding.spec.ts`。

## 运行 Gate

`test/gate.py` 需要 Python 3.11+。macOS 上它自动经 `caffeinate -is` 运行。

```sh
./test/gate.py workers --list # 列出计划，不构建
./test/gate.py workers queues --keep-going # 收集失败
./test/gate.py workflow --jobs 2
./test/gate.py --workspace --jobs 2
```

- 修复后只重跑失败和直接受影响的目标。
- `--jobs N` 默认 `min(4, CPU 数)`，每个测试进程内 `--test-threads=1`。生命周期类用例由独占屏障串行执行。
- `test/gate_cases.py` 按完整 Rust 测试名登记用例，必须与实际 discovery 一致。新增、删除或改名测试时同步它。
- 每个目标使用独立 `TMPDIR`。`TMPDIR` 必须在本地磁盘上，不能在 FUSE 挂载上。

### 目标名称迁移

旧名报错，无别名；Rust binary/case ID 不变。目标取并集，`--list` 显示展开。重复组已并入 `workers`。

| 旧名                           | 新名                        |
| ------------------------------ | --------------------------- |
| `p0-1`                         | `daemon`                    |
| `p0-2`                         | `workers`                   |
| `p0-3`                         | `bindings`                  |
| `p0-4`                         | `kv`                        |
| `p0-5`                         | `r2`                        |
| `p0-6`                         | `d1`                        |
| `p0-7`                         | `durable-objects`           |
| `p0-8`                         | `alarms`                    |
| `p0-exit`                      | `core-products`             |
| `p1-conformance`               | `capabilities`              |
| `p1-security`                  | `isolation`                 |
| `p1-crash`                     | `crash-recovery`            |
| `p1-snapshot`                  | `snapshots`                 |
| `p2-1`                         | `scheduler`                 |
| `p2-2`                         | `queues`                    |
| `p2-exit`                      | `queue-workflow-recovery`   |
| `p3-assets`                    | `assets`                    |
| `p3-services-hard`             | `services-rpc`              |
| `p3-services-product`          | `services-product`          |
| `p3-services-events`           | `services-events`           |
| `p3-services-recovery`         | `services-recovery`         |
| `p3-cache-images`              | `cache-images`              |
| `p5-search`                    | `search`                    |
| `p3-recorded`                  | `recorded`                  |
| `p6-cf-resources`              | `cf-resources`              |
| `p22-browser-run`              | `browser`                   |
| `p23-containers`               | `containers`                |
| `p6-cloudflare-sdk`            | `cloudflare-sdk`            |
| `p20-cf-cli`                   | `cf-cli`                    |
| `p21-python-main`              | `python-main`               |
| `p21-python-frameworks`        | `python-frameworks`         |
| `p21-python-services`          | `python-services`           |
| `p21-python-queues`            | `python-queues`             |
| `p21-python-durable-objects`   | `python-durable-objects`    |
| `p21-python-workflows`         | `python-workflows`          |
| `p21-python-runtime`           | `python-runtime`            |
| `p3-contract`                  | `contract`                  |
| `p3-cf-diff`                   | `cf-diff`                   |
| `p5-ai-provider-qualification` | `ai-provider-qualification` |
| `p0`                           | `core`                      |
| `p1`                           | `platform`                  |
| `p2`                           | `scheduling`                |
| `p3`                           | `products`                  |
| `p3-services`                  | `services`                  |
| `p3-isolation`                 | `product-isolation`         |
| `p3-recovery`                  | `product-recovery`          |
| `p2-3`                         | `workers`                   |
| `p1-8`                         | `hibernation`               |
| `p5`                           | `search-qualification`      |

## 录制的 Cloudflare 合同

`./test/gate.py recorded` 用隔离 ocd、本地 S3 和临时目录运行全部 portable fixtures；复用 differential 归一化，与 `contract.json` 比较，只访问 loopback，无须 Cloudflare 凭据。PR 产品/runtime 改动会选择它。

发版前用 `./test/gate.py cf-diff --refresh` 刷新；需要 `OPEN_COMPUTE_CF_MUTATION_ACK=cf-diff`、其余 `OPEN_COMPUTE_CF_*` 和本地 endpoint/account/admin token。全部选中 fixture 的 Cloudflare/ocd 观测一致且清理成功后，只写变化的合同，并在 `baseline.json.portableRecording` 记日期、cf 版本和 alias，更新摘要；失败不改录制。

## Python Workers

七个 `python-*` 目标共九个用例，用正式 workerd、SQLite 和 SigV4 fixture 比较 Python/JS。Main 测 admission、package/import、preparation 完整性与恢复。
框架测 HTTP/WSGI/ASGI；产品测 binding/FFI、异常、流/取消及持久重放。Flask 普通 HTTP 保留，streaming 阻塞见 [workers-py#287](https://github.com/cloudflare/workers-py/issues/287)。

## 显式资格测试

下列目标不属于 `all` 和 `--workspace`，只在授权后单独运行：

| 目标或命令                                                                       | 前提                                                                                                                     |
| -------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| `./test/gate.py cf-diff`                                                         | 写入真实 Cloudflare 账号。需要授权、专用账号和 cf 凭据；资源名为随机 `oc-p34-*`，结束时逐个删除并复查                    |
| `./test/gate.py ai-provider-qualification --jobs 1`                              | `BAILIAN_API_HOST`、`BAILIAN_API_KEY`、`DEEPSEEK_API_KEY`、`COHERE_API_KEY`；CI 用 `ai-search-qualification` environment |
| `./test/gate.py s3-provider-qualification --jobs 1`                              | 六个 `OPEN_COMPUTE_TEST_R2_S3_*` 变量和 `OPEN_COMPUTE_TEST_R2_S3_MUTATION_ACK=s3-provider-qualification`                 |
| `bun test/conformance/applications/browser-soak.ts <v4 url> <account-id> 600000` | 本机 loopback 实例，至少 2 个并发 action；token 放在 `OPEN_COMPUTE_BROWSER_SOAK_TOKEN`                                   |
| `./test/test-p0-2-egress-linux.sh`                                               | `OPEN_COMPUTE_EGRESS_FIXTURE_ALLOW_SUDO=1`；会修改 loopback 和 `/etc/hosts`                                              |
| `./test/gate.py single-binary` 加 `OPEN_COMPUTE_TEST_OCD`                        | 已授权构建的正式发行文件                                                                                                 |

### 安装与升级

`test/install-lifecycle/` 测 Linux systemd/macOS launchd 的 user/system 安装、重启和升级；平台矩阵见 [发布流程](releasing.md)。输入检查：

```sh
python3 -B -m unittest discover -s test/install-lifecycle
```

真实场景只在一次性主机设置 `OPEN_COMPUTE_INSTALL_TEST_DISPOSABLE=1` 后运行；见 runner 的 `--help`。不兼容的 pre-1.0 升级须设置 `UPGRADE_REJECT_CODE`/`UPGRADE_REJECT_REASON` 并写入版本说明。

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
- Release 将 coverage 分为四片：`./test/coverage.sh --shard 1/4 --jobs 2`，其余片分别使用 `2/4`、`3/4`、`4/4`。
  `test/gate.py` 定义唯一分区；新 Cargo 目标未分配时，分区检查失败。
  每片在 `.temp/coverage/run-*` 保留原始 profile、本片测试的精确 object 和共享子进程 binary。
  用 `./test/coverage.sh merge <run-1> <run-2> <run-3> <run-4>` 合并四片，合并后才检查 90.00% 门槛。
  merge 只需要相同 Rust toolchain 的 LLVM 工具，不构建，也不需要 runtime 输入。
- coverage 排除 `tests/**`、`src/tests.rs`、`src/**/*_tests.rs`、`src/mock_s3.rs` 和 supervisor fixture。不要把生产代码移进排除路径。
- `./test/check-production.py` 构建不含 `test-support` 的二进制并扫描测试标记。

## 验证磁盘预算

- 长构建和 Gate 前后检查空闲空间和 `docker system df -v`。
- 本任务的可丢弃数据不超过 32 GiB。
- Durable Objects 在磁盘用量达到 95% 时拒绝新对象，因此测试数据所在文件系统要保留超过总容量 5% 的空闲。
- Docker 容器用 `--rm`，卷要有明确归属，结束后删除。不要运行全局 `docker prune`。

## 日志与保留证据

报告在 `.temp/gate-run/<run-id>/report.json`；失败现场移至 `failed/<run-id>/`，合同证据在 `contract-report.json`。CI 输出 `CI_GATE_PROGRESS`。
定位后清理大产物并保留报告；结束后确认无 workerd 进程、监听端口或 secret。

## 本地 smoke

`./scripts/dev-test.sh smoke` 构建资产和 `test-support` 二进制，在 `.temp/r1/` 启动临时 OCD 作用域并创建 `dev` 实例。
端口冲突时设置 `OPEN_COMPUTE_DEV_PORT` 和 `OPEN_COMPUTE_DEV_OCD_ROOT`。`./scripts/dev.sh` 用同一流程，但数据放在 `.data/`。
