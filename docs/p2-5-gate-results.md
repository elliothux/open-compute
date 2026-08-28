# P2.5 Durable Workflow 与 P2 Exit 验证记录

状态：**P2.5 Conditional Go；P2 Exit Gate PASS**。2026-08-28 完成最终验收。
条件仍是 DO 内 Workflow mutation 不支持，以及既有 stock-workerd `D-abort` 限制；不扩大 G0 allowlist。
基础 WebSocket 保持可用，P1.8 hibernation 仍为原有未支持能力。

## 实现与固定输入

实现 [P2.5 设计](./p2-5-workflow-durable-waiting.md) 的显式 capability V2：静态 retry/backoff、
attempt timeout、durable sleep/event、pause/resume/terminate/restart、retention、可恢复清理及有界
parallel `step.do`。仍由一个 `platformd`、一个受监督的 stock `workerd` 和两个 SQLite authority 承担。

- 基线 revision：`9fe5b4a47a2136ff27a02989b4c8481c09bf412b`，加本次未提交 worktree。
- 验证主机：Darwin 25.6.0 arm64。
- workerd：`v1.20260826.1`，版本输出 `workerd 2026-08-26`；使用现有缓存，没有下载或升级。
- lock SHA-256：`d3614a6394cf85e24704954d9e7a9585fb38a2107e8eb73a02519e39add14d2e`。
- archive SHA-256：`22657ec7045a3677b7f52e97f106fe0493add57810687e755e8c6f4fba4b1dba`。
- binary SHA-256：`2d17da54d2671d6e9e7c776d56b934f60be8c140b9bac35ddf22f60d6cff9403`。
- control schema **14**：本阶段追加 013/014；scheduler schema **8**：追加 006/007/008。已执行的 migration 冻结。
- compatibility date `2026-08-22`；process flag `--experimental`；compatibility flags 与正式 lock 一致。
- 最终 471 个代码/config/runner 文件清单：本机 `/tmp/open-compute-p2-source-freeze-v4.json`；
  清单 SHA-256 为 `81bb0240af3158deb940900a6488431e1c41716b62bbc96769f0a10d720ca340`。

V1 两版 wrapper generator、facade、runner、JSON codec 与基线逐字节一致。V2 使用独立 codec、
runner/controller 和 generator revision 3；旧 version/binding/instance 不隐式升级。capabilities
标识为 `p2.5-workflow-durable-v2`，它是支持面标识，不能替代本记录的执行证据。

状态与配额归 storage，跨库 lifecycle saga 归 workers，transport/调度组合归 service，租户执行与
私有 token controller 归 system workers。新增源文件和测试文件均少于 800 行。既有较大的启动组合、
配置和集成矩阵保留原组织，只接入对应 wiring/断言；新的 durable 状态机按所有权拆入独立模块。

## 最终验证

所有入口使用绝对路径 `OPEN_COMPUTE_TEST_WORKERD="$PWD/poc/.runtime-cache/v1.20260826.1/workerd"`。
三轮 runner 显式设置 `OPEN_COMPUTE_GATE_ROUNDS=3`。开发和修复期间使用定向测试或单轮入口。

| 检查 | 实际结果 |
| --- | --- |
| `cargo fmt --all --check` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `RUSTFLAGS='-D warnings' cargo check --workspace --no-default-features` | PASS |
| `cargo +1.98.0 check --workspace --all-targets` | PASS |
| `cargo metadata --no-deps --format-version 1` | PASS |
| `./scripts/check-boundaries.sh` | PASS |
| `cargo test --workspace --all-targets --all-features --no-fail-fast -- --test-threads=1` | **684 passed，0 failed，0 ignored** |
| `./scripts/coverage.sh` | **90.16%**，56,391 / 62,547 Rust lines；90.00% 门槛和排除规则未改 |
| `./scripts/test-p2-5.sh` | 三轮各 2 Hard + 2 Product + 1 snapshot；随后 core 16 / storage 44 / workers 17 / service 15 个 Workflow 单测及 JS 10 项通过 |
| `./scripts/test-p2-4.sh` | 三轮各 2 Hard + 6 Product，附带 Workflow 单测通过 |
| `./scripts/test-p2-3.sh` | 三轮运行时矩阵、持久化/迁移、promotion 和生产构建检查通过 |
| `./scripts/test-p2-2.sh` | 三轮及递归 P2.1/P1/P0/G0 aggregate 全部成功退出 |
| `./poc/g0 test all` | 在上一行 aggregate 内执行；三轮 Conditional Go，唯一允许非 PASS 为精确 `loader:D-abort`，各轮 `abortEvents 0 -> 0` |
| `./scripts/test-p2-exit.sh` | 修正版三轮链路 + 实际执行的 14 点 Workflow SIGKILL 矩阵 + Queue commit crash 回归通过 |
| `git diff --check` | PASS |

