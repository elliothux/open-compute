# Gate 验证节奏

实现、审查或修复尚未完成时，每次迭代只跑一轮相关 Gate。不要每改一处就启动三轮 Gate、递归历史
aggregate 或完整 coverage。先用定向单测和单轮真实运行时测试确认改动，再继续实现或修复。

实现与审查收尾、源码冻结后，才进入最终验收：先完成 AGENTS.md 要求的完整检查和 coverage，
最后执行相关三轮 Gate。完整检查、构建和 coverage 不随 Gate 轮数重复。
最终验收失败且需要改代码时，先回到单轮反馈；修复完成后再补受影响的最终验证。
仅改文档或策略不需要重跑已经完成的 Rust/Gate 检查，只做文档核对与 `git diff --check`。

单轮减少重复执行，不减少用例、断言、真实进程或持久化路径；缺少 runtime、checksum 不匹配、
未允许的失败都仍然失败。coverage 门槛保持 90.00%。已完成的 POC 上游能力探测不再是日常或
最终验收的必跑项；历史 `D-abort` 结论仍约束设计，不能把客户端断开视为保证取消执行。

## 已确定的流程改造

[Runtime 包与测试流程整理](../runtime-and-test-layout.md) 定义目录和入口的目标状态。
本文件下面的命令描述当前实现；文档和 AGENTS.md 的规则已更新，但固定三轮、默认三轮及递归
入口还没有全部改造，不能把目标接口当成已生效的脚本行为。

| 阶段 | 执行要求 |
| --- | --- |
| 开发、审查、修复 | 相关 Gate 目标各一轮；没有相关输入变化，不换个入口重复运行；不跑完整 coverage |
| 实现完成后的静态与覆盖率检查 | 各项完整检查、构建和 coverage 按所需配置执行一次 |
| 最后一步验收 | 冻结源码上，相关产品 Gate 各三轮独立进程；不再重复静态检查和 coverage |

改造后的日常入口默认单轮，显式 `OPEN_COMPUTE_GATE_ROUNDS=3` 才执行最终三轮，只接受 `1` 或
`3`。轮数由一个顶层入口控制，测试本体只执行一轮，子 Gate 不调用其他 Gate。同一目标每轮
一次，首轮失败后停止后续轮次并保留现场，禁止自动重试到通过。

去重按实际 target 和断言覆盖进行，不按脚本名称判断：`test-p0-2.sh` 与 `test-p2-3.sh` 当前
都执行完整的 `p0_2_runtime_gate`，应统一调度或按领域拆分。已有 P0/P1/P2 递归链、P0.1 内部
三轮循环和 coverage 中的隐式多轮都属于待清理项。上游一次性探测、重复回归和废弃模型断言
删除；仍有价值且没有等价覆盖的 POC 产品测试与其 harness、fixtures 迁入 `test/`。

优先去掉重复构建/typecheck、无效 sleep、重复 runtime 准备和非必要的串行等待。复用正确
输入对应的只读资产与编译缓存；可变业务状态、进程生命周期、故障隔离及恢复场景不能共享或
跳过。只在证明隔离后提高并发，长 soak/load/fuzz 不作为每次开发迭代的前置条件。

## 测试目录

仓库内的工具缓存、临时运行目录和保留的失败证据统一放在根目录 `.temp/<用途>/`：
例如 `.temp/ruff-cache/`、`.temp/runtime-cache/`、`.temp/g0-run/`、
`.temp/p0-1-run/`、`.temp/p2-4-run/`、`.temp/p2-exit-run/` 和 `.temp/single-binary-run/`。
只使用 `.gitignore` 的 `/.temp/` 规则，不再逐项添加 ignore；配置工具的输出路径，
不要依赖旧目录或自动迁移回退。历史报告保留当时路径，不因目录迁移改写既往结果。
Rust 构建与覆盖率报告仍在 `target/`；持久化 `.data/`、依赖 `node_modules/` 不属于临时运行目录。

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

