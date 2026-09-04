# 按用例选择验收轮数

日期：2026-08-29。状态：实现、同条件基准、coverage 与重启后的本机最终验收已完成。
macOS 签名服务崩溃尚未根治，不能将本次测试通过描述为系统缺陷已修复；具体限制见末节。

## 范围

最终验收从所有产品 Gate 整套重复三次，改为完整一轮、时序用例补两轮。
`test/gate_cases.py` 是重复归属的唯一清单，`test/gate.py` 负责原生 discovery 核对、
精确用例选择、分轮、并行/独占、失败停止和证据保留。产品 Rust/TS、用例断言、真实 workerd、
SQLite/SigV4 路径、场景内故障点和恢复次数不变。

完整 workspace 的最终命令同时承担普通测试的一次执行，不再先 workspace 一次然后额外
完整产品 Gate 三次。coverage 单独一次、门槛 90.00%，拒绝将插桩测试当最终时序验收。
CI、AGENTS 和持续维护的测试参考同步更新；历史实现/结果文件保留原始运行事实。

## 分类边界

固定输入和显式状态/故障矩阵一次；进程生命周期、在途取消、异步清理、真实 deadline 与
并发调度三次。重要性、阶段编号、使用进程或名称含 crash，不足以决定重复次数。

P2.4 已有独立测试函数，直接在一个宿主中选择其确定性/时序子集，不新增每用例宿主：
版本/重放/终态、snapshot 与 HTTP known/unknown 矩阵一次；真实进程 crash、包含在途
外部效果中断及并发 backlog 的 product bindings、fixture drop/reaping 三次。
P0.2–P0.8 等未分离的混合矩阵保守仍跑三次，不删除其时序覆盖或为追求数量增加重复 setup。

每个登记用例在编译后的 `--list` 中必须恰好存在；新增/删除/改名要求审查分类，不能静默
漏跑。实际执行必须通过全部计划用例，拒绝 ignored、部分通过或零匹配假通过。

## 验收清单

- [x] 调度单测：分轮、混合目标、完整 workspace、精确过滤、零匹配/忽略拒绝、分类漂移、
  并行/独占、失败停止、不把 coverage 当最终验收。
- [x] 真实相关目标单轮验证，核对原生 inventory 与执行计数。
- [x] 相同源码、可执行文件、正式输入和并发度下，对比全部重复与按用例重复。
  区分编译/准备/用例墙钟、CPU 和次数，不把不同缓存状态混算为加速。
- [x] 完成静态检查与一次 coverage。
- [x] 冻结源码的统一 workspace 最终验收通过；此前的宿主阻塞失败记录保留于下文。
- [x] 记录实际结果与限制，将本计划及完成报告归档至 `docs/implemented/` 并更新索引。

本范围不运行 runtime 下载、特权 Linux 网络夹具、发布打包或跨平台宿主；相关发行资格仍见
[活动验收计划](../acceptance/runtime-layout-release-acceptance.md)。现行操作规则见[测试节奏](../references/testing.md)。

## 静态检查与开发验证

- `bun run build`、`bun run check:generated` 成功，JS 44/44。
- 调度回归 20/20；包含原生清单漂移、遗漏整个 Gate、精确过滤/忽略、第二轮失败停止、
  Python 不生成源码树 bytecode，以及 coverage 在清理前拒绝多轮。
- format、clippy（workspace/all-targets/all-features）、no-default-features、MSRV 1.98、
  metadata、依赖边界、production hygiene、shell/YAML 解析通过。
  修正后的原始检查记录：`.temp/test-repetition-checks/833c8d53/report.json`。
- 开发单轮 9 个原生宿主、44 个用例通过，但整次因诊断导入模块生成源码树 bytecode、触发
  输入冻结校验而失败；不把它计为 Gate 通过。记录保留于
  `.temp/gate-run/failed/20260829T024954-c887fbe1/report.json`。
  工具已禁止生成 bytecode；本次产生的缓存原样保存至 `.temp/test-repetition-python-cache/5fb3c524/`。
- 当前原生 discovery 与分类逐项核对为 63 个产品用例，其中 20 个确定性、43 个时序；
  完整轮与追加轮分别实际通过 63 与 43 个用例；统一 workspace 最终验收见末节。
- 首次同源码对比的完整轮 63/63 通过，用例阶段 459.12 秒；追加轮 P2.5 大回放在第 16 路
  结果提交前遇到 workerd socket `No buffer space available`，40 秒后失败。数据库保留了
  15 个已提交步骤与一个未提交步骤，未作为性能成功证据。调度器停止后续提交，不执行
  supervisor/single-binary，已在运行的目标完成清理。测量记录：
  `.temp/test-repetition-benchmark/failed/aed99901/report.json`；真实失败现场：
  `.temp/p2-4-run/failed/workflow-zwCxu5/`。
  P2.5 product 已移出进程并行集合；16 路并发、约 1 MiB/结果和原有超时保持不变。
  修正后的单轮 2/2 通过，用例阶段 20.74 秒、总计 21.61 秒：
  `.temp/gate-run/20260829T031747-8680fb9e/report.json`。后续重测已通过，见下节。

