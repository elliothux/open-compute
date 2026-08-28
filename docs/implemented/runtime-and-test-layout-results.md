# Runtime 布局与测试并行实测

日期：2026-08-28–29。实现见 [迁移记录](runtime-and-test-layout.md)。本机完整验收已完成；
以下分别记录单轮基准、开发失败与修正、完整检查和最终三轮，不混用证据。

## 并行配置的串行与并行实测

宿主：macOS 26.6.2、arm64、12 核、32 GiB；Rust 1.98.0，`RUSTFLAGS=-D warnings`。
使用本地已验证的 stock workerd `v1.20260826.1`，无下载或 mock 替代。
依次运行同一组 `p0-3 p0-4 p0-5 p0-6 p0-7 p0-8`，分别指定 `--jobs 1/4/6`。
每组只有一个样本，均为六个新测试进程，目标内部仍单线程；没有并行运行其他构建/测试。

| 并发进程 | 构建秒数 | 宿主准备秒数 | 测试执行秒数 | 子进程 CPU 秒数 | 结果 |
| --- | ---: | ---: | ---: | ---: | --- |
| 1 | 0.611 | 0.076 | 148.10 | 15.31 | 6/6 通过 |
| 4 | 0.545 | 0.031 | 70.98 | 15.65 | 6/6 通过 |
| 6 | 0.495 | 0.024 | 67.14 | 18.26 | 6/6 通过 |

执行部分 4 并行比串行减少 **52.1%**（**2.09 倍**）。6 并行仅再缩短 5.4%，CPU 消耗反而增加，
因此默认 `min(4, CPU 数)`，允许显式调整。CI 使用 2，适应较小的 runner。CPU 列包含各组
一次缓存构建；构建、发现宿主和用例执行分列，不把首次原生加载或编译缓存差异算作并发收益。
这是单机单次实测，不是中位数、统计置信区间或跨平台承诺；系统缓存和后台负载仍会影响结果。

三组源码摘要均为 `b2bf2065ad546ec1338f15750be1a7890c9c1eb0996d62f5458d3ff5769cefee`，
manifest、workerd/archive 和每个测试 executable 摘要也一致。每个用例仍用新进程、新业务目录；
这里只复用构建缓存与已验证的不可变输入。汇总：
`.temp/runtime-layout-final-benchmark/fcec14e7/report.json`。

- 串行：`.temp/gate-run/20260829T005928-b20733cb/report.json`
- 4 并行：`.temp/gate-run/20260829T010157-3eb30304/report.json`
- 6 并行：`.temp/gate-run/20260829T010310-04574659/report.json`

### 初测记录与口径修正

初测为串行 313.20 秒、4 并行 80.44 秒、6 并行 77.41 秒，均 6/6 通过，源码摘要均为
`766ddc0f3e1ec9862ea26ee7e861037ad8be8e1bfb7cf80d9ef9e5d8cc4558cf`。当时尚未分离
原生宿主准备，后来发现 macOS 首次系统加载存在显著等待。故 **313→80 的 74.3% 不能当作
纯并行收益**，以上述分离宿主准备的公平对比为准；原始报告不改写。

原始本地报告（未修改）：

- 串行：`.temp/gate-run/20260828T222346-8398998c/report.json`
- 4 并行：`.temp/gate-run/20260828T222935-1d7c7270/report.json`
- 6 并行：`.temp/gate-run/20260828T223112-2096faba/report.json`

## 解压和摘要校验

同机使用相同 `crates/runtime/build.rs`、输入快照和 pinned archive。以相同 rustc 参数编译
实际 validator，仅替换依赖的优化配置；不改 workspace 代码优化级别。每次使用独立 OUT_DIR，
先分别完成一次原生加载，再交替测量各三次，构建及首次加载不计入样本。12 次输出的完整文件
集合与 SHA-256 全部一致。没有同时运行其他构建或测试。

| 配置 | 三次中位数（秒） |
| --- | ---: |
| 原始 dev 依赖 | 9.748 |
| 仅 sha2 opt-level=3 | 2.896 |
| sha2 + miniz_oxide opt-level=3 | 1.038 |

