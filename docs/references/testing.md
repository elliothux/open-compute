# Gate 验证节奏

开发、审查、修复时，每次迭代只跑相关目标一轮。源码冻结后先完成静态检查与 coverage，
最后执行**完整一轮 + 时序用例补两轮**。确定性用例总计一次，时序用例总计三次。
失败停止，不自动重试；coverage 不代替未插桩的进程验收。

该入口默认验收本地平台 contract/product，不以第三方框架测试定义支持面。P3.4 的 typed target
与 Cargo target 使用同一 discovery、精确 case、分轮、环境 allowlist、超时、清理和报告协议。
应用 qualification 和真实 Cloudflare differential 必须显式选择、独立报告，不能暗中加入普通
`--workspace`。

## 显式准备输入

```sh
bun install --frozen-lockfile --ignore-scripts
bun run build
bun run check:generated
bun run test:js
export RUSTFLAGS='-D warnings'
export OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE=/abs/pinned/workerd-platform.gz
export OPEN_COMPUTE_TEST_WORKERD=/abs/verified/workerd
```

唯一正式 pin 是 `packages/runtime/workerd.lock.json`。`packages/runtime/dist/` 不提交 Git；
每个 CI job 及干净检出都必须先构建。Rust 校验 archive、binary、生成模块、完整清单及
源码/工具配置摘要，拒绝过期资产。生产启动离线，不依赖 JS 工具链。
输入准备工具 `bun scripts/prepare-workerd.ts --dest /abs/new-dir --archive /abs/pinned.gz`
只接受显式来源并拒绝覆盖；`--download`、发布打包和特权网络夹具需要单独授权。

## 一个调度入口

调度器仅依赖 Python 3.11+ 标准库；CI 显式选择 Python 3.12。

```sh
./test/gate.py p0-2
./test/gate.py p0-2 p2-3 --jobs 2
./test/gate.py workflow --list
./test/gate.py p3-contract
# 外部写入；仅在明确授权、预置账号与 Wrangler OAuth/token credential 后选择：
./test/gate.py p3-cf-diff
# 在同一个冻结报告中合成本地 contract 与 remote qualification：
./test/gate.py p3 p3-cf-diff
OPEN_COMPUTE_GATE_ROUNDS=3 ./test/gate.py all --jobs 2
OPEN_COMPUTE_GATE_ROUNDS=3 ./test/gate.py --workspace --jobs 2
```

`OPEN_COMPUTE_GATE_ROUNDS` 只接受 `1`（默认）或 `3`；非法值在构建、网络夹具变更或运行前失败。
`3` 表示按用例分轮的最终验收，不表示所有目标重复三次。`--list` 输出每轮目标、注册用例及
计划进程数，不构建、不执行；workspace 中未列入产品 Gate 的宿主用例会在构建后发现，
计划中的 `null` 不代表跳过。`inventory_verified` 只有核对原生 discovery 后才为真。
旧 `test-p*.sh` 递归入口已删除，
仅保留需要授权和清理的 Linux egress wrapper。

| 选择 | 实际目标 |
| --- | --- |
| `p0-1` … `p0-8`、`p0-exit` | 对应 service 集成测试；P0.1 本体只有一轮 |
| `p1-conformance`、`p1-security`、`p1-crash`、`p1-snapshot` | 对应当前 P1 集成测试；恢复 staging 拒绝矩阵归 `p1-snapshot` |
| `p1-8` | P0.7 WebSocket/hibernation 与 P1 capability；保留该别名用于当前产品回归，不恢复旧 No-Go 实现 |
| `p2-1`、`p2-2`、`p2-exit` | 对应 scheduler、queue producer、产品链集成测试 |
| `p2-3` | P0.2 同一 Worker/Queue/Cron 矩阵，不再重复调度 |
| `workflow-runtime` | 当前 Workflow 的真实 runtime、suspension、timeout、parallel 与 native output gate |
| `workflow-recovery` | 当前 Workflow 的 snapshot、进程恢复、transport fault 与产品 binding 路径 |
| `workflow-product` | 当前 durable execution 与大结果/批次边界 |
| `workflow` | 上述三个 Workflow 目标；`p2` 也包含它们 |
| `p3-contract` | baseline/catalog、capability/type/config/deviation/case/source 双射；无 workerd、网络或外部 mutation |
| `p3-assets`、`p3-services`、`p3-cache-images` | 对应静态资产、Service binding（含事件源与 SIGKILL cleanup）、Cache/Images 真实 runtime 产品矩阵 |
| `p3-isolation`、`p3-recovery` | 两账户 fail-closed 与 P3 产品隔离；进程/快照/跨产品 crash recovery 与清理 |
| `p3` | `p3-contract`、P0/P1/P2/Workflow 与全部 P3/L6 本地目标，不含外部 differential |
| `p3-cf-diff` | 显式真实 Cloudflare portable differential；不属于 `all` 或 `--workspace` |
| `runtime`、`single-binary` | supervisor、单文件离线首启/重启/损坏路径 |
| `p0`、`p1`、`p2`、`all` | 对应集合；多个选择取并集，首轮完整，后两轮仅时序用例 |