## 同条件追加轮对比

2026-08-29，macOS 26.6.2 / arm64，12 核，`--jobs 4`，进程内 `--test-threads=1`。
两组共用一次编译和 discovery 准备、同一组可执行文件、相同正式 pin 与生成资产；
每组重新启动测试进程并创建独立业务状态。两组都包含修正后的 P2.5 product 独占屏障。

| 实测策略 | 测试宿主 | 通过用例 | 用例墙钟 | 子进程 CPU |
| --- | ---: | ---: | ---: | ---: |
| 完整轮（原追加轮的用例集合） | 23 | 63 | 497.65 秒 | 189.35 秒 |
| 仅时序追加轮 | 18 | 43 | 413.81 秒 | 159.09 秒 |

追加轮墙钟减少 **83.85 秒 / 16.85%**。编译 0.265 秒、23 个原生宿主准备 0.083 秒单列，
不计入这组用例墙钟对比。P2.4 product 完整矩阵 175.93 秒，追加轮 93.84 秒；
P2.5 product 两组均独占通过，20.56 / 20.10 秒，原有并发和数据规模未改。

每种策略只有一个样本，执行顺序为完整轮后追加轮；不是统计显著性或固定提速承诺，
也不是旧、新整套最终验收总时间的同条件实测。新统一 workspace 命令还消除了原先
“workspace 一次再额外完整产品 Gate 三次”的重复首轮；实际最终耗时另行记录。
首次失败测量不拼入本次数据。

源码摘要：`13e0e7426bd3d1c0de0405a1b1aea80345e79728275e857530eee08fa3e1a8b8`。
正式 workerd：`v1.20260826.1`；binary SHA-256：
`2d17da54d2671d6e9e7c776d56b934f60be8c140b9bac35ddf22f60d6cff9403`。
原始报告与实验脚本：`.temp/test-repetition-benchmark/175dd6ea/report.json`、同目录 `run.py`。
报告保留 revision、完整 pin/资产/可执行文件摘要、每目标用例及耗时；实验脚本不是新的维护入口。

## Coverage

`./test/coverage.sh` 一次成功退出：34 个 workspace 测试宿主、690/690 用例通过。
Rust 行覆盖率 **90.15%**（56,276 / 62,424），门槛仍为 90.00%，排除规则未改。
源码摘要与上述基准相同；插桩二进制不作为最终时序验收证据。

构建 38.27 秒、原生宿主准备 32.25 秒、用例阶段 594.30 秒；Gate 总计 666.32 秒，
不含其后的 coverage 报告生成时间。Gate 原始记录：
`.temp/gate-run/20260829T033554-85441e32/report.json`。
生成的 summary 原字节另存至 `.temp/test-repetition-coverage/b93c7778/summary.json`，
常规报告仍在 `target/llvm-cov/`。

## 首次最终验收失败：宿主执行阻塞

最后执行 `OPEN_COMPUTE_GATE_ROUNDS=3 ./test/gate.py --workspace`，源码摘要仍为上述值。
34 个原生宿主均完成 discovery；首轮已启动 18 个宿主时发生以下失败：

- P0 Exit 在正式 workerd 的 version probe 处超时。
- Service 库为 156/158；真实 runtime 的独立 admin listener 启动超时，随后启动失败矩阵
  得到 `RuntimeInvalid` 而非预期 `SecretRefInvalid`。
- 已完成 discovery 的 P1 conformance/crash 宿主再次启动时未进入测试框架，输出为空。
  同时，新启动的只读 `python3` 与 `rg` 诊断命令也阻塞。

对本次 Python、P1 conformance/crash 进程的 `/usr/bin/sample` 采样均显示唯一主线程停在
`_dyld_start + 0`、没有已加载 binary images；Python footprint 为 96 KiB。两个 P1 宿主
的磁盘 SHA-256 与成功基准记录一致。同期系统日志包含 notarization daemon 检查错误和
`GatekeeperPolicyScanError -67018`。这些证据指向宿主在进入程序主体前的执行检查阻塞，
不能据此认定需要修改测试业务逻辑；具体系统原因及恢复操作尚未验证。

调度器没有进入第二、三轮，也未自动重试。核对 PID、父进程、启动时间、可执行文件与摘要后，
终止本次尚未进入主体的两个测试宿主及三个只读诊断进程，使失败运行退出并保留报告。
未重启系统服务、关闭执行检查、修改 quarantine/签名、放宽测试超时或改变安全边界。