合计降低 **89.4%**。更早单独优化 miniz_oxide 的三次中位数为 7.768 秒，对照为 9.621 秒；
因此只保留这两个已测量的依赖 override，不优化所有依赖。test 继承 dev 配置，debug 断言和
溢出检查不变，release 不变（[Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html#test)）。
这组数据只证明完整 archive 校验热点的收益，不冒充整套 Gate 的提速。

原始本地报告：`.temp/runtime-layout-digest-benchmark/11a06e9a/report.json`、
`.temp/runtime-layout-inflate-benchmark/1d9113ae/report.json`；记录具体 executable/编译参数及 CPU 时间。

## 已完成的定向验证

- 严格 TypeScript 检查、显式 build、generated 一致性检查通过；JavaScript 行为测试 44/44 通过。
- 两个独立空输出目录生成的文件集合与字节一致。另从无 dist 的源码快照显式构建，
  32 个生成文件与工作区输出完全一致；没有依赖已提交 JS。
  完整仓库源码快照中 runtime/toolchain 的 dist 起初均不存在，复用已锁定的本地依赖后执行
  根 build；证据：`.temp/runtime-layout-clean-workspace/a4e66c38/report.json`。
- 调度器单测 12/12 通过：默认/非法/三轮、去重、并发与独占、失败停止与证据、
  workspace 完整枚举、禁止三轮 workspace、Cargo package/kind 身份与缺失产物拒绝、
  无用例的宿主准备、长报告路径下的真实 Unix socket 与遗留文件保留、源码冻结的输入边界，
  以及清空环境的 supervisor 夹具相对诊断输出归属。
- 新增 DO 已提交响应失败、并发不丢写、确认在途后 SIGKILL/持久恢复断言的 P0.7 实测通过。
- `docs/implemented/g0-results.md` 保持 SHA-256：
  `fc6abe60b2ab46f5dfc9210a8d41f075cc5e50b9a2ede67bef08987395e3abe9`。

## 开发阶段发现与修正

- 第一次 workspace 在新原生宿主加载时触发 workerd version-probe 超时，执行 10/34 个目标后停止。
  macOS 采样显示新 Rust executable 长时间停在 `_dyld_start`，系统日志有 syspolicyd 评估；
  不是业务断言失败。新增一次有界 `--list` 准备并单列耗时，不执行用例、不预热产品状态，
  不修改业务超时或系统安全设置。报告：`.temp/gate-run/failed/20260828T224702-24fd900a/report.json`。
- 准备后的一轮通过全部并行业务目标，但 runtime 库 Unix socket 测试因报告目录路径过长失败。
  改用短的独立 `.temp/gate-tmp/<随机名>`，退出后将非空现场保留至目标报告目录并判失败。
  报告：`.temp/gate-run/failed/20260828T230407-da9f87b6/report.json`。修正后 runtime 库 86/86
  通过，service 库 158/158、P0.1/supervisor/single-binary 定向单轮通过；这些分段结果不代替完整验收。
- 完整 build 覆盖 runtime 源码/构建脚本、toolchain、examples、scripts 的严格类型检查，
  CI 不再先重复调用 typecheck。最新 JS 44/44，2.901 秒。
- Rust validator 对正确资产通过，对陈旧源码和多出的模块拒绝：
  `.temp/runtime-layout-manifest-checks/af01da8b/report.json`。
- 新配置的首次 workspace 已准备全部 34 个宿主，但并行编辑的 P3 规划文档触发了过宽的
  全仓冻结检查，因而在执行任何用例之前停止。所有 runtime 输入摘要未变。保留
  `.temp/gate-run/failed/20260829T002024-1d36c2c6/report.json`，总耗时 1195.80 秒。
  现按源码所有权排除未消费的设计/历史文档，仍冻结全部代码、配置、测试及被内嵌/测试读取的
  `docs/references/`，回归测试同时验证无关文档变化允许、代码和 runbook 变化拒绝。
- runtime 库参与并行的一轮为 84/86：owner-spawn/wait 故障测试没有在原定 1–2 秒窗口内
  观察到子进程，断言失败；service 库 158/158 及其他已执行目标通过。保留
  `.temp/gate-run/failed/20260829T004346-ed536186/report.json`。恢复 runtime 库独占，
  其他五个库并行；没有修改故障用例、预期错误或超时。
  同一 executable 和 runtime 输入恢复独占后 86/86 通过，用时 52.04 秒；并行时为 105.64 秒。
- 首次最终三轮在第 1 轮 23/23 通过后，第 2 轮 P0.4 的即时 pin 数量断言失败（1 而非 0）。
  调度器停止新提交，等待已开始目标清理，共执行 29 个测试进程，不执行第 3 轮；证据保留于
  `.temp/gate-run/failed/20260829T013127-a0413c5b/report.json`，总计 675.86 秒。
  KV streaming response 的 Content-Length 可以先抵达客户端，`spawn_blocking` producer
  随后才析构其 pin。测试改用现有 `ResourcePins::fence_and_wait` 的通知等待（最多 1 秒），
  再做原有零 pin、空 staging 和关停后的零泄漏断言；不改生产路径、不添加固定 sleep。
  此修正晚于并行基准；基准三组自身的源码与 executable 摘要仍相同，不改写测量记录。
  修正后 format、该 target 的 clippy 通过；`p0-2 p0-3 p0-4 p0-5 --jobs 4` 单轮 4/4
  通过（KV 41.64 秒）：`.temp/gate-run/20260829T014628-e5726977/report.json`。
  新 KV executable 的宿主准备为 90.99 秒，单列于执行耗时之外。生产代码未改，完整
  workspace/coverage 不重复；最终三轮从新冻结源码重新开始，不拼接前次失败 aggregate。

## 完整验收

新依赖 profile 下的 format、clippy、no-default-features、MSRV 1.98、metadata、依赖边界、
普通生产构建标记检查全部通过：`.temp/runtime-layout-final-checks/93c0d262/report.json`。
完整 workspace 为 **34/34 宿主、690 通过、0 失败、0 忽略**，一次缓存构建 0.676 秒、
准备 0.111 秒、执行 595.62 秒、总计 598.05 秒：
`.temp/gate-run/20260829T004809-e93d32de/report.json`。冷宿主等待见上方失败准备记录，
不能把这次缓存后的总耗时承诺为首次干净检出的耗时。

coverage 完整 workspace 一轮通过，**56,280 / 62,424 行，90.16%**，门槛保持 90.00%。
34 个测试进程，总计 677.02 秒：`.temp/gate-run/20260829T010547-1439e340/report.json`。
生成 `target/llvm-cov/{summary.json,lcov.info,html/index.html}`。收尾发现 supervisor 夹具的
清空环境子进程写出 13 个默认 profile；逐文件确认只含夹具函数后，原字节及 SHA-256 保留在
`.temp/runtime-layout-profile-diagnostics/20260829-coverage/`，旧有 878 个文件未动。
调度器改为让该测试的相对输出进入本轮目录，不改生产 env_clear、插桩或覆盖率排除规则。
同一插桩 supervisor 宿主修正后单轮通过（31.28 秒），13 个新夹具 profile 全部位于目标目录，
源码树仍只有原先 878 个文件，Gate 临时目录为空：
`.temp/gate-run/20260829T013019-efc1611f/report.json`。本轮未清理或重跑完整 coverage；
它只验证诊断输出归属，不作为新的覆盖率计数来源。报告现同时记录普通、encoded 与
cargo-llvm-cov wrapper 的实际编译参数，区分插桩与普通执行。

最终命令：`OPEN_COMPUTE_GATE_ROUNDS=3 ./test/gate.py all --jobs 4`，在上述 KV 测试修正后
重新冻结源码执行。每轮 **23/23 目标、63 个用例通过、0 失败、0 忽略**，共 **69 个新测试
进程、189 个用例通过**；没有重试或跨轮复用业务状态。只调用一次 Cargo 构建（0.535 秒），
23 个宿主的发现准备 0.084 秒，总计 **1402.57 秒**。

| 最终轮次 | 测试执行秒数 | 目标 | 用例 |
| --- | ---: | --- | --- |
| 1 | 467.50 | 23/23 | 63/63 |
| 2 | 462.72 | 23/23 | 63/63 |
| 3 | 469.97 | 23/23 | 63/63 |

证据：`.temp/gate-run/20260829T014920-2f1a8632/report.json`；核对摘要：
`.temp/runtime-layout-final-acceptance.json`。冻结源码摘要为
`675f7c22986c3858194dc1c860e5bd57a271511069a803bba006b3477c939f3e`，正式 workerd、archive、
pin 和 manifest 与基准输入相同。结束后 `.temp/gate-tmp/` 为空，未发现本次 workerd、
platformd 或测试进程残留；G0 报告摘要和旧有 878 个 profile 不变。归档阶段只调整文档
路径、状态和入站链接，不修改已验证的代码或生成资产。

Linux 特权 egress、正式发行打包及其他宿主验证未在本机执行；
这些事项见 [跨平台发行验收](../runtime-layout-release-acceptance.md)，不将本地验证写成发布资格通过。
