# Day1 架构清理验收记录

日期：2026-08-29。起始提交：`f6a4ba47d6cafbb814a8fbf6b9e1d31a86d0d98e`。
实施范围见 [Day1 架构清理](day1-architecture-cleanup.md)。本记录只证明本机当前源码；
没有发布、打包、下载 runtime、执行特权 egress 或重置已有数据。

## 结果

- 清理清单 1–17 与 artifact GC 引用时序全部完成；没有遗留待实现的 Day1 兼容分支。
- 最终源码指纹：`eb426598f28eeb353333d91b2dd81fbef6f8eeae657733407396f029cff68e71`。
- 使用正式 pin `v1.20260826.1` 的本机 Darwin ARM64 archive 与 verified binary。
- coverage 单轮 32 目标通过，Rust line coverage **90.02%**（54,260 / 60,273）；报告位于
  `target/llvm-cov/html/index.html`、`target/llvm-cov/lcov.info` 和
  `target/llvm-cov/summary.json`。
- 最终 Gate 报告：`.temp/gate-run/20260829T121329-f77e4f98/report.json`。
  Round 1 执行 32 目标 / 662 用例；round 2、3 各执行 17 目标 / 42 个登记时序用例；
  三轮全部通过，0 failed / ignored。

## 执行的检查

以下命令均在仓库根目录完成并成功退出：

```text
bun run build
bun run test:js
bun run typecheck
bun run check:generated
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
RUSTFLAGS='-D warnings' cargo check --workspace --no-default-features
cargo +1.98.0 check --workspace --all-targets
cargo metadata --no-deps --format-version 1
./test/check-boundaries.sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s test -p 'test_gate.py'
./test/check-production.py
./test/coverage.sh
OPEN_COMPUTE_GATE_ROUNDS=3 ./test/gate.py --workspace
```

需要 runtime archive/binary 的命令均显式设置
`OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE` / `OPEN_COMPUTE_TEST_WORKERD` 到
`.temp/runtime-cache/v1.20260826.1/` 下已存在且已验证的输入。

## 失败迭代与修正

- 初次 Workflow 开发 Gate 报告
  `.temp/gate-run/failed/20260829T105751-e0db9124/report.json` 保留。它暴露出旧恢复夹具仍按
  已删除的短 dispatch 假设运行，以及 current Workflow wrapper generator 仍选择 capability 2。
  夹具改用当前 durable timeout/recovery 契约，wrapper 改为唯一 capability 1；随后
  `.temp/gate-run/20260829T112725-5314c0e8/report.json` 三个 Workflow 目标单轮通过。
- 初次 coverage 单轮报告
  `.temp/gate-run/failed/20260829T115320-f5a1e3f5/report.json` 保留。`p2-exit` 仍发送已删除的
  `capabilityVersion` selector，Scheduler 单测用固定 yield 次数等待后台任务。删除旧 selector，
  将等待改为有界条件等待后，最终 coverage 与三轮 Gate 均通过。
- 静态检查还收敛了最后一个测试替代物调用方：`p0-2` 不再依赖
  `UnavailableKvBindingExecutor`，而是装配当前 `SqliteKvBindingExecutor`；普通无特性构建不再
  导出该测试替代物。对应 P0.2 单轮 Gate 通过。

失败证据均保留在 `.temp/**/failed/`，没有通过重试掩盖失败或删除诊断。

## 边界

历史 P2.4/P2.5 文档仍记录当时的 capability V1/V2、增量 schema 与验证事实；它们已明确标记为
被本次 Day1 当前模型取代，不能作为恢复旧实现的要求。Cloudflare compatibility date/flags、
用户 D1 migrations、版本/secret/R2 对外契约继续按当前声明支持，不属于被删除的平台历史兼容层。