工作区的顶层 Rust 测试计数包含 artifacts 53、core 72、runtime 88、supervisor 25、service 154、
storage 189、workers 62，以及 CLI/MSRV/P0/P1/P2 集成目标。不重复累加测试内部启动的子进程用例。
`--no-fail-fast` 只让失败时继续收集其他目标结果，没有忽略失败。

最后一次修正只涉及 `test-p2-exit.sh` 的测试选择和存在性预检。Rust/runtime/config 与完整工作区和
coverage 通过时完全一致；随后重新运行静态检查、该 runner 单轮开发验证及最终三轮。

### 逐轮运行时证据

| 目标 | 第一轮 | 第二轮 | 第三轮 |
| --- | --- | --- | --- |
| P2.5 Hard：2 tests | PASS / 55.11s | PASS / 55.21s | PASS / 54.74s |
| P2.5 Product：2 tests | PASS / 25.78s | PASS / 25.84s | PASS / 25.47s |
| 扩展 snapshot：1 test | PASS / 16.83s | PASS / 17.02s | PASS / 17.23s |
| P2.4 Product：6 tests | PASS / 182.95s | PASS / 180.81s | PASS / 182.58s |
| P2.3 runtime matrix | PASS / 20.03s | PASS / 20.60s | PASS / 20.58s |
| 修正版 P2 Exit 链路 | PASS / 161.22s | PASS / 160.78s | PASS / 160.13s |

P1 历史 aggregate 同时通过了三轮 conformance、crash/recovery、upgrade、产品矩阵及本机 mixed
容量采样。离线 fuzz 在 10 秒完成 10,062,218 个案例；生产 release binary 的故障入口/canary 检查通过。
mixed 采样为 3 轮、183 个请求；其结果只描述该本机夹具，不是通用 SLA。

## Case 与边界

| 验证面 | 实际验证落点 |
| --- | --- |
| Runtime Hard | `p2_5_workflow_hard_gate.rs` 验证 suspension、可信 timeout、异常 drain、parallel、原生错误与 token 隔离；`durable_binding.rs` 验证混合 capability 和固定 internal UUID 的私有 handle |
| 生产 durable 执行 | `durable_execution.rs` 走生产 scheduler/binding dispatcher，验证 sleep、retry、event、runtime 重启及 replay |
| 生产 batch | `durable_batches.rs` 覆盖 nested/overlap/unjoined 拒绝、17 sibling 整批拒绝、allSettled join、16 个近 1 MiB 结果后消费大事件 |
| 双库 authority | storage Workflow 迁移、history/accounting、event FIFO/timeout、quota、3:1 新旧任务选择与账户轮转测试；workers 验证 restart/purge saga、retained ref 和旧 handle 拒绝 |
| Snapshot | P2.4 Product 的扩展 snapshot fixture 包含 V2 waiting/paused/inbox、restart R1/R2 和 purge P1/P2/P3；真实 fresh-host restore 后继续原版本 replay |
| DO 边界 | P2 Exit 中真实 DO 可调用 V2 get/status；create、pause、resume、terminate、restart、sendEvent 六种 mutation 均返回既定拒绝码 |
| Operator/安全 | admin metadata-only GET、generation quarantine、operator resume 不绕过隔离、doctor/recover-corrupt、固定 metrics 预算 548 与启动/关机回归 |

上述 `*.rs` 运行时文件位于 `crates/service/tests/` 或其 `workflow_support/` 下。Hard Gate 的协议
探针使用测试自有 SQLite fixture，只证明 runtime 可行性；生产 schema、scheduler 和跨库验收由独立
Product/Exit Gate 完成。所有运行时 Gate 使用验证过的 stock workerd；S3 使用既有 SigV4 网络夹具。

### 完整 P2 Exit

`crates/service/tests/p2_exit_gate.rs` 每轮执行真实 HTTP → Queue → Consumer → Workflow →
KV/R2/D1/DO 链路，启动五次 platformd 并在四处 SIGKILL：

