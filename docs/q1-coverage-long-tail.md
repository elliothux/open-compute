# Q1：Workspace 行覆盖长尾补齐专项

状态：**TODO**（2026-09-14 立项）。本文跟踪把 workspace Rust 行覆盖从当前
90.0% 基线提升到 91% 以上所需的既有长尾测试补齐。它是 [W2 Standard limits
实施](implemented/w2-standard-limits.md) 验收时留下的质量专项：W2 自身新增代码
（三个 workerd 隔离模块、supervisor functional watchdog、limits authority 链、
bridge 证据分类）已全部带测试并通过；本文处理的是与 W2 无关、代码库快速扩张期
（2026-09-03 之后 crates 净增约 125k 行）积累下来的覆盖长尾。

## 1. 基线与目标

| 项         | 值                                                                         |
| ---------- | -------------------------------------------------------------------------- |
| 测量日期   | 2026-09-14                                                                 |
| 测量输入   | `./test/coverage.sh` 一轮完整插桩 workspace Gate（54 个插桩目标全部 PASS） |
| 当前行覆盖 | 90.02%（147,208 行，missed 14,719）                                        |
| 仓库底线   | 90.00%（`test/coverage.sh` 强制，`AGENTS.md` 不允许下调）                  |
| 本专项目标 | ≥ 91.00%                                                                   |
| 缺口       | 约 1,440 行                                                                |

2026-09-14 当天的提升（89.89% → 90.02%）来自 12 个新测试，已在 W2 验收中落地：
scheduler/vectorize legacy 采纳（含撕裂态与漂移身份 fail-closed）、
`EffectiveResourceLimitsV1` 边界矩阵、`control_identity`、`update_do_storage_health`
水位三态、supervisor `begin_drain` 两个生命周期场景、instance registry 校验矩阵、
schema inspection 扩展到 vectorize/ai-search 遗留库、worker_loaders 传输失败证据
分类、模型 parser 往返与 watchdog 常量冻结。

## 2. 结构性不可覆盖项（不计数）

以下缺口由测试设计本身决定，不能用普通测试消除，评估 91% 时应从分母扣除或单列：

| 文件                                                                  | 缺口 | 原因                                                                                                |
| --------------------------------------------------------------------- | ---: | --------------------------------------------------------------------------------------------------- |
| `crates/workers/src/workflows/workflow_tests/crash_matrix/durable.rs` | ~167 | crash-matrix 子进程按设计被 SIGKILL，进程被杀时 LLVM profile 无法落盘，子进程执行的代码永远无法计入 |
| `crates/artifacts/src/git_repo.rs`（clone 主路径）                    |  ~70 | `import_public_https` 要求公网 HTTPS remote；本地 fixture 只能覆盖校验与错误分类                    |

## 3. 按优先级分组的补齐清单

以下数据来自 2026-09-14 `target/llvm-cov/lcov.info`，按“缺口行数 × fixture 成本”
分组。每组内部按文件排序；括号内为 missed/total。

### 3.1 v4 API 与产品后端大场景（约 950 行）

现有 Gate 已有对应 harness（`mutations/tests.rs`、`r2_tests`、`d1_backup` 等），
扩展场景矩阵即可，不需要新基础设施。

- [ ] `crates/service/src/cloudflare_v4/r2.rs`（133/639）：R2 v4 handler 的错误分支与分页边界；
- [ ] `crates/service/src/r2_backend_multipart.rs`（130/652）与 `r2_backend/service.rs`（98/696）：
      multipart 上传/ reconcile 的故障与恢复分支；
- [ ] `crates/service/src/artifact_api.rs`（129/538）与 `artifact_git_http.rs`（58/360）：
      artifact lease/Git HTTP 的错误路径；
- [ ] `crates/service/src/scheduler/runner.rs`（128/652）：scheduler runner 的重试/让渡边界；
- [ ] `crates/service/src/ai_search_backend/*`（ingest 117、主文件 114、search 102、namespace 86）：
      AI search 后端的 ingest/search/namespace 错误矩阵；
- [ ] `crates/service/src/d1_backend.rs`（103/542）、`d1_backup.rs`（103/433）、
      `d1_backend_transfer.rs`（77/527）：D1 执行/备份/传输失败分支；