此次最终运行 **FAILED**，总计 468.54 秒；不是最终通过证据：
`.temp/gate-run/failed/20260829T034740-2217ca8e/report.json`。
进程采样与同期系统日志保留在 `.temp/test-repetition-host-diagnostics/20260829T0353/`。
之前成功的基准、静态检查和 coverage 结果保留其实际结论，不拼成缺失的最终轮次。

当时的后续安排：先恢复宿主正常执行能力，再从首轮重新执行统一最终命令；通过后核对 34/18/18
宿主与 690/43/43 用例，检查清理，再归档本文。发行打包、特权 Linux egress 和跨平台
资格仍由各自的活动验收计划承担，不包含在这次本机结果内。

## 用户要求重试后的结果与修复诊断

2026-08-29 04:00，按用户要求重试。开始前 Python、rg、正式 workerd 的启动探测均成功，
`bun run build` 成功，源码及正式输入摘要与成功基准、coverage 一致；没有重跑已完成的
静态检查或 coverage。34 个原生宿主的 discovery 全部通过。

首轮再次在 P0 Exit 的 workerd version probe 超时；Service 库再次为 156/158，相同两个
runtime 启动用例失败。P1 conformance/crash 进程再次停在 `_dyld_start`，没有进入测试框架。
核对启动身份与二进制摘要后终止这两个挂起进程，调度器成功保留失败报告并退出，未进入追加轮：
`.temp/gate-run/failed/20260829T040017-5ca040b6/report.json`，总计 **284.34 秒，FAILED**。

随后按用户的修复要求读取宿主崩溃记录，确认直接阻塞原因：

- `com.apple.security.syspolicy` 的 launchd 状态为 `spawn scheduled`，上次终止信号为
  `Segmentation fault: 11`；它运行 `/usr/libexec/syspolicyd`。
- `syspolicyd-2026-08-29-040158.ips` 记录 `consecutiveCrashCount = 18`、
  `throttleTimeout = 1200`，崩溃在 `syspolicyd.evaluations.scans` 队列，访问地址 `0x8`。
- 栈顶为 `Security::Universal::architecture()`，调用链来自
  `MachORep::signingData()` 和 `SecStaticCodeCheckValidityWithErrors`。
  03:38、03:50 两份崩溃记录具有相同签名解析调用栈。