crate 内的单元测试和 `crates/**/tests` 保持原位；当前 `runtime/tests/` 随包迁至
`packages/runtime/tests/`，存续的 POC 仓库级测试与 fixtures 迁至 `test/`，不复制已有 crate
测试。目录迁移尚未实施，目标布局见上述整理方案；`poc/` 不再作为长期维护的测试层。
`scripts/` 仅保留本地开发和发布打包入口。历史 `docs/*results.md` 保留当时执行的命令和结果；其中
原 `scripts/` 下的测试入口现已迁入 `test/`，这些记录不代表迁移后重新执行了验收。

## 开发与修复：单轮

所有 Rust 构建（包括默认/无默认 feature 和测试）都必须提供当前目标平台的正式 archive。
它只作为编译期输入，不能以独立 binary 代替，也不会在运行时查找。
底层 workerd Gate 另外使用已存在的 verified binary；禁止测试隐式下载：

```sh
export OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE=/abs/pinned-workerd.gz
export OPEN_COMPUTE_TEST_WORKERD="$PWD/.temp/runtime-cache/v1.20260826.1/workerd"
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
再用 `bun run check:generated` 核对当前生成 JS 与源码清单。当前生成目录还是受跟踪的
`runtime/system-workers/`；迁包后改为不进入 Git 的 `packages/runtime/dist/`，CI 必须先构建
再检查完整性与可复现性，所有消费 runtime 资产的 Rust job 都需要该前置构建。不能仅改 ignore
而继续依赖 Git 中已有产物；迁移时一起消除根流程与包脚本的重复类型检查。

P0.1 的三轮循环写在测试内部，当前没有单轮参数；它保留为最终进程验收入口。开发时先运行所改
process/supervisor 模块的定向测试，明确完整单轮 Gate 尚不可用，不把单测结果冒充 Gate 通过。
正式改造时移出测试内部的轮数循环，保留一轮的完整断言，让顶层入口承担最终三轮重复。
现存 `poc/g0` 是待删除的历史调查入口，不再要求重跑；旧 P2.1 等脚本仍可能间接调用它，
不能因为旧递归关系而将已退役的调查重新列为验收要求。

## 单文件发行定向测试

```sh
cargo test -p open-compute-runtime --all-features --lib embedded:: -- --test-threads=1
cargo test -p open-compute-service --all-features --test single_binary -- --test-threads=1
```

第二项将程序复制到隔离目录，清空 PATH/环境，验证内嵌资源独立工作和真实进程生命周期。
设置 `OPEN_COMPUTE_TEST_PLATFORMD=/abs/platformd` 可检查一个已授权构建的正式发行文件。
准备输入和发布脚本是显式运维操作，不能在 Gate 缺少输入时自动调用下载或打包。

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

以上是按改动选择的最终入口示例，不是每次开发迭代都执行的清单。执行三轮之前，先完成
AGENTS.md 的 format、clippy、workspace tests、no-default-features、MSRV、metadata、
dependency boundaries 与 coverage 检查。完整检查各执行一次，coverage 的插桩运行不替代
未插桩的最终进程验收。统一入口改造后，coverage 只运行一轮所需测试，不再隐式执行多轮。

其余产品 Gate 按实际覆盖选择，避免将已被其他入口执行的相同目标再次运行；现有入口仍有内部
三轮或递归时，应明确记录实际次数，不能把重复执行包装成额外覆盖。统一入口改造完成后，
每个所需目标恰好三轮，不再因入口嵌套增加次数。

`docs/implemented/g0-results.md` 保留已经完成的调查结果；POC 退役不要求重新生成它，也不手改历史报告。
不要把开发阶段一轮通过写成三轮 aggregate 通过，也不要改写历史结果里的实际轮数。报告应标明
源码基线、实际轮数、通过/失败/未运行和接受的限制；保留失败现场。