`p3-cf-diff` 每个 fixture 只使用随机 `oc-p34-*` Worker 名与 workers.dev endpoint；按 fixture 在两个
provider 创建同前缀且唯一的 KV namespace、D1 database、R2 bucket、Queue、Durable Object namespace 或
Workflow。runner 在 mutation 前验证目标账号和每个同名资源不存在，只对只读状态做有界重试，绝不重试
写操作。cleanup 禁止扩大到非本轮资源，按精确 Worker 名、route ID 和 binding resource ID/name 删除并
再次枚举确认 absent；本地 deployment retention 需要显式
`OPEN_COMPUTE_TEST_RUNTIME_RESTART_ACK=restart-generation`，且只允许对本次 qualification 的专用测试
实例轮换 generation。任何清理失败都使 Gate 失败并保留 inventory，不得扩大到账号级批量删除或触碰
其它服务。

## 哪些用例重复

唯一分类表是 [`test/gate_cases.py`](../../test/gate_cases.py)，按完整 Rust 测试名登记。
用例的重要性、P0/P1/P2 标签、使用 workerd/SQLite 或名称含 crash，都不是重复的充分理由。
一轮仍执行完整矩阵、全部故障点和场景内部的多次重启，不删断言、不用 mock 代替真实路径。

| 归属 | 用例和理由 |
| --- | --- |
| 一轮 | `p1-conformance`、`p1-security`、`p1-snapshot`：固定能力/权限/快照/提交故障矩阵；capability 用例内部的两个新 CLI 进程仍保留 |
| 一轮 | `p2-1` 与 P2.2 commit-crash 两个用例：每个固定状态点仍启动独立子进程，观察 marker 后 SIGKILL；不靠随机撞中窗口 |
| 一轮 | 当前 Workflow 的版本/重放/终态、snapshot、HTTP known/unknown 完整矩阵：显式驱动 claim/commit/replay，保留每个当前故障和 restart |
| 一轮 | Workflow hard 中独立的 native DO 回滚/禁止外部 mutation、capability/私有 handle 矩阵；同宿主其他时序用例仍三轮 |
| 一轮 | single-binary 只读命令、P0.1 固定 listener 排序、supervisor 的拒绝编译/启动前关闭/确定性时钟用例 |
| 三轮 | 进程启动中断、TERM/KILL、孤儿回收、管道与 pin 清理、在途请求中断、并发写入、alarm/Workflow deadline 与迟到回调、跨产品 crash/recovery |
| 三轮 | 尚未分离的混合矩阵（P0.2–P0.8、P2.2 producer、Workflow hard 等）：不能因其中 CRUD 确定就把其时序断言降为一轮 |

`workflow-recovery` 的 `product_bindings` 还包含外部效果提交后在途 SIGKILL、真实 scheduler 并发 backlog，
因此整个用例仍跑三轮。`process_crash` 与 fixture drop/reaping 也保留三轮。
分类不赋予旧平台 schema 或私有协议兼容义务，Day1 清理仍按其独立范围进行。

调度器将 `--list` 的实际用例集合与分类表逐项比对：新增、删除、改名、重复登记、空 Gate
或非预期 discovery 都在业务用例启动前失败。每轮用 `--exact` 传入完整名字，仍在每个宿主
内部串行执行。退出成功但通过数不足、用例被忽略或全部被过滤掉也不能通过验收；允许 Cargo
metadata 声明且 discovery 确认为零用例的 workspace binary harness。
确定性目标在第二、三轮不创建空进程；纯确定性的最终选择实际只执行一轮。