- [ ] `crates/service/src/workers_http/v4/domain/upload.rs`（112/609）与
      `workers_http/v4/multipart.rs`（71/581）：上传 metadata 边界（assets-only 克隆、
      keep-bindings secret/cron/queue 等分支同时在 `cloning.rs`，33 行）；
- [ ] `crates/service/src/binding_backend/handlers.rs`（110/609）及 kv/artifacts 子模块；
- [ ] `crates/storage/src/scheduler/workflow/durable_steps.rs`（110/407）与
      `durable_object_migrations.rs`（115/663）：workflow step 与 DO 迁移冲突/配额分支。

### 3.2 运行时与进程管理（约 420 行）

- [ ] `crates/runtime/src/process.rs`（126/459）：spawn fd/dup/pgid 错误分支
      （部分需 `test-support` fault 注入扩展）；
- [ ] `crates/runtime/src/supervisor/actor.rs`（103/623）与 `spawn.rs`（91/454）：
      supervisor 罕见转移（Drain 期间 attempt 完成等）与 spawn fault 分支；
- [ ] `crates/runtime/src/lease.rs`（63/358）、`compile.rs`（82/450）、`fsutil.rs`（70/618）；
- [ ] `crates/storage/src/platform_restore.rs`（134/577）与 `platform_snapshot.rs`（93/685）：
      snapshot/restore 的损坏与部分完成分支（`p1-snapshot` Gate 已有骨架）。

### 3.3 服务安装与实例管理（约 460 行）

- [ ] `crates/service/src/instance_control.rs`（125/416）：control socket
      bind/permission/accept 失败分支（需要 socket fault 注入）；
- [ ] `crates/service/src/instance_registry.rs`（116/526）与 `instance_ops.rs`（55/470）：
      registry 生命周期与 purge 分支；
- [ ] `crates/service/src/cli.rs`（120/569）：CLI 命令错误路径（`cli_tests.rs` 已有骨架）；
- [ ] `crates/service/src/setup/filesystem.rs`（107/380）与 `setup.rs`（66/297）：
      systemd/launchd 安装路径（需要 service-manager fake 扩展）；
- [ ] `crates/service/src/run.rs`（115/642）与 `run/startup.rs`（57/401）：
      剩余启动/维护循环分支（health helper 已在 W2 验收中覆盖）。

### 3.4 存储引擎与辅助（约 600 行）

- [ ] `crates/storage/src/d1/execution.rs`（103/685）、`d1/engine.rs`（64/227）、
      `d1/backup.rs`（43/263）、`d1/paths.rs`（58/482）；
- [ ] `crates/storage/src/cloudflare_artifacts.rs`（99/710）、`observability.rs`（95/575）、
      `queues/repository.rs`（94/752）、`cache/engine.rs`（92/630）、`crypto.rs`（85/445）；
- [ ] `crates/workers/src/pipeline/controller.rs`（109/671）、`runtime_source/resolution.rs`
      （62/507）、`workflows.rs`（79/402）。

## 4. 实施约束

- 每轮迭代遵守 [Testing 参考](references/testing.md)：一轮内写批、一次 coverage 验证
  （当前约 25 分钟一轮），不在中间反复跑 Gate；
- 测试必须命中真实行为与不变量，不为覆盖率制造空跑或 only-coverage 分支；
- 3.1 组优先：单文件收益最大且 harness 齐全；每完成一组用
  `target/llvm-cov/summary.json` 复核该组文件的实际增量；
- 第 2 节的结构性不可覆盖项在验收 91% 时单列说明，不寻求“修复”SIGKILL fixture
  或为覆盖率放宽 fail-closed 行为；
- 完成后把本文移入 `docs/implemented/`，保留最终数字与证据。

## 5. 验收

- [ ] `./test/coverage.sh` 一次完整运行报告 workspace 行覆盖 ≥ 91.00%；
- [ ] 第 3 节各组清单完成或明确记录未完成原因；
- [ ] 第 2 节结构性项与实际证据一致；
- [ ] 无任何为提升指标而弱化的断言或被排除的生产源。
