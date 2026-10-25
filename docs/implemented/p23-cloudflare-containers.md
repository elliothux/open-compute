# P23：Cloudflare Containers 基础实现

状态：**implemented，本地候选验收已有记录；正式 tag 资格失败，公开发布未完成**（2026-10-25，UTC）。
本页保留各批次的原始范围和结果；成员清单声明不替代发行验收。
以下基础验收段落保留 R9 历史结果；当前 R22 状态见下文 2026-10-23 记录。
正式 R9 的 exact3 已 verified；workspace coverage 已按生产字节等价复用证据。
最终 workspace Gate 为 failed；Linux 两平台本次 package 范围完成。
Darwin Q10 原四项 package 用例 4/4 PASS；测试进程已回收。
两组 ACL 已恢复；临时账号、home 和 staging 已删除，账号进程为零。
基础验收时的 capability inventory 包含 64 个稳定 Container 成员，成员资格均为 `blocked`。

## 2026-10-25 R2 测试客户端独立推进

正式工作流 [40155138065](https://github.com/elliothux/open-compute/actions/runs/40155138065) 的普通及覆盖率任务均在 P0.5 失败。
两项轻量请求耗时分别为 2.232 秒和 2.391 秒，超过原 2 秒门槛。
两项任务均执行 50／63 组，报告 2024 项通过；完整发行资格仍未完成。
全部 18 项任务已结束：14 项成功、2 项失败，组装和发布两项均跳过。
五类压力场景均通过既有门槛；soak 完成 3632 秒，三次重启均完成恢复核对。
压力证据对应源码 `c99f507c302402e050d94ce121bc47b668082bea`，不替代当前改动的资格。

本地 Darwin ARM64 的确定性模型确认了客户端推进问题。
探测分支等待 HTTP 时，原写法停止轮询上传流水线。
原模型为 1 项失败；独立任务模型为 1 项通过，另有 2 项过滤。
客户端改用一个所属任务推进原四路上传；测试退出时，任务容器取消未完成上传。
生产服务、正式 R22 pin、上传内容、分片持久化和全部原断言保留。

首次候选的无诊断普通复验失败。
同日，Darwin ARM64 在读回 223106803 字节后中断，读回耗时 11.385 秒。
当时结果为零项通过、一项失败，另一项过滤；插桩复验尚未启动。
随后含安全诊断的真实用例完整读回 241910375 字节，并通过原摘要和延迟断言。
诊断输入不同，不能替代当前源码的普通、插桩或 Intel 资格。
提前关流的唯一原因仍未确定；未据此放宽超时或内存限制。

撤回全部临时诊断后，Darwin ARM64 的普通及插桩用例均通过。
两轮均为一项通过、零失败、零忽略，另一项过滤。
两轮均读回全部 241910375 字节，并通过原完整 SHA-256 断言。
普通轮次最长轻量请求为 197 毫秒，插桩轮次为 224 毫秒。
所属 Clippy、格式、baseline 身份和文档检查均通过。
测试输入保留原四路上传、2 秒门槛、持久化及清理断言。
证据保存于 `.temp/release-0.4.0-actual/r2-independent-upload-polling-candidate/`。
旧读回失败仍保留；本地通过不证明提前关流的唯一原因。
当前源码的正式最终 Gate、完整覆盖率和公开发布仍待完成。

## 2026-10-25 R2 测试客户端校验隔离

正式工作流 [40137635987](https://github.com/elliothux/open-compute/actions/runs/40137635987) 的两项 Intel 任务在 P0.5 失败。
两项任务均执行 50／63 组，报告 2024 项通过。
普通任务的轻量请求耗时 2.733 秒，超过原 2 秒门槛。
覆盖率任务在整对象读回时收到 EOF，未生成合格覆盖率百分比。
同轮两项任务中的 P0.2 均为 3 项通过，P0.4 均为 1 项通过。
此前的 cf 与 KV 失败已通过本轮真实宿主执行；完整发行资格仍未完成。

本地采样发现测试客户端的数据生成占用 CPU。
2026-10-25 UTC，Darwin ARM64 的逐字节对照核对全部 241910375 字节。
原生成器耗时 2660 毫秒，周期复制耗时 20 毫秒，完整内容与摘要相同。
测试只替换上传数据生成器；期望摘要继续使用原逐字节公式。
客户端先把真实响应流写入有长度上界的临时文件，再由阻塞任务计算完整摘要。
生产 R2、正式 R22 pin、上传量、四路并发、服务超时和原断言均未改变。

同日，本地 Darwin ARM64 的普通及插桩用例均为 1 项通过、零失败、零忽略，另外 1 项过滤。
两轮耗时分别为 31.26 秒和 32.28 秒，最长轻量请求为 209 毫秒和 212 毫秒。
两轮均完成原有完整 SHA-256、分片 authority、D1、暂存清理与进程退出检查。
证据保存于 `.temp/release-0.4.0-actual/r2-client-work-isolation-candidate/`。
本地结果不证明 Intel 延迟或 EOF 的唯一原因，也不替代完整覆盖率与最终 Gate。
在上述本地记录时，原正式运行的压力任务尚未结束；本任务未公开 Release 或 npm 包。

## 2026-10-24 KV 连接关闭与 cf 重试回归

正式工作流 [40098528822](https://github.com/elliothux/open-compute/actions/runs/40098528822) 的第二次资格运行有两项失败。
macOS 最终 Gate 执行 13／63 组，报告 1782 项通过；P0.4 的大值请求返回 500。
覆盖率任务执行 49／63 组，报告 2020 项通过；P0.2 观察到三个 Version，原断言只接受两个。
两项任务均未完成完整验收，覆盖率未生成合格百分比。
原始报告没有记录 KV 的具体失败阶段，也没有记录第三个 Version 的状态。
因此，下述本地回归不证明两项 CI 失败的唯一原因。

真实 TCP 回归确认了 KV 的连接关闭缺陷。
私有后端拒绝带未读 body 的超限 PUT 时，原响应没有声明关闭 HTTP/1 连接。
原回归在 413 响应的关闭标记断言失败；修复后的同一回归通过。
修复只添加关闭标记，保留原错误、25 MiB 限制、权限检查和 pin 释放。
回归还检查暂存文件清理与所属服务退出。

真实 cf CLI 回归确认了原 Version 数量断言不成立。
测试在成功提交上传后丢弃一次响应；官方 CLI 重试后生成三个 ready Version。
原精确数量断言失败；新断言按实际上传次数检查数量，并要求全部 ready。
测试核对 CLI 返回的唯一 Version、相同 bundle 内容及独立版本描述符。
原流量、promotion、rollback、内容和 secret 断言保留。
故障注入只在测试中，不进入生产服务。

2026-10-24 UTC，本地 Darwin ARM64 的检查均以正式 R22 输入通过：

- 完整 P0.4 KV 用例为 1 项通过、零失败、零忽略，耗时 28.15 秒。
- cf 重试用例在普通及插桩模式下各 1 项通过、零失败、零忽略。
  两轮各过滤另外 2 项，耗时分别为 23.19 秒和 23.16 秒。
- KV TCP 回归为 1 项通过、零失败、零忽略，另外 1001 项过滤。
- 完整静态批次的 10 个步骤均退出成功。
  其中包含五组 Clippy、无默认功能、MSRV 和生产二进制检查。

产品用例测量后的改动只补充安全的断言失败信息；条件与成功路径不变。
最终源码身份与静态结果保存于 `.temp/release-0.4.0-actual/cf-kv-repair-static-actual/root-actual-review.json`。
产品结果保存于同目录树的 `cf-kv-current-repair-local-batch/current-actual-review.json` 和 `kv-rejection-real-runtime-green/result.json`。
本地结果不替代 Intel 宿主、90% workspace 覆盖率或正式最终 Gate。
正式 R22 pin、runtime assets、资源上限和验收时限均未改变。
公开 Release 与 npm 发布仍未完成。

## 2026-10-24 R2 测试服务调度修复

正式工作流 [40085275371](https://github.com/elliothux/open-compute/actions/runs/40085275371) 的两项升级测试已通过。
service 库测试为 993 项通过、零失败、零忽略。
最终 Gate 随后在 P0.5 上传响应检查失败：最长 GET 为 2.4066 秒，原门槛为 2 秒。
同轮覆盖率任务在同一用例的完整回读阶段收到 EOF，尚未生成覆盖率结果。
两种失败分别保留；本地结果不证明 Intel 上两种症状的唯一原因。

测试用 MockS3 与应用共享 Tokio 调度器，原服务在异步任务中计算整块校验值。
受控回归调用真实分段上传 handler，先验证完整内容和 ETag，再检查应用能否取得调度机会。
原实现失败；修复把已有校验计算和分段拼接移到拥有数据的阻塞任务。
生产 S3、R2、runtime、R22 pin、上传量和原验收断言均未改变。

修复后的本地 Darwin ARM64 检查通过：124 项所属单元测试、Clippy、普通及插桩 P0.5 各 2 项用例。
所有用例均为零失败、零忽略。
两轮真实上传均为 241910375 字节、4 路并发，并完成整对象 SHA-256 回读。
普通轮次最长 GET 为 142 毫秒，插桩轮次为 314 毫秒，均低于原 2 秒门槛。
本地定向插桩结果不替代 90% workspace 覆盖率、最终 Gate 或正式 tag 资格。
公开 Release 与 npm 发布仍未完成。

## 2026-10-24 正式 tag 宿主断言

正式工作流 [40072654539](https://github.com/elliothux/open-compute/actions/runs/40072654539) 已进入最终 workspace Gate。
本轮 Broker 资格通过，主 Gate 成功启动。
service 库测试为 991 项通过、2 项失败、零忽略。
两项升级默认值测试把 macOS Intel 当作正式安装目标，并直接解包成功结果。
生产代码按既定平台清单返回 `RELEASE_UNSUPPORTED`；项目不发布 Darwin Intel 二进制。

修复仅让两项测试在 macOS Intel 上断言拒绝结果。
三个正式平台保留原有成功断言；生产代码、R22 pin 与资源限制均未改变。
修复后的本地 Darwin ARM64 构建及两项定向测试通过，零失败、零忽略。
本地 ARM64 结果不证明 Intel 拒绝分支的新测试已经执行。
原始 tag Gate 失败记录保留；修复后的正式资格与公开发布仍未完成。

## 2026-10-24 Intel Broker 单项诊断

单项诊断 [40064512629](https://github.com/elliothux/open-compute/actions/runs/40064512629) 捕获了资源探针超时过程。
探针先写满受限磁盘，再调用 `wc -c` 读取整个文件以检查长度。
在 1/16 CPU 限制下，Intel 采样显示 `wc` 在十秒期限到达时仍未结束。
当时根目录标记尚未出现；探针尚未进入快照或 sidecar 检查。
修复用 `stat` 读取同一普通文件的长度，保留容量断言与十秒期限。
CPU、内存、磁盘、进程和文件数限制均未改变。

修复后的当前源码已完成本地 Darwin ARM64 构建。
真实 VM 保持 2 CPU、4 GiB 内存、8 GiB 系统盘及 16 GiB 数据盘。
资源探针在 0.17 秒后进入快照检查；真实资源、快照与 sidecar 资格通过。
Broker、socket、VM 均已清理，Engine 库存与宿主原有目录身份保持不变。
四项资源探针回归通过，零失败、零忽略；持续错误仍由原十秒期限拒绝。
本地结果不替代 Intel 正式 Gate、覆盖率或 tag 资格。
已完成的临时诊断入口和采样器已移除；原始调查证据保留。

修订后的正式工作流 [40054104614](https://github.com/elliothux/open-compute/actions/runs/40054104614) 绑定提交 `c89b6766991e7f7e1d494f7309c796cd4d6a19c8`。
三平台打包与十项安装验收通过。
两个 Intel macOS 任务均通过 Engine、XFS 与 helper 准备，但 Broker 启动资格失败。
两份报告仍为 `mainExecuted = false`，Broker 输出为 `CONFIG_INVALID`。
coverage 与最终 workspace Gate 均未启动；具体拒绝条件尚未捕获。
剩余压力任务已取消，原始失败和成功结果分别保留。

本地 ARM64 探针使用 Colima 0.10.3、Lima 2.2.0 与 Docker 29.5.2。
VM 保持 2 CPU、4 GiB 内存、8 GiB 系统盘及 16 GiB 数据盘。
真实 Broker 资格、快照能力握手和退出清理通过。
探针使用 `d37501b87aa997bffea8e6be767b78441f1d81b1` 的 Darwin ARM64 包。
当前 Broker Rust 源码与该候选相同；当前 tag 的二进制身份仍由正式打包任务验证。
原有 Docker、Lima 与 Colima 目录身份保持不变，探针的 VM 和子进程均已回收。
本地探针未执行产品 Gate，也不证明 Intel CI 资格。

CI 诊断记录仅包含事件时间、动作类别、镜像角色和退出码。
子进程的事件文件上限为 1 MiB，不记录命令、容器 ID、名称或标签。
诊断另记录 socket 对端 UID／GID，以及已过滤的转发计数；原 Rust 身份检查继续执行。
真实 exec canary 检查取得 25 条事件，共 2190 字节；事件文件没有 canary。
本地文件上限回归确认子进程受限，父进程的上限不变。
临时 Intel 诊断入口只执行了 Broker 资格和清理，未执行产品 Gate、coverage 或发布。
该入口用于取得缺失的拒绝证据，不能替代正式资格。

单项诊断 [40060316438](https://github.com/elliothux/open-compute/actions/runs/40060316438) 在资源探针启动后失败。
探针启动至强制清理相隔 10.33 秒，尚未执行快照或 sidecar 检查。
socket 对端 UID 为 501，Docker 与 XFS 配置检查通过。
该轮尚未捕获具体标记读取条件；转发错误计数不能单独证明根因。
后续单项诊断读取了探针的进程类别、文件存在状态与归类后的 archive 响应。
诊断不记录进程参数、文件内容或原始错误，也不改变资源上限与原验收条件。

报告组装器原来读取旧 macOS 目录名，与 Intel 工作流上传名称不一致。
新增回归先复现目录缺失；修复后 35 项发布工具检查通过。
Container 执行源码、R22 pin、原用例断言和资源上限均未改变。
当前版本尚未创建 GitHub Release 或 Draft，也未执行 npm 发布。

## 2026-10-24 正式发布环境修复

正式工作流 [40047706451](https://github.com/elliothux/open-compute/actions/runs/40047706451) 的 macOS workspace 与 coverage 任务均在环境前置阶段失败。
两份原始报告均为 `mainExecuted = false`，错误为 `COLIMA_ENGINE_SOCKET_REQUIRED`；主 Gate 与 coverage 均未启动。
固定 Colima 0.10.3 仅在 `COLIMA_HOME` 目录已存在时采用该路径。
启动器未创建目录，导致 Colima 使用默认目录，而守卫检查原定私有目录中的 socket。

修复在首次调用 Colima 前创建 mode `0700` 的私有目录。
原回归先得到 3 项失败与 3 项错误；修复后 9 项全部通过。
5 项调用方检查与 24 项发布契约检查通过。
固定 ARM64 与 x86_64 Colima 的真实本地命令均选中私有目录，宿主原有目录身份不变。
x86_64 命令使用已安装的 Rosetta；本地探针未创建虚拟机、Engine 或执行 Gate。
资源上限、原断言、退出清理、Container 执行源码与 R22 pin 均未改变。
旧失败日志和报告原样保留；正式版本与 Draft 均未创建。
修复提交仍需通过 main 检查与修订 tag 的正式资格，不能用本地探针替代。

## 2026-10-24 产品展示面更新

本批同步双语 Container 指南、README、架构图、实例配置、CLI 与内嵌运维入口。
固定 cf 使用的 13 条管理路由已纳入能力清单，并标注本机管理限制。
固定官方 SDK 未提供对应方法；生成的 SDK 报告明确登记排除项。
65 项 tenant API、Container 执行源码与 R22 pin 未变。
以下候选测量保留原 revision；本批变更后的正式资格由 tag 工作流重新绑定。
2026-10-24，本地 SDK 严格类型检查、构建、19 项测试和四种隔离消费者检查均通过。
P6 管理契约回归 7 项通过；能力清单的五项定向一致性检查通过。
官网完整构建通过，双语文档各 72 页，静态输出共 147 页，内部链接有效。
HAI 检查保留旧文档提示；新增与修改行没有硬规则问题。

## 2026-10-23 当前验收

R22 正式输入下，未插桩的最终 workspace Gate 成功退出。
本批次执行 63 个目标，2087 项通过，零失败。
P23 三项真实产品用例全部通过，零忽略，耗时 600.44 秒。
用例覆盖 exec、普通容器完整流程和 rootfs 实体排除。
目录恢复、快照和重启断言均通过。
运行器在全部用例期间保持 4 GiB 内存、2 核 CPU。

外层运行器仍以退出码 1 结束，原始失败记录保留。
原因是依赖检查保留了编译前的文件描述符。
构建替换了三个依赖文件；旧描述符的链接数变为零。
最后一次替换早于首项用例 524.23 秒。
独立补查确认八个依赖的 SHA-256 均与原固定值相同。
已有 BLAKE3 固定值的六个依赖也全部匹配。
补查验证 3380 个源码与正式输入文件的身份和摘要。
原检查另确认 68 个当前程序、349 个宿主保留文件及历史归档保持不变。
补查没有改写旧文件身份，也没有重跑编译或用例。
这些证据分别证明最终用例结果和补充输入核对，不将外层失败改为成功。

最终 Gate 报告为 `.temp/gate-run/20261023T175731-7f3ea228/report.json`。
报告的 SHA-256 为 `38d4cdb63d0a76baa3721e3b9362472984651f89d3c6174fcbed73d215951fd9`。
补充复核位于下文证据目录内的 `canonical-coverage-seven-gib-compiler/workspace-final-current-preparation/final-execution/retained-dependency-failure-readonly/root-canonical-and-completion-review.json`。
同一候选的三平台包和十项安装验收已通过。
原 x64 压力恢复测试已成功结束；正式 tag 工作流尚未执行。

候选源码为 `d37501b87aa997bffea8e6be767b78441f1d81b1`。
Darwin ARM64、Linux ARM64 和 Linux x64 均通过原四项单文件用例。
ARM64 Dashboard 的原运行器成功退出。
运行器要求两项用例、进程回收和资源后置检查全部通过。
原始子进程报告已丢失；命令历史保留原退出码和摘要一致的运行器源码。
上述回读不重建原文件身份，也没有重跑用例。

macOS、Ubuntu 24 和 Debian 13 的 user／system 安装矩阵为 10／10 通过。
四项 ARM64 安装范围取得 630 次资源采样，全部为 4 GiB、2 核 CPU。
原安装器验证首装、服务生命周期、KV／Worker 数据保留和预期升级拒绝。
本轮测试容器均已回收，输入、Engine 库存和资源配置保持不变。

证据为 `.temp/p23-release-current-recovery/root-ten-scope-installation-review.json`、
`root-current-three-binary-and-installation-review.json` 和
`root-historical-package-dashboard-readback-review.json`。
[macOS 候选资格](https://github.com/elliothux/open-compute/actions/runs/39953290637)与
[x64 候选资格](https://github.com/elliothux/open-compute/actions/runs/39955711933)绑定同一源码。
这些候选结果不替代正式 tag 工作流，也不证明公开发布已完成。

x64 压力测试在 2 CPU、4 GiB 下完成五档检查。
`smoke`、`p0-2c4g`、`scenario`、`p1-2c4g-peak` 和 `p1-2c4g-soak` 均通过。
完整强度档保持 scale 为 1，全部 stack 的原错误率和延迟门槛均通过。
测量共记录 1228944 个请求样本及 683 个错误；结果不代表零错误。
报告中的全局异常为零。
一小时恢复档实际完成 3632 秒，并完成三次注入重启后的恢复核对。
原报告绑定上述候选源码和 x64 包摘要，没有重跑压力测试。
报告归档的 SHA-256 为 `1f368751c14d6b113944c9cf6f1a85c3a757e5eb5b741f312dbebc724a870681`。
指标复核为 `.temp/p23-release-current-recovery/original-stress-terminal/root-terminal-stress-review.json`。

R22 正式输入下，完整 coverage 批次通过。
本批次执行 63 个目标，2087 项通过，零失败。
Rust 行覆盖率为 170010 / 187312（90.763%）。
运行器以 7 GiB 内存、2 核 CPU 编译，并在用例前恢复 4 GiB 内存、2 核 CPU。
原编译参数、用例断言和时限均保留；编译阶段没有新增 OOM。
用例清单核对、源码与输入检查、进程库存和资源后置检查均通过。

P23 整组为 3 项通过，零失败、零忽略，耗时 864.81 秒。
原用例覆盖普通容器完整流程、exec 和 rootfs 实体排除。
普通流程中的目录恢复、快照和重启断言均通过。
此前第二次目录恢复的具体拒绝条件仍未捕获。
两处固定阶段日志只作为诊断记录，不作为唯一根因修复的证明。

`containers.native` 合同为 `passed`。
平台级合同报告仍为 `no_go`，保留其他产品缺口和未执行的托管对照。
该平台级状态不改变已接受的两项快照兼容性差异。
本 coverage 批次不替代上述未插桩最终 Gate 或三宿主发行包验收。

证据：下文所列证据目录内的 `canonical-coverage-seven-gib-compiler/root-terminal-full-coverage-review.json`。
原始报告为 `.temp/gate-run/20261023T141040-342adf2c/report.json`。
覆盖率报告为 `.temp/coverage/run-3p6V9rgj/reports/summary.json`。

## 2026-10-22 当前验收

R22 正式输入下，完整 coverage 批次执行 63 个目标。
运行器在编译阶段使用 8 GiB 内存、2 核 CPU。
运行器在首项用例前恢复 4 GiB 内存、2 核 CPU。
本批次耗时 4049.01 秒：47 组通过，16 组失败。
报告计入 2059 项通过；批次未生成合格覆盖率报告。

P23 三个真实用例均通过，耗时 877.79 秒。
这三项覆盖 exec、普通容器完整流程和 rootfs 实体排除。
测试留下空的 `p23-product` 目录，所以运行器仍将 P23 组判为失败。
其余 15 个失败目标在共享 FUSE 目录触发本地数据库检查。
原始失败报告、剖析数据和 68 个编译产物均保留。

本批次后，9 个测试文件改用验收器指定的本地临时目录。
浏览器用例复用既有证据所有者，清理成功状态并保留失败证据。
生产代码、运行时资产、固定 65 成员和正式 R22 输入均未改变。
16 组定向复验已结束：14 组通过，2 组失败，耗时 3494.37 秒。
运行器计入 23 项通过；全部 16 组均完成用例清单检查。
`workflow-recovery` 为 3 项通过、2 项失败。
两个失败的次级夹具仍强制使用共享目录，现已改为继承本地 `TMPDIR`。
两处修复的后续真实复验已通过：5 项通过、零失败、零忽略。

本轮 P23 的 exec 和 rootfs 用例通过，普通用例失败。
第一次具名快照捕获完成；第二次省略参数的快照捕获遭拒绝。
底层事件记录两次暂停和恢复；第二次捕获未进入发布阶段。
失败边界包括恢复后的校验和归档回传，唯一原因尚未确定。
原始响应解析断言、捕获校验、限额和时限均保留。
该组为 2 项通过、1 项失败；固定字段诊断仅保留为候选。
批次后置检查记录资源不足：VM 剩余配额约 2.55 GB，宿主约 37 GB。
资源不足是否导致第二次捕获失败，现有证据不能确定。
闲置缓存已清理，两份自有快照镜像均已完整保存后退出 Engine。
原始数据库、密钥、日志、唯一数据和现用产物均保留。
2026-10-22（UTC）的独立复核通过原资源门禁。
VM 剩余配额为 6,986,211,328 字节，宿主可用空间为 65,641,197,568 字节。
后续复验将只读记录配额与内存，不改变产品限额或时限。
后续复验的 P23 组仍为 2 项通过、1 项失败。
两次整容器快照和三份目录备份均已就绪。
第一次目录恢复、重放和停止的断言均通过。
第二次目录恢复返回 HTTP 500，新租约记录为 `lost`。
失败窗口内没有第二次 Engine 容器创建记录，具体拒绝原因尚未确定。
本批配额采样最低余量为 657,043,456 字节。
原资源后置检查通过；批后 VM 余量为 4,192,133,120 字节。
宿主可用空间为 54,493,622,272 字节。
本批保留全部原始失败报告、数据与剖析记录。

为定位拒绝阶段，生产代码仅新增两处固定阶段日志。
日志仅记录阶段、稳定错误码和编译期运维消息。
登记、目录准备、租约隔离和对外错误保持原有语义。
新日志通过六组静态检查与 Linux ARM64 本地构建。
新 Broker 的 v4 资格检查通过，旧进程已按身份退出。
这些记录尚不证明当前生产源码通过真实容器复验。
完整 coverage、最终 workspace Gate、三宿主发行包仍待通过。
后续前序验收段落保留前一批次的原始结果。

本批次证据目录为 `.temp/p23-full64/workerd-release-r22/local-filesystem-fixture-repair/helper-command-mapping-repair/coverage-current-preparation/current-api-production-build/compile-run-phase-handoff-repair/`。
`canonical-phase-controlled-after-digest-rebuild/root-terminal-failed-batch-review.json` 保存完整批次结果。
`current-failed-target-readonly/native-fixture-candidate-v2/root-apply-result.json` 保存夹具修复记录。
`scoped-fixture-revalidation/root-scoped-compilation-runtime-handoff-live-review.json` 保存内存交接记录。

## 2026-10-22 前序验收

R22 输入下的普通真实容器用例通过，耗时 497.82 秒。
该用例为 1 项通过、零失败、零忽略，另两项未选中。
固定 65 个成员均已绑定固定 types 的编译夹具和该真实容器用例。
目录恢复覆盖两种调度策略、三种挂载形式、文件元数据和重启恢复。
省略快照参数与恢复后的空 `image` 继续采用已接受的官方方向。
本地证据不声明逐项复刻托管观察，也不证明发行包通过。

同日，完整 coverage 批次实际执行 63 个目标。
原批次结果为 24 项通过、39 项失败，未生成合格覆盖率报告。
运行器将临时目录放在共享 FUSE 挂载，导致本地存储和数据库检查失败。
Provider 另在前次编译 OOM 的同一时刻记录 headers 阶段超时。
Provider 进程仍存活，但合同接口返回 `503`；底层资源机制尚未确定。
临时目录修复、Provider 重新资格检查和失败目标复验不改写原失败报告。
完整 coverage、最终 workspace Gate、三宿主发行包仍待通过。

成员核对证据：`.temp/p23-full64/workerd-release-r22/local-filesystem-fixture-repair/helper-command-mapping-repair/coverage-current-preparation/compatibility-current65-readonly/current65-assertion-review.json`。
产品证据：同一 helper 目录下的 `ordinary-directory-replay-current/root-terminal-product-review.json`。
完整失败批次：`coverage-current-preparation/canonical-execution-after-service-lib-repair/root-terminal-failed-batch-review.json`。

## 基础范围与后续批次记录

2026-10-10，用户将本轮验收范围限定为基础 13 family。
snapshot、原生 inactivity timeout、主进程日志采集与其他后续能力由 [P25](../p25-platform-follow-ups.md) 跟进。
当前维护合同见 [Containers 运维](../references/containers.md)，公开状态见 [兼容矩阵](../references/cloudflare-compatibility.md)。

2026-10-12，后续目标扩展为完整 stable API 与 snapshot capture/restore。
固定 types 更新后的清单包含 65 个成员；成员资格仍为 `blocked`。
后续实现尚未通过全部成员资格；当前 coverage 和最终 Gate 结果见后续扩展状态。
当前快照流程、托管对比和证据冲突见 [快照持久化与回收](../references/containers.md#快照持久化与回收验收中)。
本页下列基础验收记录不证明后续源码已通过验收。
第 4.1 节的表格保留 R9 历史范围，不代表当前 65 个成员的逐项结果。

**2026-10-21（UTC）范围确认：保留完整 65 项，补齐目录恢复能力。**
用户确认按固定 API 合同和本地真实行为验收，不必逐项复刻托管实测。
托管差异保留为诊断记录，不再等待托管服务复现相同结果。
用户另接受[两项快照兼容性差异](../references/containers.md#已接受的快照兼容性差异)。
`snapshotContainer()` 允许省略参数；快照恢复后的 `inspect().image` 返回空字符串。
前者遵循已合并的上游改动，后者遵循官方明确写出的恢复语义。
文档与托管观察不一致或没有明确对照，不再阻止这两项的验收。
这项决定不提升其他成员资格，也不替代本地功能、coverage、最终 Gate 或发行包检查。
`snapshotDirectory()` 继续属于实验接口；九个稳定目录成员仍需真实恢复证据。
本批追加 V18 迁移，保存不可变的目录路径，保留现有快照权限和配额检查。
目录备份与整容器备份分开校验；目录备份不能用作启动镜像。
本地通过 1 项路径测试、19 项存储测试、14 项快照服务测试和 11 项 Broker 测试。
目录捕获复用私有进程权限、归档校验和持久化流程；整容器捕获回归也通过。
后续实现将目录恢复清单与执行预留写入同一事务，并冻结备份 ID 和目标路径。
启动重试不能追加、删除或改换清单；daemon 重启后保留原清单。
已有执行可继续读取其保留的过期备份；新的恢复请求仍拒绝过期备份。
应用删除期间，artifact 引用检查保留存活执行使用的目录备份。
只有执行终止、资源回收和后续操作隔离均获确认后，平台才释放引用。
当前实现通过 55 项存储回归和 14 项快照服务回归，零失败、零忽略。
目录捕获的服务回归覆盖两种调度策略；整容器快照仍限定 `durable_object` 策略。
这些检查使用 macOS 本地 Unix fixture，不证明真实 Docker 目录恢复或完整 65 项资格。
本批证据位于 `.temp/p23-full64/package-r21-main-8ee3-current-preparation/directory-restore-api-contract-implementation/restore-authority/`。
2026-10-21（UTC），真实 Linux ARM64 Engine 进一步通过目录 helper fixture。
恢复文件的内容、权限、硬链接和相对符号链接均保留；整容器 export 排除目录挂载。
目录与根文件系统共用 32 MiB 限额，截断根文件可释放目录写入容量。
未过滤的目录所有者可改写 project 属性；新增内核过滤后，同一改写返回 `EPERM`。
目录 helper 不增加 capability；主进程与 exec 入口安装同步到全部线程的过滤。
受影响的 46 项 exec 回归通过；新增过滤专项和真实 root 用户权限 fixture 也通过。
exec helper 的 Go statement coverage 为 90.2%；该数值不代表 workspace coverage。
ARM64 helper 已在实际 Engine 执行；AMD64 helper 仅通过交叉构建。
后续归档与暂存回归通过：25 项 artifact、62 项 Broker 和 1 项匿名文件单元检查。
Broker 的 62 项由原批次 61 项和更新负例后的 1 项组成；失败原报告保留。
快照服务另通过 14 项回归；P23 的 helper 归档排除单元检查通过，两个真实产品用例未运行。
Broker 冻结恢复清单，校验每份 tar 的摘要与大小，并为每个执行保留一个匿名暂存文件。
暂存追加前写入持久 marker；取消或失败继续隔离该执行，不能签发新的启动权限。

真实探针复现了旧 helper 路径的父目录替换风险。
helper 已改为根目录只读挂载，三个入口均检查目录和文件所在的只读文件系统。
真实容器通过父目录替换拒绝、配额防绕过和恢复文件检查。
更新后的 exec helper Go statement coverage 为 90.3%；该数值仍不代表 workspace coverage。
Broker 私有协议升为 v4；旧合同拒绝检查和消费者 schema 的 12 项单元检查通过。
新 helper 的 ARM64 程序已实际运行，AMD64 程序仅通过交叉构建。
旧 Provider 尚未重新构建或资格检查，不能作为新源码的运行证明。
该 helper 批次不提升九个目录成员资格；当时公开启动流程尚未接入。

**2026-10-21（UTC）目录恢复公开流程已接入，真实产品验收仍待完成。**
公开启动参数映射为冻结的备份 ID 和目录路径。
存储事务拒绝追加、删除或改换已有恢复清单。
daemon 按当前权限读取备份，校验完整 artifact，再传输原始目录归档。
Broker 独立校验目录归档；完成恢复后，启动重试不再写入目录。
清理流程只读取原租约持有的备份元数据，不授予新的启动权限。
本地 Unix fixture 的 69 项 Broker 回归通过。
存储批次为 54 项通过、1 项失败；服务快照批次为 14 项通过、1 项失败。
两项失败分别来自测试数据的内容 ID 冲突和遗漏 DO 启动镜像。
修正测试数据后，3 项存储目录测试和新增服务恢复用例通过。
原失败报告保留；这些分批结果不改写原批次状态。

真实 Docker 29.4.0 探针复现了归档跨目录写入。
目录 1 的软链接指向 `../10`；随后一个归档成员写入了目录 10。
Engine 返回 HTTP 200；探针确认目录 10 新增了该文件。
两个临时容器和 11 个卷均已确认删除；原 Provider 和 Engine 状态未变。
目录归档校验现拒绝文件或链接节点下面的成员路径与硬链接目标。
校验保留独立的绝对、相对软链接和普通硬链接，不在宿主机解压文件。
daemon 与 Broker 均在目录归档交付 Engine 前执行这项校验。
修复后的 macOS 本地回归为 26 项归档、70 项 Broker、2 项目录服务测试通过。
这些检查均为零失败、零忽略；正式 R21 pin 未变。
证据位于 `.temp/p23-full64/package-r21-main-8ee3-current-preparation/directory-filesystem-implementation/archive-confinement-engine-qualification/`。
新版 Broker 的真实容器恢复、完整 65 项、coverage、最终 Gate 和发行包仍待验收。

2026-10-21（UTC），新版 Broker 完成 Linux ARM64 本地构建和 v4 资格检查。
资源限制、整容器快照、helper 和三次健康检查均通过。
两个资格探针均有删除事件和 HTTP 404 证明。
两份启动镜像的大小均与实际 Engine 一致。
编译结束后，环境恢复为 4 GiB、2 CPU，原始配置字节保持一致。
旧 Provider 已按原进程身份精确退休。

真实 Broker 拒绝了四种跨目录写入归档，均返回 HTTP 503。
每个负例均确认 Engine 库存不变，执行清理完成。
合法恢复随后在主容器创建阶段失败，原失败报告保留。
Docker 29.4.0 的挂载回显省略 `ReadOnly: false`。
旧校验逐字比较请求和回显，因此拒绝了合法的可写目录。
[Moby 类型定义](https://github.com/moby/moby/blob/master/api/types/mount/mount.go) 也声明该字段省略零值。
该上游链接解释编码规则，不证明已安装 Engine 的源码 revision。
两个挂载生产者现使用同一回显格式；helper 也明确生成空日志配置。
Broker 仍逐项校验卷身份、目录位置和实际读写属性。
两项新增回归先在旧源码失败，修复后的 72 项 Broker 回归全部通过。
这些本地回归均为零失败、零忽略；正式 R21 pin 未变。
本次专属主容器、代理容器和三个目录卷已确认删除。
随后，实际 Engine 拒绝了 Docker 数据目录内的 `rprivate` bind 挂载。
返回错误的摘要和长度与 [Moby 校验规则](https://github.com/moby/moby/blob/master/daemon/volumes_linux.go) 完全匹配。
helper 改用只读 `rslave` 挂载；宿主机仍不接收 helper 发出的挂载传播。
新增回归拒绝 `rprivate`、`rshared` 和可写根目录；73 项 Broker 回归全部通过。
真实 Engine 已接受新挂载，但旧 helper 因嵌套挂载的空目录骨架而退出。
新版 helper 仅接纳纯目录骨架，为每层目录设置根文件系统的 project 配额。
文件、软链接、非零旧 project、外部文件系统和非 root 目录均继续拒绝。
每个卷最多遍历十万个目录节点，遍历不累积打开的文件描述符。
两个目录单元检查已在 Linux ARM64 实际通过，Go vet 也通过。
新版 ARM64 helper 在真实只读容器中正常退出，未增加 capability。
AMD64 helper 仅通过交叉构建；完整 Broker 恢复流程尚未通过。
修复后的真实目录恢复、完整 65 项和最终发行验收仍待完成。
证据位于 `.temp/p23-full64/package-r21-main-8ee3-current-preparation/directory-filesystem-implementation/broker-v4-native-build/`。

2026-10-22（UTC），嵌套目录批次完成 Linux ARM64 Broker 构建和 v4 资格检查。
空间后置检查失败的原记录保留；缓存回收后的独立只读复核通过。
真实恢复随后在归档写入阶段失败，配额 helper 已正常退出。
Docker 拒绝独立的 `../elsewhere` 符号链接，返回 HTTP 500。
错误响应的长度与摘要均与固定错误模板完全匹配。

隔离写入探针保留了文件内容、UID/GID、权限、时间戳和两种链接。
空目录和嵌套挂载也通过检查；归档接收容器从未启动。
执行代码的配额 helper 仍使用只读根目录，未增加 capability。
所有临时容器和卷均已确认删除，其他 Engine 库存不变。

正式接入复用同一个 helper 名称，分开校验配额与归档用途。
配额 helper 删除后，Broker 创建不能启动的归档接收容器。
归档只写入固定前缀下的自有目录卷，写入后先删除接收容器。
维护回归先复现旧路径的 HTTP 503；接入后的 78 项 Broker 检查通过。
本批检查在 macOS 本地 fixture 执行，零失败、零忽略。
检查包含归档边界、长路径、PAX 元数据、禁止启动、失败回收和重启恢复。
新源码仍需完成真实 Broker 恢复、完整 65 项、coverage、最终 Gate 和发行包验收。
接入证据位于上述目录的 `mount-echo-rebuild/current-helper-integration/archive-view-production-repair/`。

最终测试语法修正后，本地 78 项 Broker 和 44 项容器所属检查通过。
检查均为零失败、零忽略。
测试与 fixture 两组 Clippy 及 MSRV 全目标检查也通过。
生产源码未变的三组 Clippy 和无默认特性检查复用前一批记录。
本轮证据位于同一目录的 `current-owning-v5/`、`static-v3/` 和 `failed-static-test-groups-v4/`。

**2026-10-22（UTC）目录恢复已通过 Broker 实测，公开 API 验收尚未完成。**
真实 Linux ARM64 Broker 通过六项目录恢复检查。
检查覆盖文件元数据、硬链接、符号链接、空目录、嵌套挂载和重复恢复。
Broker 重启后保留恢复清单；自有资源回收获得 HTTP 404 证明。
原后置检查失败报告保留，后续独立只读复核通过。
这组实测只证明当时的 Broker 输入，不证明当前完整 65 项资格。

workerd 已通过现有弱引用回调传递目录参数，私有 Container 协议更新为 v4。
本地四个原生目标分两批通过 108 项检查。
直接受影响的 Root 用例通过 1 项检查，包含真实原生目录捕获。
未变化的另外三个原生目标和其余十项 Root 用例未在本地重复执行。
原生源码已提交为 `65267e100a15718300cd17f9d82c2142158ce44a`。
[R22 四平台供应构建](https://github.com/elliothux/workerd/actions/runs/39721082079)仅启动一次。
四个平台各通过 7 个原生目标和 11 项 Root 回归，publish 也成功。
10 项不可变资产的签名、摘要、协议 v4 和 catalog 校验通过。
正式 lock 与 gitlink 已更新为 R22；旧 R21 结果保留原始适用范围。

公开验收已增加两种调度策略、三种恢复参数、错误类型和跨应用拒绝检查。
目录捕获只在实验开关开启时执行。
关闭实验开关后，两种策略均应恢复真实目录备份。
同一流程检查恢复后的文件元数据、链接、重复恢复和 daemon 重启。
候选 fixture 的两组严格类型检查和官方 `cf build` 均通过。
维护 fixture 的 12 项响应与委托测试通过，零失败、零忽略。
本批尚未编译新的 Rust 产品用例，也未执行真实公开目录恢复流程。
完整 65 项、当前 coverage、最终 Gate 和三主机发行包仍待验收。
本批证据位于上述目录的 `native-directory-capture-preparation/public-directory-product-candidate/`。

**后续扩展状态（2026-10-20 UTC）：exec 用户策略修复已通过当前 coverage 和 Linux ARM64 最终 Gate；发行资格仍待完成。**
正式 pin 为 R21，revision 为 `204a75bcfcc35fdf15756bef03eea867711847a6`。
[R21 四平台 CI](https://github.com/elliothux/workerd/actions/runs/39042535973) 的五个 job 均成功。
十个 immutable Release asset 已校验；正式 lock 已更新。

2026-10-19（UTC），exec 用户策略的集成修复完成 Linux ARM64 全轮验证。
被测源码摘要为 `639499080ce14ed7a2a67a18fbcebf1941e788735229d3ea2061db329c9a6076`。
原始 coverage 和最终 Gate 报告的 revision 字段均为 `cd78385a069f3a877c65adfd84ec8c783a336852`。
报告按 runner 算法记录的源码摘要为 `cccb3af22051e1b3af18eb47edec5c39336c655080e4e4870473926fb0494917`。
生产与 Rust 输入快照、原始报告和候选输入复核分别保留；输入等价复核不替代正式 tag 资格。
原版 coverage 通过 63 个目标、2,053 项用例，环境为 4 GiB、2 CPU。
行覆盖率为 90.761046%（168,202／185,324），使用 68 个 object 和 138 个新 profile。
随后，原版未插桩最终 Gate 通过相同的 63 个目标和 2,053 项用例。
最终 Gate 耗时 5,605.09 秒；P23 两项真实产品用例通过，耗时 426.21 秒。
原始父进程记录的 UTC 完成时间为 `2026-10-19T18:15:32.152099+00:00`。
Gate 目录名使用宿主本地时间 UTC+8，不代表 UTC 执行日期。

额外文件守卫错误地要求可重建的测试程序保持文件身份不变；原失败报告保留。
Gate 构建步骤记录了实际执行程序的摘要；两个程序与预编译版本的字节不同。
独立复核确认 69 个实际构建程序的当前字节，以及全部目标的构建摘要。
两处源码的 3,973 个持有文件均通过整轮文件身份和摘要检查。
八项依赖输入、六项依赖观察、Provider、Engine、库存和原始资源配置检查均通过。
快照镜像已按本轮标记精确回收；两个旧安装失败容器保持停止状态。
原始报告为 `.temp/gate-run/20261020T004202-a2c31c90/report.json`。
独立复核记录为 `.temp/p23-full64/r21-exec-user-policy-current-investigation/policy-integration/production-current-fixture-repair/formal-current/orb-management-disconnect-normal-final-current-v2/current-final-independent-terminal-review.json`。

当前兼容合同仍为 `no_go`，不声明全部 65 个成员通过。
本地兼容清单的类型来源已更正为固定的 `5.20260930.2`；65 个成员的状态未提升。
九个目录成员仍缺少稳定生产入口和正向恢复证据。
已保留的托管观测中，仅含 `mountPoint` 的启动失败。
该观测的 `monitor` 拒绝，`inspect()` 返回 `null`。
这组负向子集不证明目录快照生产或正向恢复。
2026-09-01（UTC），上游 [PR #7123](https://github.com/cloudflare/workerd/pull/7123) 将 `snapshotDirectory()` 放回实验开关。
固定稳定类型仍保留九个目录快照和恢复成员。
2026-09-21（UTC），上游 [PR #7453](https://github.com/cloudflare/workerd/pull/7453) 将 `snapshotContainer()` 的选项改为可省略。
两项改动均已进入固定 R21 的上游基线；可省略选项也与已保留的托管实测一致。
当前官方文档仍声明省略选项应抛出 `TypeError`。
恢复后的 `inspect().image` 也存在文档与托管实测差异；两项历史差异按页首接受决定处理。
2026-10-20（UTC），Linux ARM64 和 macOS ARM64 发行包均已构建。
两个包均绑定 revision `8ee3babe5319d3001b3663c58475af970f16ddce`，并包含本次用户策略修复。
Linux ARM64 的原四项 package 用例和两项 Dashboard 用例均通过。
同一包在 Ubuntu 24.04、Debian 13 的 user、system 四个安装范围均通过。
macOS ARM64 的原四项 package 用例已在普通账户下通过，耗时 274.87 秒。
本批为四项通过、零项失败、零项 ignored。
临时账号、home 和 staging 已回收；17 个对象的 ACL 已恢复。
源码、原程序、Native 输入和原始 Orb 配置检查均通过。
Linux x64 候选包也已构建，并通过原四项 package 用例。
本批为四项通过、零项失败、零项 ignored，实际测试耗时 130.53 秒。
同一 x64 包在 Ubuntu 24.04、Debian 13 的 user、system 四个安装范围均通过。
不兼容的 0.3.0 升级按声明拒绝；原程序、receipt、密钥、配置和数据检查均通过。
三个原生候选包均绑定上述 revision，尚不证明正式 tag 工作流资格。
[x64 验收](https://github.com/elliothux/open-compute/actions/runs/39391746780)已在 2026-10-20（UTC）成功结束。
五个完整压力档均通过；实际限制为 2 CPU、4 GiB。
29 组性能测量共取得 1,113,500 个样本，记录 723 次请求错误；各组均满足原 SLO 阈值。
本批没有降低负载、缩短观察或放宽阈值。
恢复观察从 11:13:22 持续至 12:13:54（UTC），实际经过 3,632 秒。
三次注入重启均通过 readiness 和后续状态一致性复核。
报告保留候选 revision `8ee3babe5319d3001b3663c58475af970f16ddce`、x64 二进制摘要和正式 R21 lock 摘要。
监控自然退出为 0；原监控进程已不存在，没有重启 CI。
压力报告复核为 `.temp/p23-full64/package-r21-main-8ee3-current-preparation/linux-x64-stress-terminal-evidence/root-stress-terminal-review.json`。
[macOS 安装验收](https://github.com/elliothux/open-compute/actions/runs/39398464481)已在干净宿主通过。
该宿主构建的包通过原四项用例，耗时 137.23 秒；user、system 两个安装范围均通过。
该包与前述本地普通账户包的字节不同；两组产物和证据分别保留。
三平台共五个 OS／架构任务、十个安装范围的证据已收齐。
当前被测源码的 Linux ARM64 受控出口三项也已在 2026-10-20（UTC）通过。
原 Gate 共耗时 149.27 秒，其中准备耗时 111.54 秒，测试进程耗时 36.20 秒。
本批使用 4 GiB、2 CPU；八项依赖 FD 和六项观察均保持原身份和摘要。
Provider、Engine、源码、输入和原始 Orb 配置检查均通过。
loopback 地址、hosts 原文与权限、三个监听端口和临时证书目录均已恢复。
这项本地验证绑定前述被测源码，不证明正式 tag 工作流已经执行。
复核记录为 `.temp/p23-full64/package-r21-main-8ee3-current-preparation/current-controlled-egress-preparation/root-current-controlled-egress-terminal-review.json`。
2026-10-20（UTC），macOS ARM64 的原版最终 workspace Gate 完成 63 个目标、2,061 项 discovery。
原批次通过 61 个目标，共 2,054 项用例；两个目标失败，原报告保持 `failed`。
P23 两项真实产品用例均通过，耗时 430.00 秒。
`single-binary` 使用了错误角色的二进制；`p22-browser-run` 缺少官方构建产物。
修正输入角色并完成原有官方夹具构建后，仅复验这两个目标的原七项用例。
七项全部通过，零项失败、零项 ignored；浏览器耗时 278.58 秒，单文件测试耗时 209.85 秒。
两批报告均保留 revision `ff7dc6836829c177a2c2185082d2285fa4447af5`。
两批的 Gate 源码摘要均为 `ce8ee9e6690c573d105b221205d748462c4c6c929d0a4fe71925f61f7fded9f7`。
独立复核按原 discovery 去重，确认 63 个目标、2,061 项用例均有通过证据。
原完整批次没有重跑；分批复核不把原报告改为 `passed`。
源码、持有输入、Native 守卫、原始配置、库存和临时目录回收检查均通过。
本轮快照镜像按原归属标记精确回收；旧容器保持原状态。
原全量结束后，磁盘余量低于当批保守预留线；预算未通过的记录保留。
两目标复验满足独立的固定空间门槛，门槛高于文档要求的 44 GiB。
复核记录为 `.temp/p23-full64/package-r21-main-8ee3-current-preparation/local-macos-two-preparation-repairs-v3/root-terminal-and-full-diagnostics-review.json`。
本地分批验证不替代正式 tag 工作流资格；正式发行报告仍待完成。
前一 CI 的 S3 测试失败；当时测试 bucket 不存在，返回 HTTP 404。
前一 CI 另有一项旧平台矩阵文本断言失败。
本地修复保留正式发行的三个平台；100 项夹具工具测试均通过。
2026-10-21（UTC），私有测试 bucket 已恢复为 Standard，公开访问关闭。
其他 bucket 和凭据未变；本批没有恢复历史对象。
通过本地真实用例后，修复已一次性快进推送 main。
main revision 为 `b3b78bdd05337a6399d9d826ed804fa960a96909`。
[本次 main CI](https://github.com/elliothux/open-compute/actions/runs/39534275671)已通过。
九个 job 成功，三个条件 job 按路径跳过。
本轮 CI 不替代 coverage、完整 workspace Gate 或正式 tag 资格。
当前包的证据清单为 `.temp/p23-full64/package-r21-main-8ee3-current-preparation/current-release-evidence-inventory.json`。
这些包检查不替代全部 65 个成员的兼容资格，也不证明发行流程已经通过。

2026-10-20（UTC），本地候选补充零参数 `snapshotContainer()` 的真实成功回归。
macOS ARM64 使用正式 R21 和独立的 4 GiB、2 CPU Engine，原 ordinary 用例通过，耗时 477.26 秒。
本批发现三项用例，仅执行 ordinary；结果为一项通过、零项失败、零项 ignored、两项过滤。
新增断言核对无 name 的新 handle、持久化状态、实际大小和跨对象恢复。
原命名快照、S3 字节数与 SHA-256、helper 排除、保活和 daemon 重启恢复断言全部保留并通过。
前一失败来自测试助手要求本地缓存命中；当时两个 snapshot 均为 Ready，零参数快照已恢复。
共享助手现从 S3 读取归档并校验大小与 SHA-256，再用私有临时文件核对完整归档。
原 180 秒读取期限和 1 GiB 缓存预算保持不变。
本批持有文件、Native 输入、Provider、Engine、原始配置和库存检查均通过。
本批自有进程、临时目录和一个快照镜像已回收；前一失败的数据库、日志和镜像导出归档保留。
独立 exec 流与代码更新用例在前一测试候选中另批通过，耗时 96.28 秒。
本批未执行新的完整 workspace Gate，也未提升 65 个成员的资格。
当前单例复核为 `.temp/p23-full64/package-r21-main-8ee3-current-preparation/local-macos-omitted-snapshot-owner-v5/root-terminal-success-review.json`。
助手断言复核为 `.temp/p23-full64/package-r21-main-8ee3-current-preparation/current-snapshot-authoritative-archive-test-repair/root-helper-preservation-and-source-review.json`。

2026-10-21（UTC），五个串行托管动作取得八个稳定成员的有限观测。
HTTP 指定主机规则和兜底规则均返回预期字节，HTTP 与 socket 关闭均已确认。
预先取消的 exec 抛出 `AbortError`；实际 READY 后取消的 exec 以 137 退出。
取消后的 stdout、stderr 均到达 EOF，主容器仍可执行下一条命令。
实际 READY 后调用 `kill(9)` 的 exec 也以 137 退出，两条输出流均到达 EOF。
`lite` 和 custom 两种启动均通过真实 readiness、running、image 和 label 核对。
custom 输入为 `vcpu:1`、`memoryMib:3072` 和 `diskMb:4000`。
本批没有测量实际分配量，也不证明其他数值、边界或完整成员语义。
每次动作均销毁容器后再执行下一次动作；同时运行的容器最多一个。
临时 Worker、Container 应用和命名空间均已删除，三份资源清单与执行前相同。
临时 secret 仅经内存管道传入；本批没有创建 secret 文件或调用快照 API。
托管记录现为 56 项正向子集、3 项负向子集、6 项未实测。
九个目录成员仍缺稳定生产入口和正向恢复证据；全部 65 个成员的资格未提升。
复核记录为 `.temp/p23-full64/package-r21-main-8ee3-current-preparation/authorized-external-execution-20261021/hosted-root-terminal-review.json`。

2026-10-21（UTC），资源输入对照使用五次串行观测。
前后两次 `1` vCPU、`3072` MiB、`4000` MB 均通过 readiness 和 inspection。
中间三次输入分别为本机组合、仅降低 CPU、仅降低内存。
本机组合为 `0.03125` vCPU、`128` MiB、`1000` MB。
仅降低 CPU 的组合保留 `3072` MiB 和 `4000` MB。
仅降低内存的组合保留 `1` vCPU 和 `4000` MB。
三次请求均在 readiness 阶段返回受控 `Error`；容器销毁均成功。
记录未捕获具体校验谓词，也未测量实际分配量。
本机组合的本地成功与本次托管错误构成有限行为差异。
本批不确定完整数值边界，也不提升成员资格。
临时 Worker、应用和命名空间均已回收，执行前后的资源清单一致。
复核记录为 `.temp/p23-full64/package-r21-main-8ee3-current-preparation/current-custom-resource-differential-preparation/root-terminal-review.json`。

以下段落保留各批次的历史输入和结果。

2026-10-19（UTC），Ubuntu 24.04 ARM64 和 Debian 13 ARM64 的 user、system 安装范围均通过。
四个范围保留原安装、服务生命周期和升级拒绝断言；0.3.0 的不兼容 Worker 仍被拒绝。
原程序、receipt、密钥、配置和数据保持原值；本批容器均已回收。
这些结果绑定候选 revision `fbe605015d484500d913b599a2d53a20226c250d`，不证明其他 revision 的发行包身份。
当前 Linux 受控出口的三项原用例也已通过。
构建准备改写了编译产物的元数据，外层守卫失败记录保留；独立只读复核确认用例结果和产物归属。

同日，独立核查发现 exec 用户语义仍有缺陷。
[官方 Container API](https://developers.cloudflare.com/containers/api/durable-object-container/)规定：裸 UID 使用 root，用户或组名称应使 exec 拒绝。
当前 native 和 Broker 原样转发 `user`；真实 Engine 将裸 `65534` 按 UID 65534 执行，并接受 `root` 和 `nobody:nogroup`。
五项 Engine 探针均取得结果；探针容器已回收，八个持有输入、六项观察和 73 个程序均保持原字节。
本批只验证 Engine 行为和转发源码，不替代公开 Container API 复验。
Broker 注册也未携带调度策略，尚不能区分两种策略的用户能力合同。
官方文档要求 `durable_object` 策略中的非 root 用户保留 root 能力；默认策略中的非 root 用户不保留能力。
裸 UID 和名称拒绝规则已在 Broker 修复。
原实现的新增回归先失败；修复后六项 exec 单元用例全部通过，零项 ignored。
本批发现 976 项单元用例，只执行 exec 模块六项；环境为 Linux ARM64、4 GiB、2 CPU。
本批源码摘要为 `2a5ca50cb20ab721397eec476cf1ef9bffdc427bd357486891c95ca7b65c8830`。
构建后内存已恢复为 4 GiB，原配置字节保持不变。
临时凭据实现的四项单元用例已通过，零项跳过；测试使用 Linux ARM64、4 GiB、2 CPU。
隔离 Engine 探针确认非 root 主进程和 exec 子进程可保留原能力上限；默认非 root 对照没有有效能力。
探针未启用特权模式，也未增加能力；两只容器均已回收。
持久化调度策略已接入 Broker 注册源码；私有 schema 更新为 3。
当前 helper 的 42 项顶层测试、5 项子测试和隔离凭据验证均已通过；语句覆盖率为 90.2%。
隔离验证使用 scratch 镜像，无基础镜像下载和宿主 root；本批镜像与容器均已回收。
默认策略和 Durable Object 策略的公开回归已加入原普通产品用例；夹具 strict 与官方 cf build 已通过。
该初始批次尚未完成新 Rust 编译、Provider 资格、真实产品、coverage、最终 Gate 和发行包资格。
该批次使用旧 Provider；当时的公开 API、完整 65 成员和发行资格仍未通过。
源码差异证据为 `.temp/p23-full64/r21-exec-user-policy-current-investigation/root-terminal-review.json`。
单元结果为同一目录下 `numeric-user-candidate/green2-test-actual/result.json`。

同日，Ubuntu 24 ARM64 的真实 user 安装／升级夹具发现升级缺陷。
候选包首装和服务启停通过；0.3.0 升级命令成功退出，但旧 Worker 请求返回 HTTP 500。
升级后的二进制、receipt、PID、身份密钥和 KV 读取断言均已通过；完整夹具仍为失败。
两次失败容器均已停止，原数据和报告保留。
0.4.0 描述符新增 Container 声明，但旧源码仍使用 loader schema 1，升级预检只检查数据库迁移。
本地修复将当前 loader schema 改为 2，并在预检中拒绝不兼容的可执行版本，不改写旧数据。
该单元批次的源码摘要为 `894f64b84a53c531c33a9de2345aa723e5134379bc503de8962b4b628bfb0ea5`。
storage 单元验证已通过：417 项通过、零项失败、零项 ignored，测试环境为 4 GiB、2 CPU。
本批使用未插桩程序，但输出目录沿用既有 coverage target；本批只证明有限的单元行为。
原正常构建输出和八项受保护输入保持原字节。
该单元批次没有完成安装、产品、coverage、最终 Gate 和发行包资格。
下述 F806 结果保留原始范围，不证明修复后的 Rust 源码已通过验收。
证据为 `.temp/p23-full64/install-r21-qualification-preparation/root-upgrade-worker-500-terminal-review.json`。

2026-10-18（UTC），升级预检修复后的 Rust 输入完成静态检查和产品复验。
被测源码摘要为 `e44524acbad26befa9046bee89370e4a42e6d56610fe62db4eb3a537102c85f3`。
对应的 2,036 项 Rust 编译输入摘要为 `a8f902ba79ecd49c0f96826c941d446b26137c26396a19c1bd25968ea005990e`。
原版完整 coverage 执行了 63 个目标、2,050 项用例；四个目标失败。
本批 P23 两项真实产品用例均通过，耗时 539.38 秒。
原批次的八个持有文件、六项依赖观察和运行环境检查均通过。
失败后的逐项复验证明 alarm 原单例通过；129 个对象和 45 秒断言均未改变。
alarm 指标采样没有取得可用计数，首次失败原因仍未确定。

浏览器复验现使用同版本的官方 Linux ARM64 Headless Shell。
归档的 SHA-256 为 `d433c45172c7836e38124fe545f767b02210bfb43a6262f08a297473a8e91c99`。
原版 preparation 工具成功退出；原 x64 输入和全部失败证据保留。
ARM64 批次的 51 项浏览器用例中，50 项通过，一项 DevTools 初始化失败。
随后，仅失败单项启用调用栈诊断，原断言和时限不变；单项通过，耗时 116.50 秒。
首次 DevTools 初始化失败的唯一原因仍未确定。
普通用户文件守卫、Provider、Engine、4 GiB 和原始配置字节检查均通过。
浏览器孤儿恢复的可执行文件摘要校验保持原规则，并已通过原生 ARM64 复验。

保留输入的初步行覆盖率为 90.7581%（168,104／185,222）。
该计算尚未包含后续 ARM64 批次的 profile，不声明当前完整 coverage 资格通过。
LLVM 的 371 个函数 `mismatched data` 警告仍保留；原因尚未确定。
维护工具现补充 Linux ARM64 归档 pin、CI 宿主选择和四平台校验回归。
当前维护源码摘要为 `87bd0389d3ca2e61419850a03d77cda1801402fd30095ac1baa88204b0e8e7ef`。
工具与文档变更不修改 2,036 项 Rust 编译输入；已完成的 Rust 构建未重复执行。
OS 安装夹具的 Dockerfile 还屏蔽 `systemd-binfmt` 单元，避免改写宿主解释器注册。
新夹具镜像仍待安装资格验证；过去注册丢失的具体时刻未捕获。
当前源码的 macOS 最终 Gate、安装／升级矩阵、三宿主发行包和完整 65 成员资格仍待完成。
原失败批次没有改写；以下 F806 完整验收只证明历史源码。

同日，失败项复验后的 2,050 项用例并集均通过。
原 coverage 批次仍保留失败状态，没有改写为通过。
正式汇总使用 68 个 object 和 156 个 profile，行覆盖率为 90.754338%（168,097／185,222）。
该数值超过 90% 门槛；原 90.7581% 初步汇总保留为历史记录。
LLVM 的 371 个告警符号均为零 hash，来自依赖和标准库，没有项目自有符号。
原告警未压制；具体成因仍未确定。

随后，同一源码的 Linux ARM64 最终未插桩 workspace Gate 通过。
原命令为 `./test/gate.py --workspace --final --keep-going --jobs 1`。
环境为 4 GiB、2 CPU 配额；63 个目标和 2,050 项用例全部通过。
完整 Gate 耗时 5,319.93 秒；P23 两项产品用例均通过，耗时 438.40 秒。
源码、输入、进程、资源、依赖观察和快照镜像回收检查全部通过。
原始 Orb 配置字节一致，两个旧安装失败容器保持停止状态。
机器可读兼容合同 verdict 仍为 `no_go`，不证明全部 65 个成员资格通过。
证据为 `.temp/p23-full64/r21-loader-schema-local-build-preparation/test-fixture-type-repair/current-native-arm-final/root-current-final-terminal-review.json`。

证据为 `.temp/p23-full64/r21-loader-schema-local-build-preparation/test-fixture-type-repair/` 内各批次的原始结果。

2026-10-18（UTC），当前被测源码摘要为 `f8063676e783128e57baedb31b94de6bc61b7c8f31e1e7796c0c3faf9ee03521`。
Mac arm64 未插桩普通容器单例通过，耗时 463.99 秒。
结果为一项通过、零项失败、零项 ignored、一项 filtered。
本次复验只更换夹具传输，平台源码、正式 pin、原断言和时限不变。
夹具使用经二进制半关闭校准的官方 Docker stdio 连接。
原 SSH 连接丢失延迟响应的失败记录保留。
131 条连接、独立 Broker、测试镜像和 helper 卷均已回收。
证据为 `.temp/p23-full64/r21-layer-transport-local-build-preparation/darwin-p23-stdio-current-local/root-product-terminal-review.json`。

同日，当前源码的完整 workspace coverage 通过。
原命令为 `./test/coverage.sh --keep-going --jobs 1`，环境为 4 GiB、2 CPU。
本轮 63 个目标全部通过，共 2,049 项用例；P23 的两项真实产品用例均通过。
Rust 行覆盖率为 90.7401%（168,058／185,208），超过 90% 门槛。
输入、五个持有文件、六项依赖观察、Provider、Engine 和清理后置检查均通过。
LLVM 报告保留 371 个函数的 `mismatched data` 警告；警告原因尚未定位。
本轮原始 68 个 object、140 个 profile 和报告均保留。
警告未压制，coverage 排除规则、原断言和时限均未改变。
证据为 `.temp/p23-full64/r21-layer-transport-local-build-preparation/canonical-coverage-candidate/root-canonical-coverage-actual-review.json`。

当前 69 个未插桩 workspace 程序已完成 `--no-run` 编译。
编译结束后，环境恢复为 4 GiB、2 CPU；原始配置字节一致。
该编译不替代最终 workspace Gate。
随后，同一冻结源码的最终未插桩 workspace Gate 通过。
原命令为 `./test/gate.py --workspace --final --keep-going --jobs 1`，环境为 4 GiB、2 CPU 配额。
本轮 63 个目标全部通过，共 2,049 项用例；P23 的两项真实产品用例均通过。
完整 Gate 耗时 5,388.42 秒；五个持有文件、六项依赖观察和 69 个正常程序均保持原字节。
输入、源码闭包、Provider、Engine、资源和精确清理后置检查均通过。
证据为 `.temp/p23-full64/r21-layer-transport-local-build-preparation/final-candidate/root-canonical-final-actual-review.json`。
报告的全局兼容合同 verdict 仍为 `no_go`。
三平台包资格和全部 65 个成员资格仍待完成。
9 个目录成员的资格和两项 snapshot 来源冲突仍未解决。

2026-10-18（UTC），同一源码的 Darwin arm64 安装包原四项用例全部通过。
两组 ACL 已恢复；临时账户、Home、staging 和账户进程均已回收。
Linux arm64 安装包原四项用例也全部通过，结果为四项通过、零项失败、零项 ignored。
随后，同一 Linux 包的两项 Dashboard 浏览器用例通过。
Linux 两批验证均使用 4 GiB、2 CPU；源码、运行时、编译输入、资源和清理后置检查均通过。
Dashboard 创建的用户数据已移入本批证据目录，真实 Home 中没有遗留用户根目录。
Darwin 证据为 `.temp/p23-full64/package-r21-qualification-preparation/darwin-login-candidate/acl-direct-failure-repair/root-darwin-v5-qualification-terminal-review.json`。
Linux 证据为同一准备目录下 `linux-arm-product-cas-recovery-current/root-gate-terminal-review.json` 和 `root-dashboard-terminal-review.json`。

同日，候选 checkout `13f6d3a0242540d2ce2e5a71f8ed3645a1a9a7cc` 的 SDK 打包检查通过。
原版类型检查、十九项测试、构建和 tarball 消费检查均成功退出。
消费检查覆盖 Node ESM、CommonJS、Bun 和 TypeScript；3,060 个已跟踪文件保持原字节。
`@open-compute/sdk@0.4.0` tarball 的 SHA-256 为 `5c8326de698cd0c2f68aa7b960f37ce723618fb72314d3136623d271f9089c86`。
证据为 `.temp/p23-full64/package-r21-qualification-preparation/sdk-current-candidate-actual/result.json`。
本批未发布 npm；真实 x64 宿主、安装／升级矩阵、压力测试和完整发布资格仍待完成。

同日，当前源码的 Linux 受控出口验收通过。
原版 `test/test-p0-2-egress-linux.sh` 执行 P0.2 原目标，环境为 4 GiB、2 CPU。
三项用例全部通过，耗时 37.74 秒；完整 workspace Gate 未重复执行。
测试添加的 hosts 记录、回环地址、证书和密钥均已清理。
69 个正常程序、五个持有文件、六项依赖观察、源码、Provider、Engine 和资源后置检查均通过。
证据为 `.temp/p23-full64/r21-controlled-egress-current/root-terminal-review.json`。
以下段落保留旧输入的历史证据，不替代当前源码的验证。

2026-10-16（UTC），下述冻结源码的完整 workspace coverage 已通过。
原命令为 `./test/coverage.sh --keep-going --jobs 1`，环境为 4 GiB、2 CPU。
本轮 63 个目标全部通过，共 2,049 项用例；P23 的两项真实产品用例均通过。
Rust 行覆盖率为 90.755427%（168,109／185,233），超过 90% 门槛。
被测源码摘要为 `14c07b0f7668f2ed6ee75433ef18e46f390e7dbdad412334bd6d5a7813ea1432`。
完整输入、五个持有文件、六项依赖观察、Provider、Engine 和资源后置检查均通过。
证据为 `.temp/p23-full64/r20-arm-sha2-local-build-preparation/canonical-coverage-candidate/root-canonical-coverage-actual-review.json`。
当前源码启用既有 `sha2` 依赖的 `asm` feature，保留软件后备路径和原产品时限。
该配置的直接插桩普通单例也已通过，耗时 548.28 秒。
随后，冻结源码的最终未插桩 workspace Gate 已通过。
原命令为 `./test/gate.py --workspace --final --keep-going --jobs 1`。
本轮 63 个目标全部通过，共 2,049 项用例；输入、进程、资源和清理检查均通过。
证据为 `.temp/p23-full64/r20-arm-sha2-local-build-preparation/final-candidate/workspace-final-r20-arm-sha2-actual/result.json`。
该报告的全局兼容合同 verdict 仍为 `no_go`，不声明全部 65 个成员通过。

2026-10-17，当前包的 Linux arm64 原四项用例和两项 Dashboard smoke 已通过。
Linux x64 原四项用例在 ARM 宿主的 QEMU 中通过，尚不构成真实 x64 宿主资格。
Darwin arm64 原批次为三项通过、一项失败；失败项触发 95% 磁盘停写保护。
平台保留停写阈值和全部原断言，清理已完成构建的闲置中间文件。
同一发行程序和测试程序的失败项复验通过，耗时 104.80 秒。
复验使用新建的普通账户和真实 Home，结果为一项通过、零项失败、三项 filtered。
原三项通过证据与本次复验证明原四项分别通过，不改写原失败批次。
两批次的权限已恢复，临时账户、Home、staging 和账户进程均已回收。
复验证据为 `.temp/p23-full64/package-r20-qualification-preparation/darwin-login-candidate/failed-do-disk-exact-retry-actual/root-actual-result-review.json`。
全部 65 个成员、真实 x64 宿主资格和发布资格仍待完成。
目录成功输入和两项 snapshot 来源冲突仍未解决。

2026-10-17，Linux 与 Mac 的独立 Broker 生命周期分别通过真实验证。
两次验证均新建 Broker，完成两个资格探针，并确认探针容器删除后返回 404。
维护中的构建入口接受两份镜像输入；Broker 正常退出，进程和临时套接字均已回收。
两次验证复用既有专用 Engine，不证明新 CI runner 能准备全部运行环境。
对应脚本现位于 `test/containers-fixture/`，与实测脚本的字节一致。
CI 的 core 检查已接入脚本自检；原 33 项和新增 helper 准备工具的 12 项自检均通过。
helper 准备工具还通过真实 Engine 验证：新卷的回读摘要一致，验证用容器和新卷均已清理。
本次仅新增夹具工具和 CI 自检入口；上述 coverage、最终 Gate 和包证据仍绑定各自实测输入。
新 CI runner 的 Engine、helper、sidecar 和镜像准备仍待验证。

2026-10-17，合并最新 main 后的源码摘要为 `4c91afd605f81c4b5b9447c78b2ac55f885cf3e7bf07acb162000d5e7c42ad0d`。
该源码的 Linux 静态检查、正常测试程序编译、服务管理回归和真实 P22 浏览器用例均通过。
P22 用例耗时 150.29 秒；原断言和实际浏览器路径保留。
上述检查不更新冻结源码 `14c07b0f` 的 coverage、最终 Gate 和包证据。

同日，当前 Darwin 正常程序和测试程序已编译。
服务管理所属单例通过，结果为一项通过、零项失败、零项 ignored、974 项 filtered。
该单例使用隔离的命令替身，验证 start 与 restart 的参数。
另一个临时 launchd 服务验证真实系统行为：重复 start 保持 PID，restart 更换 PID。
临时服务及其进程已回收，用户现有 ocd 服务保持不变。
两项检查不证明真实 ocd 的安装、升级或发行包资格。
证据为 `.temp/p23-full64/r20-main-merge-local-build-preparation/darwin-current-local/root-owned-launchd-semantics-actual-review.json`。

2026-10-17（UTC），0.4.0 的 Mac arm64 未插桩普通单例在 exec 输出断言失败。
本次使用正式 R20，耗时 83.98 秒，结果为零项通过、一项失败、一项 filtered。
实际子进程退出码为零，stderr 为空，但默认 exec 的 stdout 为空。
显式 shell 对照返回 11 字节 stdout；这些记录尚未确定空输出的唯一原因。
原失败结果和全部输入、进程、资源与清理证明保留。

本地原生回归确认一个独立的输出生命周期缺陷。
退出码先到达后，V8 GC 可以释放 process handle，取消尚未排空的输出流。
回归使用真实 Actor IoContext、V8 GC 和 Cap'n Proto 流，生产进程由延迟输出替身提供。
修复前回归失败；修复后 stdout、stderr 和退出码断言通过。
补充断言确认输出完成后 process handle 释放。
Mac 真实产品中的空输出仍待新正式 pin 复验。

本地 workerd 构建、7 个原生 owning target 和 11 项 Root 回归均通过。
Container API 与 policy channel 目标为 57 项通过，原断言和时限不变。
修复提交为 `204a75bcfcc35fdf15756bef03eea867711847a6`。
[R21 四平台供应构建](https://github.com/elliothux/workerd/actions/runs/39042535973)仅启动一次，五个 job 均成功。
十项 immutable 资产、签名、摘要、协议和 catalog 已校验。
正式 lock 和父仓库 gitlink 已更新，新 pin 的完整产品资格仍待验证。
资产证据为 `.temp/p23-full64/workerd-release-r21/verified-assets.json`。
上述旧 coverage、最终 Gate 和包证明不覆盖这次原生修复。
本地证据位于 `.temp/p23-full64/release-0.4.0-current-preparation/mac-p23-current-local/exec-result-diagnostic/native-output-lifetime-candidate/`。

R12 两个平台与 R14 Linux x64 的同一原生测试用例超时。
两个旧 run 已取消；原始失败日志保留。
R15 只修正测试替身的 callback 连接池；本地 client 目标为 17/17 PASS。
原测试预算和生产源码保持不变；这些结果不替代跨平台 CI。
当前成员资格继续保持 `blocked`，旧 coverage、Final 和 package 记录不证明这些改动。

R15 的真实产品用例执行至原生 inactivity 场景后失败。
exec 生命周期阶段已完成；120 秒对照容器却在约 35 秒时进入停止流程。
平台 alarm repair 触发私有 tenant 重建，导致 Root 的保活策略重置为默认 10 秒。
R16 修复让私有 tenant 复用 Root 的保活策略，不增加 active request 或重置计时。
本地十项原生回归通过。
原生测试时钟推进至 90 秒时，对照容器保留同一 tenant 和运行状态。
[R16 四平台 CI](https://github.com/elliothux/workerd/actions/runs/38327659482) 已失败，未发布 Release。
Linux arm64、Linux x64 和 Darwin arm64 的 CI 回归通过。
Darwin x64 两次执行均有两个目标达到 30 秒时限。
原始日志保留，具体变慢原因仍未确定。
R17 将 Rosetta medium 测试进程时限改为 60 秒，保留全部目标和断言。
四平台均通过四个原生目标和十项 Root 回归，immutable Release 已发布。
十项资产的签名、摘要、架构和协议已校验。
R17 未通过真实产品、coverage、最终 Gate 和三平台包的完整验收。

2026-10-13，R17 的首次真实 P23 Gate 有一项通过、一项失败。
独立 rootfs、helper 排除和 hardlink 回归通过；普通产品用例在 HTTP 拦截阶段失败。
只重跑失败项的诊断确认 HTTPS 和 WebSocket 阶段已完成。
HTTP 注册使用公开 DO Fetcher 时返回 `DO_RPC_UNSUPPORTED`。
该 Fetcher 的 policy 负责权限和顺序，但原生层仍拒绝提取其通道。
修复应保留这条公开 policy 路径，不应绕过权限或替换 fixture binding。
诊断证据为 `.temp/p23-full64/r17-network-diagnostic-preparation/ordinary-network-phase-diagnostic-78e552-plan-loader-actual/`。
这些子阶段结果不证明普通用例、90 秒保活或完整 P23 验收通过。

R18 源码修复公开 DO 和 Service Fetcher 的回调通道。
通道保留公开 policy、原 owner 和真实流的清理路径。
WebSocket 的同一租约监控覆盖外层响应等待和内层传输。
2026-10-13，Darwin arm64 的 16 项新增原生 owning 用例均有成功记录。
这些记录来自同一生产源码的 55 项成功与失败项的精确复验。
原混合批次仍记录失败，不作为一次完整目标通过。
测试夹具改用 JSG 返回值包装函数，交付原生租约。
原预算、真实 EOF、取消和 owner abort 断言保留。

2026-10-13，Linux arm64 的三项 Rust WebSocket 租约所属回归通过。
回归使用当前 Rust 源码和正式 R17 编译输入，执行环境为 4 GiB、2 CPU。
回归验证静默正文保持租约、真实 TCP 断连释放租约和无效认证／句柄的原子拒绝。
测试程序发现 973 项用例；每次精确执行均为一项通过、零项失败、零项 ignored。
输入、进程和测试程序的前后守卫均通过。
证据为 `.temp/p23-full64/rust-websocket-handoff-owning-preparation/owning-actual/result.json`。

该结果只证明 Rust 租约回归，不证明原生 runtime 和完整产品通过。

[R18 四平台 CI](https://github.com/elliothux/workerd/actions/runs/38447450369) 已结束，结果为 failure。
源码 revision 为 `4bdbfda718f57284e40f11e41ec51a5a9aa0cda8`。
工作流验证七个原生目标和十项 Root 回归。
2026-10-13，Linux arm64 的构建通过，但原生资格检查失败。
Container 目标记录 55 项通过、一项失败；另外六个原生目标通过。
失败断言把响应字节数组转换成要求 NUL 终止符的 `StringPtr`。
测试夹具没有添加终止符；修复应直接比较长度和字节内容。
Linux x64 也记录同一断言失败；Container 目标同为 55 项通过、一项失败。
两个 Linux 平台均未执行 Root 回归和资产上传。
两个 Darwin 平台均通过七个原生目标和十项 Root 回归。
工作流跳过 publish；R18 未发布正式资产。
证据为 `.temp/p23-full64/workerd-release-r18/linux-arm64-job-evidence/`。
R18 失败时，正式 binary pin 仍为 R17；R18 没有正式资产。
R18 未执行真实产品、coverage、最终 Gate 和三平台包验收。

R19 只修正上述字节比较断言；生产源码和原预算不变。
2026-10-13，Darwin arm64 的失败 HTTP 用例精确复验通过。
响应长度、完整内容和替换 owner 的拒绝断言保留。
[R19 四平台 CI](https://github.com/elliothux/workerd/actions/runs/38463100288) 已结束，结果为 success。
源码 revision 为 `d9b8d6ce40e0bbd3b9233acad4d1516cbab28e3b`。
四个平台均通过七个原生目标和十项 Root 回归；publish 已成功。
收集程序核对五个 CheckRun 的真实步骤结果，并保留原 REST 记录。
十项 immutable Release 资产的签名、摘要、架构、协议和 catalog 均通过校验。
远端 Release tag 与已获取的 tag 均指向上述 revision。
正式 binary pin 已更新为 R19。
证据为 `.temp/p23-full64/workerd-release-r19/verified-assets.json`。
R19 的真实产品、coverage、最终 Gate 和三平台包验收仍待完成。

2026-10-14，R19 的普通产品用例在启动阶段失败。
系统状态返回 `RUNTIME_INVALID`；workerd 在 ready 前退出，exit code 为 1。
用例未执行 HTTP 拦截和 inactivity 场景。
输入、Provider、Engine、库存和 4 GiB 内存的前后守卫均通过。
原始失败证据保留于 `.temp/p23-full64/r19-owning-runner-preparation/product-runtime-invalid-evidence/`。

普通 host 的模块清单缺少私有 RPC facade 的依赖。
直接补入依赖模块会触及禁止的 INTERNAL 导入。
当前源码将纯 WebSocket 句柄和响应 header 函数移入独立模块。
私有 WeakMap、RPC、租约和权限边界保持不变。
严格类型、资产构建和受影响的所属测试均通过；实际生成模块的闭包检查通过。
这些检查不证明真实启动或普通产品用例通过。

随后，真实原生探针确认私有 AlarmIndex 已被包装为函数 RPC view。
原 guard 只接受对象，因此在 SDK 构造前拒绝该 view。
当前 guard 接受对象或函数，并检查三个必需方法可调用。
三十三项所属回归通过；私有 RPC 发行者和权限边界保持不变。

修复后，实际 R19 普通产品用例通过首请求、exec、出网、inspection、PTY、取消、网络拦截和 code-only 更新断言。
该用例在镜像 rollout 的新 target2 Running 等待处失败；整例耗时 221 秒。
四十五秒等待期间有四十四次 `RuntimeUnavailable`，且 Engine 未创建 replacement。
临近失败收尾时出现的新租约和 Engine start，不证明等待期间已进入 Starting。
原证据保留于 `.temp/p23-full64/r19-alarm-ordinary-failure-readonly/`。

所属回归确认 cold control start 的 grant 登记晚于 lazy class 句柄返回。
当前源码在可信 snapshot 校验后登记同一 grant；lazy code callback 只引用该 grant。
原 class、namespace、root facet、撤销和弱 owner 边界保持不变。
十三项本地所属回归、严格类型和资产构建均通过。
后续 Linux 构建和 Broker 资格检查通过；普通用例曾通过真实 rollout。
另一次普通用例仍在 rollout 的四十五秒等待处失败。
证据为 `.temp/p23-full64/r19-rollout-grant-issuance-candidate/root-local-validation-result.json`。

2026-10-14，所属回归确认并发 route advance 可留下两个有效 grant。
当前 host 依次加载 tenant class 和 metadata，并把删除纳入同一顺序。
执行 tenant 方法和原生 start 不占用这段顺序。
三十七项所属回归、严格类型和资产构建均通过。
Linux 构建、Broker 资格检查和新正常测试程序的编译通过。
canonical Clippy、无默认 features 和 MSRV 检查均通过。
编译后恢复 4 GiB、2 CPU，原配置字节和输入守卫均通过。

2026-10-14 09:07 UTC，普通用例通过 rollout、重启和九十秒 inactivity 的原断言。
命名镜像、信号和 exec options 的原断言也已执行通过。
整例仍失败；结果为零项通过、一项失败、一项 filtered。
首个失败请求写入 snapshot 文件，Python 返回 `PermissionError`。
用例尚未调用 `snapshotContainer()`，SQLite 没有 snapshot 记录。
本地 Engine 探针确认默认用户 `oc-test` 可写 `/tmp`，但不能写 root 所属的 `/run`。
探针使用同一镜像；自有容器已删除，GET 返回 404。

测试夹具为 snapshot 文件的写入和读取显式指定公开 exec 选项 `user: "0:0"`。
主容器用户、文件 mode `0640`、`/run` 排除和快照断言保持不变。
官方 strict TypeScript、cf build、baseline identity 和格式检查均通过。
两个宿主已同步三个测试输入文件；生产编译输入和五个正常测试程序摘要均未变化。
失败请求的证据为 `.temp/p23-full64/r19-ad8-snapshot-permission-failure-readonly/`。
夹具修复和输入证明位于 `.temp/p23-full64/r19-snapshot-fixture-user-repair/`。
修复后的普通用例通过 snapshot 文件写入；整例仍失败，耗时 432.32 秒。
Engine 事件确认源容器已暂停、导出和恢复运行。
snapshot 请求在 30,007 毫秒返回 503；当前 transport 的响应头期限为三十秒。
SQLite 只留下 pending 记录，尚未登记 artifact 或尝试 PUT。
五个持有 FD、RPC 观察和测试程序守卫通过。
一项输入后置守卫失败，失败瞬间的具体条件未保存。
随后只读检查确认输入、资源、Provider 和 Engine 身份均正常。
原始失败与后续检查分别保留；完整 snapshot 验收仍未通过。
证据为 `.temp/p23-full64/r19-snapshot-user-capture-failure-readonly/`。

随后只添加阶段日志的普通用例仍失败，耗时 397.86 秒。
rootfs 在 23.3 秒交给 service；请求在 30,004 毫秒返回 503。
当时流程尚未完成源镜像校验，不能据此认定发布或恢复已执行。
原始阶段证据位于 `.temp/p23-full64/r19-snapshot-phase-failure-readonly/`。
当前 spool 使用 256 KiB 缓冲合并小帧写入，并在交付文件前刷新缓冲。
字节上限、完整 tar 校验、错误处理和响应头期限保持不变。

本地检查还发现匿名文件权限缺陷。
Linux 的 `O_TMPFILE` 路径在默认 `umask 022` 下创建 `0644` 文件。
发布 API 正确要求私有文件，因此原匿名发布五项测试均失败。
三个内部调用点现于写入前设置 `0600` 权限；发布 API 的拒绝条件保持不变。
测试夹具也显式声明私有输入，不更改进程的 `umask`。

2026-10-14，私有权限修复的三十二项所属测试在 4 GiB、2 CPU、`umask 022` 下通过。
这些测试包括 spool 两项、Broker snapshot 十项、service snapshot 十三项。
匿名发布五项和私有文件正反例两项也通过；公开权限和硬链接仍被拒绝。
修复后的生产构建、新 Provider 资格和正常测试程序构建均通过。
Clippy 测试检查和 MSRV 检查在修正两处测试导入后通过。
已通过且输入未变的生产 Clippy 与 no-default 检查只保留原有证明。
输入、五个文件身份、RPC、资源和库存守卫通过。
证据位于 `.temp/p23-full64/r19-private-snapshot-spool-local-build-preparation/`。
这些结果不证明真实 snapshot capture、restore 或完整 ordinary 用例通过。
后续普通用例仍失败，耗时 398.50 秒；首个失败请求为 snapshot capture 的 503。
本次 rootfs 在 22.872 秒交给 service，截止前仍未完成源镜像校验。
SQLite 只留下 pending 记录；输入、文件身份、RPC、资源和库存后置守卫均通过。
原始数据库、WAL 和 SHM 在隔离复制及查询前后保持相同字节。
证据为 `.temp/p23-full64/r19-private-spool-capture-failure-readonly/`。
现有源码并行校验源镜像和捕获 rootfs，并等待两个操作结束后再交付结果。
完整校验、Provider 错误优先级、发布权限和原期限保持不变。
并行修复的生产构建、Provider 资格、静态检查和正常测试程序构建均通过。
service snapshot 十三项回归通过；前述三十二项结果仍属于修复前的权限版本。
2026-10-14，并行版本的普通用例仍失败，耗时 398.66 秒。
源镜像校验在 21.627 秒完成，rootfs 在 23.729 秒就绪。
三十秒期限前仍未完成归档写入，snapshot capture 返回 503。
SQLite 只留下 pending 记录；五个文件身份、RPC 和测试程序身份保持不变。
结束门禁另报输入检查失败；稍后只读复核的资源、输入和库存检查通过。
失败瞬间的输入检查条件未留存，不能用后续结果覆盖原失败。
证据位于 `.temp/p23-full64/r19-parallel-snapshot-capture-failure-readonly/`。

当前源码复用完整 tar 校验结果构造单层 snapshot graph，省去同一 tar 的第二遍校验。
写出过程仍重新计算全部 blob 的摘要，并核对字节数。
单层数量、JSON、tar 和总字节上限保持不变。
归档输出使用 256 KiB 缓冲，并在同步和计算完整摘要前刷新缓冲。
归档改动的生产构建、静态检查和正常测试程序构建均通过。
service snapshot 十三项与归档四项回归通过，运行条件为 4 GiB、2 CPU、`umask 022`。
新 Provider 的两项探针通过，原资格流程在最后的资源检查失败。
只读补充复核通过全部后置条件，并复用原探针证据；原失败记录保持不变。
旧 Provider 已精准退休，当前源码和新 Provider 的冻结检查通过。

2026-10-14，归档版本的普通用例仍失败，耗时 403.64 秒。
snapshot capture 返回 503；源镜像校验在 21.135 秒完成，rootfs 在 28.332 秒就绪。
期限前仍无归档写出、发布或物化完成记录，SQLite 只留下 pending 记录。
五个文件身份、RPC、测试程序身份和 Engine 库存差异检查通过。
结束输入检查仍失败；稍后只读复核的资源、输入和库存检查通过。
失败瞬间的检查条件未留存，归档改动的耗时改善也未确认。
证据位于 `.temp/p23-full64/r19-archive-writing-capture-failure-readonly/`。

共享 tar 校验器新增 256 KiB 读取缓冲，字节计数器仍位于缓冲下方。
2026-10-14，缓冲版本的生产构建、Provider 资格和静态检查通过。
独立 artifacts 测试程序构建通过，镜像、归档和 snapshot 十九项回归全部通过。
运行条件为 4 GiB、2 CPU、`umask 022`；完整发现为 116 项，未运行的 97 项被过滤。
新增回归检查完整摘要、批量读取和上限外最多读取一个字节。
服务测试程序构建失败，MBX 报告正常 `libcapnp` 的 `.rmeta` 在编译期间变化。
五个既有文件守卫和 RPC 检查通过，但本次守卫未观察报错的 `.rmeta`。
只读复核中，当前 `.rmeta` 字节和文件身份与 MBX ledger 相同。
编译前的 MBX 比较值未留存，不能认定工具误报或实际文件未变。
首次失败后，编译流程恢复了 4 GiB、2 CPU 和原配置字节。
随后单次诊断构建追加报错 `.rmeta` 的同 FD 九字段、SHA-256 和 BLAKE3 观察。
正常测试程序构建通过，原有五文件守卫、RPC 和新增观察均通过。
原失败的 MBX 比较值仍未知；本次成功不能证明原失败原因已修复。
另外两项 spool 回归通过，运行条件为 4 GiB、2 CPU、`umask 022`。
原断言和期限保持不变，真实产品仍待复验。
证据位于 `.temp/p23-full64/r19-buffered-tar-local-build-preparation/`。
独立诊断为 `.temp/p23-full64/r19-normal-capnp-rmeta-guard-readonly/`。
真实 snapshot capture、restore 和完整 ordinary 用例仍未通过。

2026-10-14，缓冲版本的普通用例再次失败，耗时 372.97 秒。
用例在命名容器停止后的 `provider.observe()` 处收到 `CONFIG_INVALID`。
用例尚未调用 snapshot capture。
结束库存检查发现 Broker 合约返回 503，但无残留容器。
五个文件守卫、RPC、测试程序身份和资源检查通过。
Broker 的持续连接正常结束，Engine 进程和 socket 身份未变。
连接关闭的发起原因仍未知，归档缓冲的耗时改善也未确认。

真实 Unix/Hyper 回归复现了另一项确定缺陷。
取消健康探针的调用方会关闭共享 Engine 连接。
修复复用既有 keepalive 任务完成探针，调用方只等待完成通知。
原 5 秒期限、持续连接失效后的永久封锁和禁止重连规则保持不变。
修复后的四项回归全部通过，运行条件为 4 GiB、2 CPU。
回归包含取消、异常响应、连接关闭和 socket 替换。
完整发现为 975 项，未运行的 971 项被过滤。
原始现场与取消回归的因果关系尚未确认。
新 Broker 的生产构建和资格检查通过，旧 Broker 已精准退休。
当前源码的静态检查和正常测试程序构建通过。
构建后恢复了 4 GiB、2 CPU 和原配置字节，文件与 RPC 观察通过。
真实产品复验仍失败，耗时 404.13 秒。
命名容器停止后的 `observe()` 已通过，结束时 Broker 合约仍返回 200。
首个失败请求为 snapshot capture 的 503。
源镜像校验在 20.274 秒完成，rootfs 在 23.503 秒就绪。
期限前仍无归档写出、发布或物化完成记录，SQLite 只留下 pending 记录。
五个文件身份、RPC 和测试程序身份保持不变。
结束输入检查另报失败，失败瞬间的具体条件未留存。
随后只读检查确认当前输入、资源、Provider 和 Engine 身份均正常。
原始失败与后续检查分别保留，不能用后续结果覆盖原失败。
证据位于 `.temp/p23-full64/r19-anchor-snapshot-capture-failure-readonly/`。
取消回归的证据位于 `.temp/p23-full64/r19-buffered-tar-named-stop-failure-readonly/`。

snapshot 夹具现立即返回响应头，正文等待真实 `snapshotContainer()` 完成后才发送 JSON。
三项夹具回归核对等待中的正文、完成后的完整 handle 和两种失败路径。
响应头三十秒期限和 HTTP 正文总计 180 秒期限保持不变。
快照状态、实际字节数、SHA-256、恢复和重启断言保持不变。
这项夹具修复尚未通过真实产品复验。
Mac 和 Linux 的三项夹具回归及官方 strict／cf build 均通过。
2026-10-14，流式夹具的普通复验仍失败，耗时 217.47 秒。
本次在镜像 rollout 等待 target 2 Running 时触发 45 秒期限，尚未到快照。
SQLite 中的旧 lease 已停止，target 2 没有新 lease；两个 rollout 仍在 step 0。
日志包含 45 次 `RuntimeUnavailable`，Provider 合约仍返回 200。
输入、资源、五个文件身份、RPC、测试程序和库存后置检查均通过。
原始数据库、WAL 和 SHM 在隔离复制及查询前后保持相同字节。
隔离查询确认对象、命名空间和当前版本的授权条件均成立。
证据位于 `.temp/p23-full64/r19-streaming-rollout-failure-readonly/`。

当前 Rust 仅在测试配置中记录内层已有的固定错误码和 HTTP 状态。
诊断拒绝未知 header 内容，不读取正文，也不改变错误返回或期限。
2026-10-14，拒绝码诊断版的五组 canonical Clippy、无默认 features 和 MSRV 检查均通过。
正常 service library 和 P23 测试程序编译通过；五个依赖文件及 RPC／rmeta 观察保持不变。
编译后恢复为 4 GiB、2 CPU，原配置字节保持不变。
容器启动控制和 alarm 协议两项所属测试通过，各为 1 PASS、0 FAIL。
隐私断言核对已知、未知和缺失 header，原认证、长度限制及错误断言保持通过。
证据位于 `.temp/p23-full64/r19-replacement-rejection-diagnostic-local-build-preparation/`。

新 Broker 的原资格报告因后置预算检查失败，仍保留失败结果。
独立只读复核通过，复用原两项探针，不创建新探针。
随后精确退休旧 Provider，当前 Provider 为 PID 244675。
新普通单例仍失败，耗时 217.79 秒，计数为 0 PASS、1 FAIL。
全部输入、资源、文件身份、RPC、测试程序和库存后置检查通过。
镜像 rollout 尚无 target 2 lease，快照仍未开始。

内层日志实际记录 45 次 HTTP 500 和 `DO_RUNTIME_EXCEPTION`。
这 45 次记录与 reconcile 的 45 次 `RuntimeUnavailable` 逐次对应。
原始日志和数据库在只读复制及查询前后保持不变。
早期 native 对照使用委托 Loader，挂接因合法的原生权限检查失败。
修正为生产所用的配置宿主 Loader 后，正式 R19 对照通过。
只取得 lazy facet stub 时，start 未进入租约解析。
先经 facet 完成 RPC 激活后，start 到达租约解析，连续三代撤销和重建均通过。
探针没有创建 Docker 执行实例，不证明完整产品通过。
证据位于 `.temp/p23-full64/r19-rejection-diagnostic-product-readonly/`。

DoHost 现通过内部初始化 RPC 登记原生 tenant facet，再启动捕获的 grant。
class 与 grant 在同一 transition 中取得；初始化和 start 在 transition 外执行。
route 切换撤销旧 grant，旧请求不能转而启动 replacement grant。
内部方法保留在 wrapper 的固定方法表中，公开 RPC 仍拒绝保留名称。
所属五组 Node 测试通过，计数为 71 PASS、0 FAIL、0 SKIP。
strict 和 runtime 资产构建通过。
证据位于 `.temp/p23-full64/r19-lazy-facet-initialization-candidate/`。

初始化版的本地 Broker 构建、三项 Rust 静态检查和正常测试程序编译通过。
资格流程的两个真实探针通过，但首次资源后置检查失败。
独立只读复核通过，原失败记录保留；旧 Provider 已精确退出。
编译后恢复为 4 GiB、2 CPU，原配置字节不变。

本版普通单例仍失败，耗时 351.72 秒，计数为 0 PASS、1 FAIL。
两个镜像 rollout 已完成；前序重启和九十秒 inactivity 的原断言已通过。
HTTP JSON 解析因空内容返回 EOF，具体请求尚未确认。
数据库没有 snapshot 记录；全部输入、资源、文件、RPC 和库存后置检查通过。
证据位于 `.temp/p23-full64/r19-lazy-facet-product-readonly/`。

测试夹具现记录 snapshot 调用开始、返回或拒绝的固定阶段。
记录只含固定阶段、白名单错误码和响应字节数，不含原始异常或凭据。
原生调用、成功句柄断言、失败流和时限不变。
三项所属 Node 测试、两项严格类型检查和本地 cf 构建通过。
首次构建遗漏已准备的输入路径，失败记录保留；补入后仅重跑 cf 构建。
诊断夹具的真实普通单例仍失败，耗时 353.33 秒，计数为 0 PASS、1 FAIL。
持久阶段为 `CAPTURE_REQUESTED`、`CAPTURE_REJECTED`，没有返回句柄。
二十四个 Host／tenant 数据库在隔离复制及查询前后保持原始字节和身份。
证据位于 `.temp/p23-full64/r19-snapshot-empty-diagnostic-product-readonly/`。

本地 native 流式对照最初因测试助手只接受固定 Content-Length 而失败。
该失败不能证明 native 回调不能执行 I/O；原失败日志保留。
修正 HTTP 分块响应断言后，原始 R19 对照通过，计数为 1 PASS、0 FAIL。
调查用的 native 生产和测试改动已逐字节恢复，fork 工作树干净。
该对照使用既有测试宿主配置，不证明完整产品的私有策略路径通过。
证据位于 `.temp/p23-full64/r19-native-snapshot-policy-stream-regression/`。

私有请求和捕获前置阶段现增加仅 `test-support` 编译的固定日志。
日志只含固定阶段、数值状态或标准错误码，不含请求体、凭据或原始异常。
权限、来源、reservation、预算和错误传播不变；新输入尚未构建或资格化。
证据位于 `.temp/p23-full64/r19-snapshot-private-preflight-diagnostic-candidate/`。

2026-10-14，私有阶段日志版已完成本地构建、Broker 资格和正常测试程序编译。
后续两次夹具修复只改测试数据；实际生产 binary 摘要保持一致。
测试改用 `durable_object` authority 和声明的 main image，并给模拟 Engine 设置正数 image size。
原私有 endpoint 用例通过，计数为 1 PASS、0 FAIL；原成功断言全部保留。
这些测试修复不解释真实 named application 的失败；真实 application 原本已使用 `durable_object` policy。

本版真实普通单例仍失败，耗时 357.41 秒，计数为 0 PASS、1 FAIL。
持久阶段为 `CAPTURE_REQUESTED`、`CAPTURE_REJECTED`，没有 snapshot 记录。
私有 Rust snapshot 入口没有固定阶段日志；其他 Rust 日志正常。
全部输入、文件、RPC、Provider、Engine、库存和 4 GiB 后置守卫通过。
证据位于 `.temp/p23-full64/r19-positive-size-snapshot-product-readonly/`。

本地原生对照已复现 capture 回调的 I/O 边界缺陷。
对照使用当前 `containerAttachment` 代码、真实 native grant 和声明的测试 Service。
原版 R19 的普通响应通过；流式响应在进入 capture 回调前拒绝，容器仍为 running。
仅让 lease resolver 执行 Service I/O 的精简回归也在原版失败。
候选用既有 Root control event 承载 capture，并保留前后 weak owner 和 exact lease 校验。
未加诊断日志的当前 producer 对照和精简维护回归均通过。

候选的本地 workerd 构建和 7 个原生 owning target 通过。
首次 Root 批次为 9 PASS、2 FAIL；共享夹具的异步查询改变了两个 sidecar 流场景。
异步查询已限定到新 snapshot 回归，原断言和错误日志检查不变。
受影响的 Root 组复验为 11 PASS、0 FAIL。
这些结果证明本地原生回归，不证明完整产品的唯一失败原因已解除。
提交前，正式 pin 仍为 R19；候选尚未 push、运行 CI 或替换产品 runtime。
证据位于 `.temp/p23-full64/r19-snapshot-attachment-producer-native-control/`。

本地验证通过后，修复提交为 `1c0659a70e9a568a4b6e072fc4a8b5427a03120e`。
R20 的不可变供应构建仅启动一次：[run 38644356217](https://github.com/elliothux/workerd/actions/runs/38644356217)。
2026-10-15（UTC），四平台构建和 publish 均成功。
每个平台的 7 个原生 owning target 和 11 项 Root 回归均通过。
R20 收集器已通过 93 项纯检查，并完成一次严格资产收集。
exact Release tag 指向上述 revision；10 项不可变资产、签名和摘要已校验。
原始 GraphQL 的 5 个完整 `CheckRun` 与必需资格步骤均成功。
正式 lock 和父仓库 gitlink 已更新；新 Broker 和正常产品测试程序已使用 R20 输入重建。
资产证明为 `.temp/p23-full64/workerd-release-r20/verified-assets.json`。
原生资格与供应校验不证明真实 snapshot capture、restore 或完整 ordinary 用例通过。

R20 新 Broker 的两项真实资格 probe 通过，探测容器均已销毁并返回 404。
两项镜像大小与 Engine 一致，三次健康检查均返回 200。
旧 Provider 已通过原 PID 身份核验后退休。
新输入冻结、五组 canonical Clippy、无默认 features 检查和 MSRV 全目标检查通过。
正常 service-lib 与 P23 测试程序构建通过；编译后恢复为 4 GiB、2 CPU，原配置字节不变。
这些检查不包括插桩 coverage。

R20 普通用例复验耗时 473.40 秒，结果为 0 PASS、1 FAIL、1 filtered。
请求返回成功状态后，空响应体导致 JSON EOF；完整用例未通过。
安全阶段日志证明私有请求已进入 Rust 快照流程。
归档写入、哈希与外部发布均已完成；发布返回时耗时 86.783 秒。
后续镜像物化尚未完成，捕获返回 `RUNTIME_UNAVAILABLE`。
SQLite 中的快照状态为 `retiring`，tenant 记录为请求后拒绝。
原 DB、WAL、SHM 的九项文件属性和 SHA-256 在隔离查询前后保持一致。
后置输入守卫因 native 配额余量约 1.78 GB、低于 3 GiB 而失败。
该预算拒绝不证明镜像物化失败的唯一原因。
当前 cgroup 的 OOM 与 OOM-kill 计数均为零；内核日志没有本轮 OOM 记录。
不得据此宣称完整 snapshot capture 或 restore 通过，也不得原样重跑该用例。
构建与资格证据位于 `.temp/p23-full64/r20-formal-input-local-build-preparation/`。
失败与隔离诊断证据位于 `.temp/p23-full64/r20-snapshot-product-failure-readonly/`。

后续单层 snapshot 源码生成 gzip OCI layer，保留原始 tar 的 DiffID 和解压字节上限。
固定压缩器的读取尺寸后，两种归档读取方式生成相同字节。
2026-10-15（UTC），二十项 artifacts 所属回归在 4 GiB、2 CPU 下通过。
生产构建、Broker 资格、静态检查和正常测试程序构建通过。
证据位于 `.temp/p23-full64/r20-snapshot-gzip-layer-local-repair/` 和 `.temp/p23-full64/r20-snapshot-gzip-local-build-preparation/`。

测试夹具使用现有的 1 GiB 缓存配置；原镜像、断言和时限保持不变。
2026-10-15（UTC），本版普通用例耗时 564.80 秒，结果为 0 PASS、1 FAIL、1 filtered。
HTTP 正文触发原有 180 秒期限；rootfs 在 29.645 秒就绪，期限前没有归档完成记录。
输入、五个文件身份、RPC、内存、磁盘、Provider、Engine 和库存后置守卫通过。
失败证据位于 `.temp/p23-full64/r20-gzip-http-body-timeout-readonly/`。

编译记录表明，完整程序的 `flate2` 选择未优化的 `zlib-rs` 后端。
此前使用 `miniz_oxide` 的独立测量不代表这个后端的耗时。
2026-10-15（UTC），独立候选在 4 GiB、2 CPU 下使用相同的 `flate2` 后端 features。
候选仅将 `zlib-rs` 依赖设为 `opt-level=3`。
同一 1,530,102,784 字节原始层生成 687,477,248 字节 OCI 包，耗时 31.48 秒。
原始 DiffID、源文件和全部前后守卫保持不变；该结果不证明完整产品通过。
当前构建配置已加入这项局部优化，原 HTTP 期限保持不变。
候选证据位于 `.temp/p23-full64/r20-snapshot-gzip-local-build-preparation/production-gzip-zlib-profile-candidate/`。

2026-10-15（UTC），后端配置版本的 Linux 生产构建和 Broker 资格检查通过。
五组 canonical Clippy、无默认 features 和 MSRV 检查均通过。
正常 service library 与 P23 测试程序编译通过。
编译记录确认两个实际 `zlib-rs` feature 组合均使用 `opt-level=3`。
二十项镜像所属回归使用实际后端，在 4 GiB、2 CPU 下全部通过。
完整发现为 117 项；未执行的 97 项被过滤，没有 ignored 项。
编译后恢复为 4 GiB、2 CPU，原配置字节保持不变。
五个持有文件、RPC 和三项依赖观察的前后检查通过。
证据位于 `.temp/p23-full64/r20-zlib-profile-local-build-preparation/`。

2026-10-15（UTC），本版普通单例在 4 GiB、2 CPU 下仍失败，耗时 514.90 秒。
结果为零项通过、一项失败、一项 filtered。
快照归档在 60.460 秒写完，镜像物化在 133.005 秒完成。
持久快照状态为 `ready`，OCI 包大小为 687,477,248 字节。
跨对象恢复的内容、`0640` 权限和临时文件排除断言均通过。
后续 inspection 预期运行状态，却返回 `running=false` 和 `info=null`。
测试在恢复后先扫描完整归档，再查询即时运行状态。
两个请求的完成时间相隔 18.63 秒，超过原生默认的 10 秒闲置期。
持久停止意图出现在恢复请求完成约 10 秒后。
测试现先检查即时状态，再扫描完整归档；原断言和时限保持不变。
改动后的格式、所属 Clippy 和正常测试程序编译通过。
证据位于 `.temp/p23-full64/r20-restored-inspection-failure-readonly/`。

失败结束时，Engine 留有本次新建的 snapshot 镜像，触发库存后置检查失败。
原输入后置检查也保留失败结果；五个持有文件和 RPC 观察通过。
随后只读检查确认生产输入、4 GiB 资源、Provider 和 Engine 身份正常。
派生镜像在无容器引用后已精确回收，Engine GET 返回 404。
原始归档与校验副本保留，数据库、WAL、SHM、密钥和唯一数据保留。
2026-10-16（UTC），Docker 专用 XFS 的限定范围 `fstrim` 已执行。
完整输入、唯一数据、Provider、Engine 和库存检查通过。
清理后的 native 配额满足普通用例的峰值估计和保留余量。
证据位于 `.temp/p23-full64/r20-docker-xfs-unused-block-plan/`。

同日，正式 R20 普通单例在 4 GiB、2 CPU 下通过，耗时 556.87 秒。
测试发现两项，精确执行普通单例，结果为一项通过、零项失败、一项 filtered。
用例通过 rollout、重启、九十秒 inactivity、命名镜像、信号和 exec 断言。
快照捕获、跨对象恢复、实际对象存储字节和 SHA-256 断言通过。
helper 排除和 daemon 重启后的恢复断言也通过。
独立 rootfs 用例被过滤；本次结果不证明完整 P23 目标通过。

原执行器仍返回失败，因为镜像标记解析器未接受 libtest 的同行前缀。
原普通测试进程的 exit code 为零，原执行器的失败结果保留。
解析器只补入固定前缀，精确镜像身份和无容器引用条件保持不变。
独立补充流程回收本次派生镜像，Engine GET 返回 404。
全部输入、文件、RPC、资源和库存后置检查通过；普通单例没有重复执行。
证据位于 `.temp/p23-full64/r20-restored-inspection-failure-readonly/inspection-before-archive-candidate/ordinary-maintained-table-candidate/`。

早期 workspace coverage 的 69 个插桩测试程序已编译，编译后恢复为 4 GiB、2 CPU。
coverage 阶段另做一次限定范围的 XFS 空闲块回收，实际 native 余量为 8,773,373,952 字节。
峰值估计和保留余量检查通过；原 workspace coverage 批次只启动一次。
批次完成全部 63 个目标，其中 58 个通过、5 个失败，共有 1,075 项用例通过。
失败目标为 service library、P1 conformance、Service RPC、Python runtime 和 P23。
service library 的三项失败与 P1 失败来自旧测试合同。
测试现按固定清单保留已验证成员和 blocked 成员，并验证非法 execution 覆盖仍被拒绝。
Python 夹具改用 `await repo.info()`；原 Rust 断言保持不变。
原工具组合已重建夹具并完成本机 cf 捕获，十九个 SDK 模块的字节与 MIME 均未变。
Service RPC 在 capability 断言处收到 `RUNTIME_INTERNAL`，具体步骤仍待固定阶段诊断。
插桩 P23 的 rootfs 用例通过，普通用例在 HTTP 正文的 180 秒期限处失败。
隔离数据库记录首个快照为 pending，两个 rollout 均已完成。
固定阶段日志记录 source configuration 校验和 rootfs 捕获已完成，归档写入未记录完成。
这些记录尚未确定具体性能原因；原断言和时限保持不变。
原失败报告保留；测试修正与诊断已应用，真实失败项仍待复验。
整批结束后，完整输入、五个文件与六个 RPC／编译输入观察器检查通过。
配额余量为 8,071,860,224 字节，资源保持 4 GiB、2 CPU。
证据位于 `.temp/p23-full64/r20-current-coverage-preparation/`。

上述旧批次未通过完整 workspace coverage，原失败证据保留。
当前 coverage、最终 Gate 和各平台包的结果见页首；完整兼容资格仍待完成。

2026-10-13，当前 Rust 输入的五组 canonical Clippy 检查通过。
workspace 无默认 features 检查和 MSRV 全目标检查均通过。
编译环境为 8 GiB、2 CPU；检查后恢复为 4 GiB、2 CPU。
输入、文件身份、Provider、Engine 和原配置字节的守卫均通过。
证据为 `.temp/p23-full64/rust-websocket-handoff-remaining-rust-checks-preparation/root-actual-review.json`。
这些检查使用正式 R17 编译输入，不证明 R19 产品资格。

Broker 另有独立的 Wait 正文期限问题。
Docker 先返回 headers，再等待容器退出或删除；正常正文可能静默超过 30 秒。
候选修复移除 Wait 的正文空闲期限，保留字节上限和取消路径。
其他响应继续使用 30 秒空闲期限。
2026-10-13，三项新增回归均在实际 4 GiB、2 CPU machine 上通过。
回归使用正式 R15 编译输入，验证 31 秒静默正文、shutdown 取消和字节上限。
该结果只证明 Broker 回归，不证明后续正式 pin 的产品或新 Broker 资格。
R17 的新 Broker 已通过两项实际资格 probe，并完成探测容器的销毁和清理验证。
该资格仍不证明完整产品用例通过。
该问题不解释上述保活策略重置。

named images 保存于不可变 Worker Version 声明。`Container.images` 返回独立副本。
`durable_object` application 没有默认 image 或资源 configuration。
调用方应在 `start()` 中显式选择 image 或 snapshot。
省略 instance 时使用 Lite；显式资源仍受已验证的 operator 上限约束。
`default` policy 从 target 取得 image 和资源，拒绝 startup 中的 image、instance 和资源覆盖。
snapshot capture 与 restore 仅对 `durable_object` policy 开放。
配置和 admission 边界见 [实例 admission](../references/containers.md#实例-admission)。

快照源码以 SQLite 保存捕获身份、artifact、保留期限与配额。
Broker 暂停并导出已验证的进程，确认同一进程恢复后才返回 rootfs。
service 生成并校验完整 OCI artifact；跨 DO 恢复仍受同一 application authority 约束。
候选源码从新快照镜像读取实际 Docker `ImageInspect.Size`，并独立持久化为 `image_size`。
集成候选记录为 `.temp/p23-full64/snapshot-size-integration/candidate-v3.json`。
候选包含 20 项源码摘要；格式和源码规则检查通过，Rust 测试尚未执行。
失败矩阵覆盖 load 成功后缺少最终镜像证明的路径；新增场景尚未执行。
storage、artifacts、workers 和 service 的 Darwin 类型预检均成功退出。
service 预检使用正式 R9 构建输入，不证明 R15 的运行行为。
预检记录位于 `.temp/p23-full64/` 下的三个 `*-compile-preflight/result.json`。
Provider 私有协议升为 v2；原 v1 Broker 的资格证据不覆盖新候选。
public `size` 使用该值；artifact 长度继续用于 digest 和配额。
正式 R20 普通单例已核对实际 Engine 值；托管完整计量公式仍未确定。
未知 PUT 继续占配额，不因 HEAD 404 自动释放。
TTL、恢复续期、私有 image 和回收边界以运维合同为准。

2026-10-16（UTC），下表记录正式 R20 未插桩普通单例的有限结果。
单例在 4 GiB、2 CPU 下耗时 556.87 秒，一项通过、零项失败。
这些结果不证明固定清单的全部 65 个成员通过，当前成员资格仍为 `blocked`。
早期插桩批次的普通用例失败；当前插桩结果见页首，完整验收仍待完成。

| 新维护场景                      | 已完成检查与待验收边界                                                                                                             |
| ------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| named images                    | 普通单例通过两个 alias、getter 副本、缺 image 不占 lease、停止后换镜像和重启后的 map 断言                                          |
| instance 与资源                 | 普通单例通过默认 Lite、自定义资源和字符串 Lite 的真实 execution 断言，并核对 SQLite 与 Engine 配置                                 |
| snapshot                        | 普通单例通过实际 S3 字节数、SHA-256、跨 DO 恢复、内容与 mode、`/run` 和 helper 排除、即时 inspection 及 daemon 重启后的恢复断言    |
| exec 生命周期                   | 普通单例通过 PID、user/cwd、无 shell、后代输出、helper 失效和 destroy 清理断言                                                     |
| signal、monitor 与 exec options | 普通单例通过主进程信号 `10`／`34`、monitor 退出码、destroy 原因身份、exec kill、stdin EOF、ignore、combined 和重复 output 拒绝断言 |
| HTTP 拦截                       | 普通单例通过指定主机和兜底规则的回调断言                                                                                           |
| HTTPS、WebSocket 与 PTY         | 普通单例通过 HTTPS、WebSocket 关闭握手和 PTY 输入输出、resize、环境变量断言                                                        |

新增场景仍归入同一个真实 P23 产品用例，不新增生产测试分支。
fixture 准备证据标识为 `p23-named-images-distinct-all-resources/candidate.json`。
该记录证明 strict、cf build 与格式检查成功，不证明产品执行成功。
数值信号 `34` 的新增场景见 `p23-main34-product-tests/candidate.json`。
该候选阶段的 strict、cf build 和两项 fixture 自检通过。
后续 R20 的有限产品结果见上表。

Rust 所属回归已验证 Lite 默认值、命名边界和 default／named-map 事务竞态。
storage 原批为 45/46 PASS；修正测试清理参数后，失败项单独重跑为 1/1 PASS。
service 的 startup 6/6、upload 4/4 与 named HTTP 2/2 PASS。
自动故障回滚也检查策略关联；冲突候选不激活，故障部署仍完成隔离。
冲突回滚与合法回滚的两项 SQLite 回归均 PASS。
证据清单为 `named-images-rust/candidate-final-v2-quarantine.json`。
这些检查使用正式 R9 编译输入，不证明 R15／v3 的原生产品行为。

托管 CF15 的 HTTPS 与 WebSocket 子集已成功，CF17 的 PTY 子集也已成功。
请求、TLS、升级、关闭握手、resize 与清理范围见 [CF 对比证据与冲突](../references/containers.md#cf-对比证据与冲突)。
恢复后的 `inspect().image` 与省略 snapshot 参数仍有两项来源冲突。
官方文档已说明 PTY stderr 的 `null` 类型声明与 `undefined` 运行值差异。
平台保留上游声明和运行行为；当前正式 runtime 的成员验收仍未完成。
快照 `size` 的候选修复复用上游的 Engine 镜像大小字段；托管计量公式仍有证据缺口。
目录 API 与非空 `directorySnapshots` 的成功语义也未获证明。
这些范围继续保持 `blocked`。
CF18 的直接 PID 1 信号 `34` 对照返回退出码 `23`。
独立 Container API 调用后的 12.225 秒观察仍未完成，兼容性继续保持 `blocked`。
mount-only 前后的普通启动对照成功，但 mount-only 本次仍未启动。
CF18 已精确清理资源；有限观察不证明信号永久忽略或全部目录输入不可用。
CF19 的两个内容不同的 snapshot 在不同 DO 中恢复正确，`inspect().image` 却相同。
公开 metadata 没有提供 image 对应关系；base／派生镜像身份仍未确定。
独立复核确认三项库存还原、临时 secret 不存在。
application 删除不证明 snapshot 字节回收或计费停止。
该结果不解除上述来源冲突，也不替代本地成员验收。
后续正式 pin、当前 coverage 和最终 Gate 已通过；各平台包的结果与未验证范围见页首。

## 1. 已落地范围

应用使用固定 cf、官方 Vite 插件和原版 `@cloudflare/containers@0.3.7`。
Worker 的 SQLite-backed Durable Object 通过原生 `ctx.container` 执行，不改写官方 package。
动态 attachment 只授予已声明的 Container class；旧 facet 或 generation 不授予新执行的控制权限。
code-only deploy 保留已有 execution。image 或 resource rollout 发布新的 immutable target。

基础集合为 bridge inspect、container create/inspect/start/stop/kill/wait/delete、
exec create/start/resize/inspect 和 archive PUT，共 13 个 family。
Broker 验证固定 route、query、schema、状态码、HTTP upgrade、stream framing 和 tar 路径。
未知操作和危险 HostConfig、mount、device、capability 被拒绝；不提供通用 Docker 透传。

cf 管理面已接入 application、immutable image、instance 和 rollout。
当前 API 只接受零 `rollout_active_grace_period`，rolling steps 使用同一规范化和事务路径。
不提供全球 placement、fleet autoscaling、计费或每实例独立 VM。

## 2. Authority 与恢复

SQLite 持有 image、application target、Worker Version declaration 和单调 execution lease generation。
V16 保存有序 layer diff IDs、effective resources、AEAD startup envelope 和持久 stop intent；已发布 SQL 保持原字节。
manifest、config、全部 layer digest、diff IDs、架构和压缩／展开预算在 admission 前完整校验。
application create/update 只提交已准入 target，Engine 缓存不构成 image authority。
execution start 先保留 lease 和容量，再证明 Engine 图；只有明确 404 才离线导入并重新证明。

`lost`、未知 create/start 和 provider 失效仍占容量，不自动创建 replacement。
Broker 与 daemon 重启后，cleanup-only 登记只收养冻结的 lease，不启动执行或颁发凭据。
maintenance 继续持久停止意图；确认终止、清理和迟到操作 fencing 后才释放容量。
旧 key 的停止幂等受同一事务中的完整 facet、历史 identity、stop intent 和无 replacement 条件约束。

## 3. Provider 与安全边界

Docker 与 Broker 由 operator 独立部署和管理；普通 `ocd run` 不下载、启动或 supervise 两者。
只有 Broker 持有 Engine socket，workerd 与 `ocd` 使用私有 Unix endpoint。
资源资格验证实际 CPU、内存、磁盘、PID、文件与进程限制，不把 Docker 接受字段当作有效执行证明。
Engine continuity 失效后，Broker 保持 fail-closed，不自动重连或重新授权。

sidecar profile v2 使用控制端与回调凭据。token 不进入 argv、env、公开响应或 image artifact。
application 固定移除 `NET_RAW`；网络命名空间精确绑定已验证 proxy 的真实 Docker ID。
反向连接的 idle reader 在交接前退出；关闭或取用 unused connection 立即释放池槽位。
Owner、client 与 attachment grant 分别撤销所属回调，不把 native Owner 退出当作 Engine 终止。
宿主允许的 IP 网络仍由 operator 管理；平台不承诺 provider 网络直连隔离。

## 4. 验证记录与待完成检查

正式输入为 `v1.20260930.0-open-compute-r9.a8e3d0bd4`，revision 为
`a8e3d0bd40dff4c3457d0eb75acd000601f4b7b2`。
workerd lock SHA-256 为 `9205ca08efcb8327b8a96b174102c819dd5d21e8630140969b13725b3aa0f573`。
[R9 四平台 CI](https://github.com/elliothux/workerd/actions/runs/38093474159) 与不可变资产校验已通过。
签名、GitHub digest 和 `SHA256SUMS` 均已核验；原生资格不代替产品验收。

2026-10-11，已测产品 source 为
`266f5a459610acde26156a9a7c386e42097b9f9342d7f78d1eb6b5a95cabc6b6`。
产品执行条件为 4 GiB／2 CPU，package candidate 为 `b481cb419ab4e7e4e0591e8ddd039fb99bcbbbd5`。

| 检查                      | 当前结果                                                                                                                           | 关键证据标识                                                                                                                                                 |
| ------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Linux Provider            | 独立资源与 sidecar 资格通过；两个 probe 已清理，Engine 库存未变；不证明完整 Container API                                          | [Provider 合同](../references/containers.md)                                                                                                                 |
| R9 exact3                 | 三项 EXIT 0；原版 cf＋SDK、registry 凭据拒绝和不完整／未引用 image graph 边界                                                      | `r9-boundary-and-sdk-current-socket-8ec1/execution.json`                                                                                                     |
| Workspace coverage        | accepted reuse，90.634663%（165,314／182,396）；原 63 目标批次仍为 61 PASS／2 FAIL；测试修正后两个目标复验通过，生产与配置字节未变 | `coverage-r9-36fa-diagnostic-export-plan/export-canonical-objects/diagnostic-result.json`；`coverage-r9-36fa-mismatch-diagnostic/coverage-reuse-review.json` |
| 未插桩最终 workspace Gate | EXIT 1，63 目标中 62 PASS；唯一失败为 `p21-python-frameworks`，原目标随后 scoped 3/3 PASS；原 1102 原因仍未知                      | `20261011T125223-2cd33e9b/report.json`；`20261011T143221-b0d9ee9d/report.json`                                                                               |
| Darwin ARM package        | `d10` 构建 EXIT 0；Q10 原四项 4/4 PASS；两组 ACL 已恢复，临时账号与目录已删除                                                      | `d10/result.json`；`darwin-r9-single-admin-q10/q10-original-product-acceptance.json`；`darwin-r9-single-admin-q10/q10-gate-target-00-output.log`             |
| Linux ARM package         | 当前范围完成：构建身份复核通过；8 GiB 编译准备 EXIT 0；4 GiB／2 CPU 原四项 package 用例与两项 Dashboard smoke 均 PASS              | `package-linux-arm64-b481-gate-qualifier-v3/result.json`；`package-linux-arm64-b481-dashboard-qualifier-v3-bunx/result.json`                                 |
| Linux x64 package         | 本次原四项范围完成：构建与编译准备 EXIT 0；4 GiB／2 CPU 原四项 Gate 4/4 PASS；独立身份、资源与清理复核 EXIT 0                      | `package-linux-x64-b481-completed-gate-readonly-revalidated/result.json`；`20261011T184206-2c8758fa/report.json`                                             |

exact3 的原测 source 为 `8ec1a158b0c386aea03bfd243b37fbf9daeb7f1d4e33a21627bb6d370e1b59e3`。
原版 SDK 用例覆盖初次启动、12 秒 idle、HTTP／exec／egress、streams／取消、code-only、image rollout 和 daemon restart。
code-only 与 restart 保留 lease，image rollout 更换 lease；SQLite 状态延续和正清理均已验证。
已测 source `266f5a459610acde26156a9a7c386e42097b9f9342d7f78d1eb6b5a95cabc6b6` 的 16 个受审源文件按第 4.1 节复用证据。

coverage summary SHA-256 为 `e4eed077606fad12c86c5749ed3e96be07f03babd256e24ff866550a2c5c8f60`。
复用审查 SHA-256 为 `523eb38bc488caa6983189eb4308333db81746d810e7b21ac6909aaf08eea1f4`。
原测 source 为 `36fa2cde691658350d12451265eed88d4df08d3f8fd1e6c02d6c961eebd5ba9f`。
145 个 profile 与 68 个 object 的导出、报告验证及差异复盘已完成；原始副本已按保留规则清理。
summary、LCOV、复用审查和原失败报告保留；没有新增 coverage 排除项。

原 Final 报告 SHA-256 为 `66d3b0f6011afe950a3cc4f40f9c72bd6737688da5756c7afb4f8816596be9eb`。
P21 scoped 报告 SHA-256 为 `1ae3170c6e70ee0cd7bd8548a010a9bca37ac3cd1578e71f61458d7112f1558d`。
同一输入和 executable 的 invocation／普通 startup CPU 配置分别为 30,000／1,000 ms；原 1102 的 scope origin 和 CPU 消耗仍未知。
预算补充 SHA-256 为 `db27f5c0cb297309da3adf81b041813c8ee45f7529b659444b128d3adb9c96e8`。
scoped PASS 不改写原 Final 失败，也不构成已修复结论。

Linux ARM 产物为 185,536,464 字节，SHA-256 为 `f98c3d05d209187d0621c03f0dbc7148601a76fb67ab6a87c19e752a6271aa02`。
构建身份复核／编译准备报告 SHA-256 分别为 `ae1ed15ffb991da84c4f79c76ed31f297af55c9ae07b757ae105a3d7db0135c5`／`8cfe9c15fd5d27d75b68e3156be7b1cc55229b4aaaab0204ae9781d766c594d8`。
package Gate／Dashboard 报告 SHA-256 分别为 `039fe969d37cfd30d99e9fdcb6be63ac20b923b4a2afb95798530d861e1daf2b`／`43afc7de0d1711f9d21d8dacb716fca83353fe61b27058a00c2f4a8d2ef1ade6`。
Dashboard smoke 结束后确认 daemon 清理并保留任务 UserRoot；实际 home 下 `.open-compute` 不存在，Engine 库存未变。
Darwin 构建产物为 176,336,416 字节，SHA-256 为 `19a0e9bfca189c691984111b24631809e7cdbee6a3b60adbdcb1ed76f5ba65da`。
Darwin 的历史失败结果均保留：

- Q6 编译准备因 PATH 缺少 `cmake` 返回 ENOENT，原四项产品用例未执行。
- Q8 原四项为 2 PASS／2 FAIL，两个失败均为 `ocd did not stop`。
- Q9 清空继承的信号屏蔽集后，严格预检发现 11 个旧测试 socket，产品用例未执行。

原始记录分别为 `darwin-r9-single-admin-q6/result.json`、
`darwin-r9-single-admin-q8/result.json` 和 `darwin-r9-single-admin-q9/result.json`。
Q10 不改写这些失败记录。

2026-10-12，Q10 在同一真实普通账号下运行原四项用例。
入口清空授权进程继承的信号屏蔽集，UID 502 子进程读回为空。
11 个旧 socket 经身份和无人引用检查后删除，原严格预检通过。
原 driver、D10 正式包和 R9 pin 均保持原字节。
原始 harness 结果为 4 PASS／0 FAIL，耗时 270.28 秒；没有 ignored 或 filtered 用例。
正式 Gate 报告 SHA-256 为 `57e500ab7c098e61b2cfc4d2987e772e351ef4a6d06dc7a45631b10c385955fb`。
登录结果 SHA-256 为 `6a433f5ee804df9d8c125fbcb623b584b0ea34b7c6269f7b156db19a435a9e29`。
测试进程已回收；两组 ACL 回滚均 EXIT 0，任务账号 ACL 剩余数均为零。
最终归属检查通过，原 owner、mode 和已有 ACL 顺序保持不变。
Q10 外层仍为 failed：原账号删除命令返回 40；结果 SHA-256 为
`830ec9f70083bd145be5367bbaed2467c29dd4fb72d34bcce3a8c6aee1e811cc`。
独立账号清理 V1 因系统拒绝 `-keepHome` 返回 255。
V2 的系统删除命令 EXIT 0，已删除账号和 home；同进程账号查询仍读到旧记录，V2 结果保留 failed。
随后新进程确认账号名称、UID 和目录服务记录均不存在，只删除原身份匹配的 staging。
最终清理 EXIT 0；账号、home、staging 和账号进程均不存在。
清理记录为 `formal-package-preparation/q10-staging-only-final/result.json`，SHA-256 为
`786507ed0789e3e14c21509e0c800b61171f7f3b27b5e74d1da29cafa6dd3f47`。
独立清理没有重跑产品用例、修改 ACL 或改写 Q10、V1、V2 历史结果。

x64 定向 native 重建报告 SHA-256 为 `0563dd6e5bce5b1033f2628056fb688c9c404257ed83bd36e6f94f873aa2dd06`。
Linux x64 产物为 203,020,128 字节，SHA-256 为 `2d6e31ed0955f298854bc028374ccea6e4979779d65d3b122f68a65cb3ca253b`。
构建报告 SHA-256 为 `fef69f58f2195c2d87b0c31e2ddaa718210b2a01e868dde2ebd0da318669cd54`。
x64 编译准备报告 SHA-256 为 `0b42138f6567c2b810af32d108cdbb080a897d42013f06e1cc7ee125c6889145`。
2026-10-11，Linux x64 原四项 Gate 耗时 159.03 秒，EXIT 0，4 passed／0 ignored／0 filtered。
外层执行在用例结束后的 quota headroom 检查失败，EXIT 1；原失败记录保持不变。
随后独立只读复核 EXIT 0，确认身份、资源与清理；没有重跑四项产品用例。
最终快照的输入与 Engine 库存通过检查，实际 home 下 `.open-compute` 不存在。
原外层结果 SHA-256 为 `dd522255ca95c3e898da594e6602bc49d838ac4f5695759cb6f9d1076abcad4e`。
原 Gate 报告 SHA-256 为 `c2db4c0320f2c72938caa627c7ddca3cdd752934d7480fa29a4d607aa1db3f45`。
独立复核结果 SHA-256 为 `0c3c95a5f3d6fd96759d884bcd1e86c867ac6af08043f181fbeb0ae58c16602e`。
Linux x64 已完成本次正式包原四项范围；Rosetta 执行不证明原生 x64 CPU 资格。
这些结果不证明 Darwin 执行资格。

原失败报告和排错证据保留在 `.temp/`；当前矩阵不覆盖或改写历史失败结果。
本次三平台资格不包含 OCD 发布的五项安装／升级矩阵，也不证明 x64 原生 CPU 资格。

### 4.1 CF 兼容性复核

2026-10-11，复核采用上述 source `8ec1a158b0c386aea03bfd243b37fbf9daeb7f1d4e33a21627bb6d370e1b59e3`、
正式 R9、原版 `@cloudflare/containers@0.3.7` 和 `@cloudflare/workers-types@5.20260830.1`。
语义来源为 [官方 Container API](https://developers.cloudflare.com/containers/api/durable-object-container/)和
[官方 snapshots](https://developers.cloudflare.com/containers/guides/snapshots/)，并核对固定 upstream 的原生测试。
原始复核标识为 `r9-final-runtime-cf-review-8ec1/report.md`。

已测原生 `ctx.container`、HTTP、exec、streams、取消、Owner、idle、code-only、rollout 和 restart 路径为 aligned。
本次复核未发现这些路径中的新确定性 mismatch；结论只覆盖基础 13 family 与原版 SDK HTTP 场景。
该结论不声明管理 API 全量 parity，也不将 Provider 资源上限等同于 Cloudflare plan quota。

| 未完成范围                            | 状态与边界                                                   |
| ------------------------------------- | ------------------------------------------------------------ |
| 完整 64 个稳定成员资格                | blocked；未逐成员完成验收，已测路径不自动提升 inventory 状态 |
| TCP、WebSocket、HTTPS 拦截            | unverified；HTTP Fetcher 和 exec 证明不能替代这些协议        |
| Snapshot、startup snapshot options    | blocked；稳定 API 仍为目标，不改标 experimental              |
| 原生 inactivity timeout、主进程日志   | blocked；SDK alarm 和 logs 请求意图不证明对应能力生效        |
| Stock／hosted Cloudflare differential | unverified；本轮没有该执行对照，不把证据缺口写成 mismatch    |

已测 source `266f5a459610acde26156a9a7c386e42097b9f9342d7f78d1eb6b5a95cabc6b6` 的 CF 复核复用上述结论。
16 个受审源文件的 SHA-256 与原复核逐字节一致。
复用标识为 `r9-current-cf-review-reuse-266f.json`，SHA-256 为
`3d1772154e7db953ca237914d64e4ccaaf5ca4f28c8323434d5ea46384c0d34c`。
该复用不扩展协议、成员或 hosted Cloudflare 对照范围。
最终 workspace Gate 保持 failed；Linux ARM 与 Linux x64 已完成本次 package 范围。
Darwin Q10 原四项产品用例 4/4 PASS；两组 ACL 已恢复，临时账号与目录已删除。
本页不从已测子集外推全平台兼容。

## 5. cf 配置、Build Output 与 upload contract

### 5.1 标准配置

```ts
import {
  bindings,
  defineConfig,
  defineContainer,
  defineWorker,
  exports,
} from "cf/config";

const apiContainer = defineContainer({
  name: "api-container",
  image: { dockerfile: "./Dockerfile" },
  maxInstances: 4,
  instanceType: "lite",
  rollout: { stepPercentage: [10, 100] },
});

const worker = defineWorker({
  name: "container-app",
  entrypoint: "./src/index.ts",
  compatibilityDate: "2026-09-07",
  workersDev: false,
  exports: {
    ApiContainer: exports.durableObject({
      storage: "sqlite",
      container: apiContainer,
    }),
  },
  env: {
    API_CONTAINER: bindings.durableObject({
      worker: "container-app",
      exportName: "ApiContainer",
    }),
  },
});

export default defineConfig({ worker, containers: [apiContainer] });
```

此例保留 CT0 当时固定 `cf/config` 的声明关系，范围为基础 13 family。
当时的完整 64 个稳定成员均为 `blocked`；当前成员范围与结果见本页的 R22 记录。
mode 求值、DO export lifecycle 与 Build Output
由 cf 和官方构建链拥有；`ocd` 验证实际上传合同，不读取 TypeScript 项目文件或沿用旧 migrations/tag 配置。

字段、默认值、互斥关系、mode 求值和错误文本以固定 cf/config schema、deploy producer 与 tests 为准。
配置使用 camelCase；v4 multipart/API wire 按各自 schema 冻结，不把配置字段拼写直接当成 wire。特别要求：

- `exports` 中的 Container DO 应是同一 Worker 的 SQLite-backed export，env binding 指向该 export；
  wire 中的 `class_name`/export 关联由 CT0 fixture 固定，不能指向另一个 script；
- application name、class name、binding name 与 account/script 唯一性逐项按 fixture 校验；
- `instanceType`（wire `instance_type`）的 named/custom 两种形状不能混用，resource 值在 admission 时归一化一次；
- `maxInstances`、`rollout` steps/grace、scheduling/affinity 中未实现的字段明确拒绝，不能静默忽略；
- `image` 是开发/部署输入，不作为 tenant runtime 可修改字段；
- tenant config 不接受 engine socket、Docker host、privileges、devices、mounts、network mode 或 provider auth；
- P23 只实现当前 cf/Build Output 的 Day1 schema；旧项目由用户显式 `cf migrate <exact-file> --bundler vite`，
  不在平台内读取旧配置或增加兼容路径。