三轮只是对真实时序的额外抽样，不证明不存在竞态。新增时序场景应先用明确的状态屏障或
故障点覆盖，不用重复次数弥补缺少断言。拆分混合测试时要实测，避免为每个断言重复启动整套系统。

调度器一次 `cargo test --no-run --all-features`，根据 Cargo JSON 的精确 executable 路径运行测试，
不搜索可能过期的哈希文件；typed target 则校验 tracked source/executable identity，并通过自身 JSON
discovery 枚举精确 case。后续各轮不再调用 Cargo，也不重建 JS。正确 keyed 的 `target/`、
`node_modules/`、正式 immutable 输入可复用，不清缓存、不下载。库单测、类型检查与 coverage
由下面的完整检查负责，不能塞回 Gate 循环。load/soak/fuzz 和正式打包保留独立显式入口。

构建后先对每个原生测试宿主执行一次有界 `--list`（最多 600 秒），全部成功后才进入用例阶段。
这会执行测试框架的发现功能，但不执行用例、不启动 workerd、不创建产品状态；其耗时及
`preparation_processes` 单列。它隔离冷宿主加载/系统可执行文件评估与业务超时，三轮之间不再
重复。真实运行时验证、readiness、事务、崩溃与恢复的超时和 fresh-process 要求均不改变。

## 多核与隔离

`--jobs N` 只并发经过隔离审查的**测试进程**；默认 `min(4, CPU 数)`，进程内保留 `--test-threads=1`。
业务数据库、generation token、临时目录、S3 fixture/prefix 和端口均由各目标独立拥有；
每目标每轮提供独立 `TMPDIR`，端口由系统分配。P0.1 的全局 staging 断言、supervisor、
single-binary 与 `workflow-product` 使用独占屏障。该目标的 16 路约 1 MiB 结果在并行实测中遇到
workerd socket `ENOBUFS`，仅完成 15 路；隔离整个目标以避免叠加内核缓冲区压力，不缩小
数据、减少内部并发、放宽断言超时或将失败改为重试。不能把全局状态测试直接改成多线程。
临时目录使用较短的 `.temp/gate-tmp/<随机名>/`，避免报告目录中的长目标名称超过 Unix socket
路径长度上限；退出后非空目录移入该目标的诊断目录并拒绝覆盖，成功返回但留有文件也算失败。
并行失败后不再提交目标，已开始的目标完成自己的清理；不启动下一轮。

串行/并行对比必须使用相同源码、输入、目标及构建配置，分别记录编译和执行耗时，不能用
串行冷编译与并行热缓存混算提速。基准对比是明确的测量活动，不冒充最终三轮验收。
并发结果仍受 CPU/内存、磁盘和系统负载影响，不承诺固定提速比例。

开发/test profile 仅对 `sha2` 和 `miniz_oxide` 依赖使用 `opt-level=3`，降低每个独立数据目录
首次物化时的完整解压/摘要校验成本。workspace 源码仍未优化，debug 断言、溢出检查和 release
配置不变；不缓存或绕过完整性检查。实测及其测量口径见 [性能记录](../implemented/runtime-and-test-layout-results.md)。

`--workspace` 通过 Cargo metadata 枚举全部启用的 test harness，使用
`cargo test --workspace --all-targets --all-features --no-run` 一次构建，再逐一执行；
拒绝缺少或未计划的 executable。它与普通 Cargo workspace 测试的目标集合相同，保留 package
工作目录（supervisor 的相对诊断输出使用本轮目标目录，避免清空环境的夹具写入源码树）；
不接受同时指定 Gate 名称。默认一轮；最终 `OPEN_COMPUTE_GATE_ROUNDS=3` 时第一轮覆盖全部
workspace 宿主，第二、三轮只运行登记的产品时序用例。库、普通集成和零用例 binary harness
不被机械重复；它替代同一冻结输入上“workspace 一次再额外完整 Gate 三次”的重复调度。
core/storage/artifacts/workers/service 五个库的故障钩子均
为进程私有，staging/数据库归属独立 TMPDIR，已经并发验证。runtime 库的 1–2 秒进程故障
窗口在并行实测中失败，因此保留独占，不放宽时限。CLI 先独占执行，使实际 platformd 的首次
加载不与定时 runtime probe 重叠；新增未审查目标保守独占。无需安装额外 Rust 测试运行器。