原始崩溃报告的字节副本、launchd 状态和进程采样保留在
`.temp/test-repetition-host-diagnostics/20260829T040017-5ca040b6/`；系统原报告未修改。
Apple 的[公开签名解析实现](https://github.com/apple-oss-distributions/Security/blob/main/OSX/libsecurity_codesigning/lib/machorep.cpp)
包含这条调用链，但公开源码不能证明本机系统二进制的全部细节。尚未证明哪个文件或生命周期
事件触发了系统缺陷，不把临时可执行文件清理或进程并发直接定为根因，也不据此改生产安全边界。

诊断后的恢复步骤如下。恢复服务本身不等于根治系统缺陷，也不替代最终验收。

## 系统服务自行恢复后的单轮验证

用户显式授权后执行 `sudo /bin/launchctl kickstart -k system/com.apple.security.syspolicy`，
命令退出码为 150，返回 `Operation not permitted while System Integrity Protection is engaged`。
没有关闭 Gatekeeper/SIP、删除系统数据库、修改隔离属性、重新签名或重启整机。

04:19 的只读检查显示 launchd 已自行重新启动服务：PID 69926，启动时间 04:18:45，
`runs = 41`、`successive crashes = 40`、状态 `running`。不能将自行恢复记作手动重启成功。
Python、rg 和正式 workerd 启动探测均成功，源码与正式输入摘要仍与基准、coverage 相同。
`bun run build` 再次通过后，执行受控单轮检查：

- P0 Exit：1/1 通过，用例阶段 91.23 秒，总计 92.17 秒；记录为
  `.temp/gate-run/20260829T042027-bc9a7500/report.json`。
- Service 库只执行此前失败的独立 admin listener 与启动失败资源清理两个用例：2/2 通过，
  进程墙钟 33.57 秒，其余 156 个用例未运行；不是完整 workspace 验收。

命令结果、服务状态和两个启动用例的日志保留在
`.temp/test-repetition-host-diagnostics/20260829T041940-syspolicy-recovery/`。
这些结果仅证明当时的执行能力已恢复，不证明签名解析崩溃不会复发。

### 服务自行恢复后的最终验收仍失败

04:23 从首轮重新执行冻结源码的统一最终命令。34 个宿主 discovery 通过；04:25:27，
`syspolicyd` 再次发生 `SIGSEGV`，访问地址仍为 `0x8`，launchd 回到 `spawn scheduled`。
本次服务从 04:18:45 启动至再次崩溃仅约 6 分 42 秒，不能将自行恢复认定为根治。
新报告为 `syspolicyd-2026-08-29-042529.ips`，其中 `consecutiveCrashCount = 19`、
`throttleTimeout = 1200`；launchd 的 `successive crashes = 41` 是另一项计数。

P0 Exit 再次在 workerd version probe 超时；Service 库再次为 156/158，相同两个启动用例
失败。P1 conformance/crash 再次停在 `_dyld_start`，footprint 均为 96 KiB，未进入测试框架；
磁盘可执行文件摘要仍与成功基准相同。核对启动身份、父进程与摘要后终止这两个挂起宿主。

本次 **FAILED**，总计 **240.46 秒**，没有进入第二、三轮或自动重试：
`.temp/gate-run/failed/20260829T042346-b1bd0047/report.json`。
崩溃记录副本、采样及服务状态保留在
`.temp/test-repetition-host-diagnostics/20260829T042346-b1bd0047/`。
退出后未发现本次测试宿主或 workerd 残留，`.temp/gate-tmp/` 为空。

这次失败后停止继续重跑相同测试，等待宿主恢复措施。当时尚未执行整机重启，也没有证据保证
重启能消除签名解析缺陷；未关闭 SIP/Gatekeeper 来绕过阻塞。当时最终验收未通过，本文保持
活动状态。用户随后重启系统，后续结果如下。

## 用户整机重启后的本机最终验收

系统启动时间确认为 2026-08-29 04:32:35。开始前确认 `syspolicyd` 为本次启动的首次运行，
Python、rg 与正式 workerd 均可正常启动；源码摘要仍为上述 `13e0e742…a8b8`，正式 pin、
archive 和 binary 校验通过。`bun run build` 成功后，从第一轮执行统一最终命令。
静态检查与 coverage 的源码没有变化，沿用其既有通过记录，不重复执行。

同一次连续运行成功退出，逐项核对原生 discovery、每轮目标、实际用例、退出码与输入冻结：

| 阶段 | 测试宿主 | 通过用例 | 用例墙钟 |
| --- | ---: | ---: | ---: |
| 完整首轮 | 34 | 690/690 | 589.44 秒 |
| 时序追加第二轮 | 18 | 43/43 | 385.47 秒 |
| 时序追加第三轮 | 18 | 43/43 | 404.02 秒 |
| 合计 | 70 | 776/776 | 1,378.93 秒 |

没有 ignored 用例、失败后重试或拼接历史轮次。Cargo 构建 0.89 秒，34 个原生宿主准备
384.96 秒；Gate 总计 **1,766.78 秒（29 分 26.78 秒）**，子进程 CPU 合计 543.47 秒。
重启后的原生程序启动准备明显慢于此前缓存已热的运行，单列准备成本，不将本次总时间当成
同缓存条件下的旧/新整套性能对比。上文 16.85% 的追加轮收益仍只来自那次独立同条件基准。

原始记录：`.temp/gate-run/20260829T043702-c104329b/report.json`。
源码和正式输入与成功基准一致，coverage 记录同一源码的 **90.15%** 行覆盖率仍有效。
退出后核对未残留本仓库测试进程或 workerd，`.temp/gate-tmp/` 为空，`test/` 无新 bytecode。
清理检查记录：`.temp/test-repetition-host-diagnostics/20260829T043702-c104329b/cleanup.json`。

### 已知宿主限制：整机重启没有根治签名服务崩溃

本次首轮期间，PID 237 的 `syspolicyd` 于 04:45:08 再次在
`Security::Universal::architecture()` / `MachORep::signingData()` 调用链访问 `0x8`，
以 `SIGSEGV` 退出；报告中的 `throttleTimeout = 10`。系统自行重新启动为 PID 3482，
后续测试继续运行并全部通过。验收结束时 launchd 为 `running`、`runs = 2`、
`successive crashes = 1`；验收结束前未观察到本次开机后的第二次服务崩溃。

这说明重启恢复了本次验收所需的执行能力，但没有修复签名解析问题。没有关闭安全检查、
放宽超时、跳过测试或修改产品安全边界；本记录不宣称已经定位或修复系统缺陷的触发条件。
启动时间、崩溃原字节副本和前后服务状态保留于
`.temp/test-repetition-host-diagnostics/20260829T043702-c104329b/`，系统原报告未改动。

本机测试流程改造完成验收；宿主系统问题仍待后续排查，发行打包、特权 Linux egress、
跨平台与 CI 执行资格仍由各自的活动验收计划承担，不扩大本次通过结论。
