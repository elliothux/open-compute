# Gate 验证节奏

实现、审查或修复尚未完成时，每次迭代只跑一轮相关 Gate。不要每改一处就启动三轮 Gate、递归历史
aggregate 或完整 coverage。先用定向单测和单轮真实运行时测试确认改动，再继续实现或修复。

实现与审查收尾、源码冻结后，才进入最终验收：运行 AGENTS.md 要求的完整检查、相关三轮 Gate 和
coverage。最终验收失败且需要改代码时，先回到单轮反馈；修复完成后再补受影响的最终验证。
仅改文档或策略不需要重跑已经完成的 Rust/Gate 检查，只做文档核对与 `git diff --check`。

单轮减少重复执行，不减少用例、断言、真实进程或持久化路径；缺少 runtime、checksum 不匹配、
未允许的失败都仍然失败。coverage 门槛保持 90.00%，G0 的精确 `D-abort` allowlist 不变。

## 测试目录

仓库级测试工具统一放在根目录 `test/`：

| 路径 | 用途 |
| --- | --- |
| `test/test-*.sh` | P0/P1/P2 Gate 入口 |
| `test/check-boundaries.sh`、`test/coverage.sh` | 依赖边界检查与覆盖率 |
| `test/load-p1.sh`、`test/soak-p1.sh` | 压测与持续运行测试 |
| `test/p0-2-egress-fixture.py` | Linux egress 网络夹具 |
| `test/fuzz-p1.sh`、`test/fuzz/` | fuzz 启动脚本、独立 Cargo package 与种子 corpus |

fuzz harness 是测试程序，`test/fuzz/corpus/` 中的输入样本才是 fixture。fuzz package 保持独立
workspace，不随根目录 `cargo test --workspace` 自动运行；入口为 `./test/fuzz-p1.sh --seconds 60`。

crate 内的单元测试、`crates/**/tests` 和 `runtime/tests/` 保持原位；`poc/` 保留 G0 黑盒证据。
`scripts/` 仅保留本地开发和发布打包入口。历史 `docs/*results.md` 保留当时执行的命令和结果；其中
原 `scripts/` 下的测试入口现已迁入 `test/`，这些记录不代表迁移后重新执行了验收。

## 开发与修复：单轮

先选择已经存在且校验通过的 workerd，不能为测试隐式下载：

```sh
export OPEN_COMPUTE_TEST_WORKERD="$PWD/poc/.runtime-cache/v1.20260826.1/workerd"
```

P2.2、P2.3、P2.4、P2.5 和 P2 Exit 脚本支持 `OPEN_COMPUTE_GATE_ROUNDS`。只运行当前改动相关的入口，例如 Workflow：

```sh
OPEN_COMPUTE_GATE_ROUNDS=1 ./test/test-p2-4.sh
```

Durable Workflow 与完整 HTTP → Queue → Consumer → Workflow → KV/D1/R2/DO 链路分别使用：

```sh
OPEN_COMPUTE_GATE_ROUNDS=1 ./test/test-p2-5.sh
OPEN_COMPUTE_GATE_ROUNDS=1 ./test/test-p2-exit.sh
```

P2.5 包含真实 Hard/Product、扩展 snapshot/restore、Workflow authority 和 JS parity 测试。
P2 Exit 包含实际 platformd SIGKILL 链路及独立的 Queue/Workflow authority crash 矩阵；不递归运行
历史 aggregate。失败证据分别保留在相关 ignored run 目录的 `failed/` 下，不能删除后重写通过结果。
P2 Exit 的进程配置复用资源准备阶段的 KV/R2/D1 policy，避免把冻结配额变化当成重启恢复；版本
切换仅推进 Workflow current version，不绕过 DO 的 active Worker deployment 校验。

Queue producer 或 Queue consumer/Cron 分别使用：

```sh
OPEN_COMPUTE_GATE_ROUNDS=1 ./test/test-p2-2.sh
OPEN_COMPUTE_GATE_ROUNDS=1 ./test/test-p2-3.sh
```

这些是可选的相关入口，不是每次迭代都要依次执行的清单。P2.2 单轮模式不会递归启动 P2.1/P1/P0/G0；
P2.3、P2.4 仍会执行各自脚本中的附加检查。

旧 P0/P1 shell aggregate 没有统一单轮开关，不要给它们加上未读取的环境变量后便声称“只跑一轮”。
P0.2 至 P0.8 和 P0 Exit 可直接调用相关 integration test，避免外层三轮及递归，例如：

```sh
cargo test -p open-compute-service --all-features \
  --test p0_2_runtime_gate -- --test-threads=1
```

P0.2 同时验证 Worker TS CLI 的编译、部署、请求和类型错误拒绝路径，需要本机 Bun 与已经安装
的根 workspace 锁定依赖。可用 `OPEN_COMPUTE_TEST_BUN` 指定 Bun 路径；测试不会下载工具链。
JS/TS 开发检查使用 `bun run typecheck` 和 `bun run test:js`；修改系统源码后执行 `bun run build`，
再用 `bun run check:generated` 核对提交的 JS 与源码清单。CI 使用锁定依赖执行相同检查。

P0.1 的三轮循环写在测试内部，当前没有单轮参数；它保留为最终进程验收入口。开发时先运行所改
process/supervisor 模块的定向测试，不修改循环或断言来伪造单轮结果。G0 可按改动选择
`./poc/g0 test bootstrap`、`loader`、`binding`、`durable-object` 或 `recovery` 做单次诊断；
standalone loader 的既定 `D-abort` 仍退出非零，不能把它记为普通 PASS。

## 源码冻结：最终验收

P2.4 最终三轮入口为：

```sh
OPEN_COMPUTE_GATE_ROUNDS=3 ./test/test-p2-4.sh
```

P2.5 和 P2 Exit 使用相同轮数规则：

```sh
OPEN_COMPUTE_GATE_ROUNDS=3 ./test/test-p2-5.sh
OPEN_COMPUTE_GATE_ROUNDS=3 ./test/test-p2-exit.sh
```

再按改动范围执行相关历史 Gate 和 AGENTS.md 的 format、clippy、workspace tests、
no-default-features、MSRV、metadata、dependency boundaries 与 coverage 检查。只运行所需的
aggregate 入口，不把已经由它递归覆盖的同版本 Gate 再单独重复一遍。

`./poc/g0 test all` 始终是三轮最终回归入口，并原子生成 `docs/g0-results.md`；不要手改生成报告。
不要把开发阶段一轮通过写成三轮 aggregate 通过，也不要改写历史结果里的实际轮数。报告应标明
源码基线、实际轮数、通过/失败/未运行和接受的限制；保留失败现场。