## 完整检查与最终验收

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --keep-going -- -D warnings
RUSTFLAGS='-D warnings' cargo check --workspace --no-default-features
cargo +1.98.0 check --workspace --all-targets
cargo metadata --no-deps --format-version 1
./test/check-boundaries.sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s test -p 'test_gate.py'
./test/check-production.py
./test/coverage.sh
# 最后一次调度同时完成普通 workspace 测试与产品时序验收。
OPEN_COMPUTE_GATE_ROUNDS=3 ./test/gate.py --workspace
```

production 检查只构建无 test-support 的普通开发二进制并扫描测试标记，不调用发行包装。
coverage 的清理限于自身 workspace 插桩产物，Rust 行覆盖率门槛仍为 **90.00%**。
coverage 在任何清理前拒绝多轮请求；调度器拒绝把已知 coverage 插桩环境用于最终三轮。
coverage 使用 cargo-llvm-cov `show-env --sh` 的外部运行器接口和相同 workspace 调度器，
产物放在 `target/llvm-cov-target/`，每进程 profile 名包含 PID/模块身份。supervisor 夹具的环境
由生产 spawn 清空，其默认 profile 留在本轮 `.temp/gate-run/` 诊断目录；不向生产环境透传变量，
夹具仍按原有测试源码规则排除，生产模块的插桩与计数不变。
`./test/coverage.sh --jobs 1` 可串行诊断，不清理普通 `target/debug/` 缓存。
真实运行时测试缺少 workerd 即失败，不允许静默 `return` 成为通过证据。

## 日志与保留证据

调度报告位于 `.temp/gate-run/<run-id>/report.json`，包括源码/输入/测试可执行文件摘要、
revision、工具链、目标集合、轮数、并发度、一次构建耗时、每目标耗时、总墙钟和子进程 CPU 时间。
另列分轮策略、每轮精确用例、已验证 inventory、计划用例数与实际通过数；不能只用目标个数
推断覆盖完整。P3 运行另写 `contract-report.json`，列出 baseline/catalog digest、逐 contract
状态/case/deviation、L0–L3/L6 结果、Cloudflare qualification 与 Platform verdict。显式
`p3-cf-diff` 写去 credential 的 `diff-report.json` 和 cleanup inventory；未选择应用时不伪造
`application-report.json`。旧“全部三轮”的历史记录保留原样，不改写成新策略的实测。
源码冻结包含所有仓库代码、配置、测试及 `docs/references/`（内嵌 runbooks 与 conformance
输入），不包含没有代码消费者的 `docs/` 规划和 `docs/implemented/` 历史记录；并行编写规划
不会中止测试。报告声明此范围；代码、维护参考文档、正式 pin 或资产变化仍会失败。
报告另列测试宿主准备阶段和实际准备进程数，不把发现进程算成已执行的业务用例。
`test_processes_executed` 统计顶层测试进程，场景内 restart/crash 的子进程仍由各测试拥有。
失败整次保留在 `failed/<run-id>/`；已运行、失败和未运行目标不混淆。终端不回显失败测试的
原始输出；日志供本地诊断，生成报告不包含 secret 或环境变量转储。

所有仓库内临时文件及失败现场留在 `.temp/<purpose>/`；Rust 产物仍在 `target/`，依赖在
`node_modules/`，业务持久状态在 `.data/`。不删除历史 `.temp/` 证据。

POC 一次性上游探测已退役；删除分类与保留回归见 [迁移记录](../implemented/runtime-and-test-layout.md)。
`docs/implemented/g0-results.md` 保留原始字节，`D-abort` 仍是已接受限制：不能把客户端断开
当作保证取消执行。产品 Gate 保留上传取消、流中断、事务/响应失败及 crash/recovery 断言。

Linux 受控 egress 仍需显式 `OPEN_COMPUTE_EGRESS_FIXTURE_ALLOW_SUDO=1`，执行
`OPEN_COMPUTE_GATE_ROUNDS=3 ./test/test-p0-2-egress-linux.sh`；它变更 loopback 与 `/etc/hosts`，
不能在未授权宿主上运行。正式发行文件需另行授权构建，再以 `OPEN_COMPUTE_TEST_PLATFORMD`
传给 `single-binary`，本地未包装的二进制测试不能声称正式发布已通过。