1. Queue 已接受延迟消息后终止，重启后消息仍存在。
2. Consumer 已创建 Workflow、尚未 ACK 时终止；重投递得到同一 instance，ACK 后 Queue 清空。
3. 已完成产品 step 之后、另一 callback 尚未 commit 时终止；已完成 step 不重跑，Unknown 使用同一
   业务 attempt 和新 run token 恢复，旧 token commit 被拒绝。D1/DO 幂等效果不会重复增加。
4. 已注册 sleep、暂停并接受事件后终止，让 Workflow deadline 和 DO alarm 在离线期间到期；恢复后
   暂停状态/inbox/原 deadline 保留，resume 使用冻结原版本，消费事件并恢复 alarm。

版本切换只更新 Workflow current version，不绕过 DO 对 active Worker deployment 的校验。资源准备与
进程启动使用同一 KV/R2/D1 policy。等待期间不持有 run lease；暂停不冻结 wall-clock deadline，
maintenance 可以结算到期等待，但不会执行 paused instance 的用户代码。

附加的 `workflow_sigkill_durable_wait_retry_pause_restart_and_purge_boundaries` 是 **1 个 Rust 测试内
14 个真实子进程 SIGKILL 边界**：attempt-granted、retry-committed、pause-requested、wait-registered、
event-committed、yield-committed、restart-prepared/applied/finalized、terminal-unretained、
purge-prepared/deleted/released/swept。修正版最终运行选中并通过该测试（2.23s），随后 Queue commit
crash 目标的 2 个测试通过。没有把“0 tests”计作矩阵证据。

## 保留限制与失败记录

默认单个 JSON 值最多 1 MiB、step timeout 60 秒、并行度 4。attempt 上限还受 operator 的
dispatch/drain 预算约束。V2 terminal 在 retention 内保留代码引用；restart 保持原 internal ID、input
和 frozen target，仅增加 generation。外部副作用仍需业务幂等，不承诺 exactly-once 或回滚。
不支持完整 structured clone、并行 wait、任意 Promise DAG、createBatch、动态 retry function、
rollback hooks、restart-from-step 或完整 Cloudflare REST/Wrangler 兼容。详见
[兼容性偏差](./p1-deviations.md) 与 [设计边界](./p2-5-workflow-durable-waiting.md)。

开发链路发现并修复了 route revision 与 Queue/Cron 持久 epoch 冲突、runtime 尚未 Running 时的 V2
admission、DO V2 binding transport 缺失导出，以及已结算 batch 结果未及时释放的问题。全量回归还
修正了空 Workflow 工作时的 poll admission、capability 偏差断言，以及旧 schema 升级夹具的版本期望和
残留 operation-progress 表；没有修改已应用的 migration。

首次 Exit runner 用文件名作为 Cargo filter，虽然三轮链路通过，附加 Workflow 矩阵却选中 0 项。
该输出不作为最终 Gate 证据。现使用精确测试名并预检其存在；修改后先通过单轮，再重新完成三轮。
早期运行超时和其他失败现场保留在相关 `failed/` 目录及本机日志中；未放宽 timeout、断言、覆盖率或
G0 allowlist，也未删除失败现场。

最终检查未发现遗留 platformd/workerd；`.p2-4-run/`、`.p2-5-run/`、`.p2-exit-run/` 没有非 failed
残留目录。测试验证了监听器关闭、子进程 reaping 和私有 token/result 不进入平台日志。
本次未执行 Linux-only sudo egress 专项，也没有正式发布打包、上传或部署。

## 本机证据路径

- 工作区：`/tmp/open-compute-p2-final-workspace-v3.log`。
- 覆盖率：`/tmp/open-compute-p2-final-coverage.log`；报告为 `target/llvm-cov/html/index.html`、
  `target/llvm-cov/lcov.info`、`target/llvm-cov/summary.json`。
- 静态检查：`/tmp/open-compute-p2-final-{fmt,clippy,no-default,msrv,boundaries}-v4.log`；
  metadata 为 `/tmp/open-compute-p2-final-metadata-v4.json`。
- P2.5/P2.4/P2.3/P2.2：`/tmp/open-compute-p2-final-gate-p2-{5,4,3,2}.log`。
- 修正版 Exit 最终证据：`/tmp/open-compute-p2-final-gate-p2-exit-v2.log`；单轮为
  `/tmp/open-compute-p2-exit-runner-development.log`。旧 `/tmp/open-compute-p2-final-gate-p2-exit.log`
  保留用于说明筛选错误，不计为最终通过证据。
- 容量采样：`target/p1-results/load/result.json` 与 `capacity.ndjson`。
- G0：[自动生成结果](./g0-results.md)，由本次 `./poc/g0 test all` 原子更新，未手工编辑。
