# workerd 原生运行时方案

当前待实施：[W4：workerd fork 收薄](w4-thin-fork.md)。该方案复核 W1/W2/W3、R3 与 P21 候选的全部差异，区分必要原生接口和可替代的平台策略；W4 尚未实施；当前 R24 formal pin 记录见下文。下文已有资格记录保留其原始输入与适用范围。

W1 的逐 surface 复核见[兼容审查记录](../implemented/w1-worker-loader-compatibility-review.md)。

历史状态：**W1/W2/W3 已在各自原始 pin 完成产品 qualification**。历史 R8 的四平台原生资格和资产校验已通过；R9 的 CI 与资产校验曾通过；当前 R24 的原生资格与资产校验已通过，完整产品验收仍待完成。W2 的 limits/v4 配置、Dynamic Worker ceiling、原生执行、公开错误、
isolate 摘除与 supervisor 自恢复已在同一 Day1 路径完成资格化。四个平台的源码 revision、二进制与 digest
由 formal lock 固定。2026-09-05 用户确认接受维护自己的 workerd fork 并重新编译。
W1/W2/W3 不再以“等待上游合并后才能开发”为实施前提；public Loader 已接入原生 fork。W2 同时交付原生
ResourceLimits、超限 isolate 摘除，以及 generation-fenced supervisor 功能性探活与自动恢复。

2026-09-06 调整交付顺序：先完成 W1 原生 Loader，再实现 W2 Standard limits。W1 的范围不包含默认
CPU/内存/subrequest enforcement 或 custom limits；显式 limits 应由原生 API 拒绝，不能静默忽略。
W1 已完成 namespace/权限隔离、结构大小限制、in-flight 计数、缓存与生命周期及正式 pin 验收。
W2 通过请求限额、isolate 摘除和执行器自恢复三层机制消除失控代码影响同进程邻居的已知故障，并完成
当时的固定部署客户端、v4 Settings、Dynamic Worker ceiling、官方错误分类和完整产品验收。
当前应用侧配置与部署由 [P20](../implemented/p20-cf-cli-migration.md) 的 cf/config 和 Build Output 拥有；
旧 W2 客户端输入仅是历史证据，不能替代当前 cf 与正式 pin 的资格检查。

## 源码与运行时基线

**后续 workerd 修改统一基于仓库中的 [`third_party/workerd/`](../../third_party/workerd/)。**
该目录是用户 fork 的 Git submodule，由根目录 [`.gitmodules`](../../.gitmodules) 登记远端，父仓库 gitlink 固定源码提交。
不要另建一份 workerd 实现、复制到其他目录，
或为了匹配旧的测试二进制而重置这个 checkout。

2026-10-30（UTC），正式 binary source 与源码 checkout 均固定 R24 `9de631551791218aed0c4cd570163cece3c00590`。
R24 修复 KJ 对 HTTP/1.0 响应的默认连接关闭语义。
没有显式 `Connection: keep-alive` 时，客户端不复用该连接。
HTTP/1.1 和显式 keep-alive 的既有路径保留。
固定 KJ 源码的 owning 回归先失败，修补后 140 项 HTTP 测试全部通过。
四平台构建及其必需原生资格已通过。
正式资产、签名、协议和 catalog 校验已通过。
这些结果不证明 Open Compute 0.4.0 的 P23、coverage、Final 或 package 验收通过。

| 项目                                                              | 2026-10-30 核对结果                                                                                                         |
| ----------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| [workerd 上游 issue / PR 核验](../references/workerd-upstream.md) | 已合并能力、standalone 缺口、补丁范围与升级回归重点                                                                         |
| fork origin                                                       | <https://github.com/elliothux/workerd>                                                                                      |
| upstream 项目                                                     | <https://github.com/cloudflare/workerd>                                                                                     |
| fork checkout HEAD                                                | `9de631551791218aed0c4cd570163cece3c00590`                                                                                  |
| upstream base                                                     | `d99bc6b777e35d72d71c2f1fe2fd1db53284528a`（上游 `main`，日期为 2026-09-30）                                                |
| HEAD 提交说明                                                     | Honor HTTP/1.0 connection persistence in container tunnels                                                                  |
| fork working tree                                                 | HEAD 相对上述上游基线领先 54 个提交；工作区干净                                                                             |
| formal binary source tree                                         | `ce9a3090aeb254113c97a69795f69d5a2b01b370bac7ac066fec10ff594c44e6`（`sha256(git ls-tree -r <revision>)`，不是 Git tree ID） |
| W3 runtime                                                        | `workerd 2026-09-30`；版本文本未变，摘要和 pin 已变                                                                         |
| open-compute 当前正式 pin                                         | `v1.20260930.0-open-compute-r24.9de631551` / `9de631551791218aed0c4cd570163cece3c00590`                                     |
| 正式 pin authority                                                | [`packages/runtime/workerd.lock.json`](../../packages/runtime/workerd.lock.json)                                            |

R24 保留 R23 的 Tokio readiness 修补、私有 Container 协议 v4 和原有 compatibility catalog。
四平台原生资格与 Release 资产校验已通过；新 pin 的父仓库产品、coverage、Final 和三平台 package 仍待验收。
fork 的 `--version` 不能替代源码身份与二进制摘要；各项历史资格结果仅适用于其原始输入。
正式切换应完成构建、固定来源和协议、更新所有 pin 消费者及验证。
formal lock 还固定 system Worker date/flags、binary maximum 和 exact binary 生成的 compatibility catalog digest；四目标
catalog 应逐字节一致。tenant Version 的 date/flags 不使用 system 值，也不由 open-compute allowlist 判断。
初始迁移保留 `main` 分支与 origin。W1 已完成原生本地提交和三个正式平台优化构建，正式 fork pin 已通过本机完整产品验收；macOS Intel 仅保留手动编译输入，
详见 [W1 实施记录](../implemented/w1-dynamic-workers-worker-loader.md)。

## Submodule 工作流

在 open-compute 根目录初始化已有 checkout，或首次克隆时一并获取固定的源码提交：

```sh
git submodule update --init -- third_party/workerd
# 首次克隆可使用：
git clone --recurse-submodules https://github.com/elliothux/open-compute.git
```

初始化可能访问网络；本次直接移动已有 checkout 并登记，没有重新克隆或下载。
`git submodule update` 使用父仓库记录的提交，不使用 `--remote` 自动追踪 fork 最新版本；
更新前先检查并保存子仓库的本地改动。初始化通常得到 detached HEAD，开发前在子仓库创建工作分支。

后续先在 `third_party/workerd/` 内提交源码，再在父仓库通过 `git add third_party/workerd` 记录新的 gitlink，
与本次修改的平台代码和文档一起提交。父仓库只保存子仓库提交 ID，不会保存未提交的 fork 文件改动。
共享父仓库更新前应确保被引用的提交已在 fork 远端可获取；push 仍需相应外部写入授权。
迁移后子仓库 Git 元数据由父仓库 `.git/modules/third_party/workerd/` 管理，旧源码目录不保留副本或别名。

构建平台从 formal lock 指定的 fork GitHub Release 显式下载并验证目标 archive；根 build 不隐式联网。
源码辅助的 conformance 校验在子仓库初始化后，
通过 `git show <正式 pin revision>:<path>` 读取固定版本，不把开发 checkout 当作正式运行时或 npm types 基线；
缺少所需 Git 对象时校验失败，不自动下载或改用 HEAD。后续源码与 pin 升级需保持这些对象可获取。

先读取 fork 的
[开发规则](../../third_party/workerd/AGENTS.md)和修改组件已有的规则，保持其 Bazel/C++/测试布局。

P21 历史资格的运行时为正式 R4 pin，包含部署时原生 preparation、语言无关 binding construction/policy、Python Queue 单 batch 参数与 scheduled class handler 参数。内部 waitUntil observer 通过既有 IoContext/AsyncContextFrame 登记原 promise；原生 DO state 与 handler 继续使用官方 Python helper，不修改 SDK 文件。普通 Main、三框架、Service、Queue、DO、Workflow、Runtime 及现有产品 Gate 已通过完整 workspace coverage 和最终单轮 Gate，输入与已知限制见 [P21 Python Workers 实现](../implemented/p21-python-workers.md)。历史候选 probe 和 R4 产品结果不能替代当前 formal pin 的验收。P21 当次发布复用了已有四平台产物。

## R24 发布与输入校验（2026-10-30）

[R24 四平台 CI](https://github.com/elliothux/workerd/actions/runs/41025742244) 的四个 build 和 publish 均成功。
每个平台通过八个 Container／KJ HTTP owning target、52 个普通 Rust target 和十一项 Root 回归。
八个 reactor target 与 Rust lint 也已通过。
两个 sanitizer 专用 Rust target 在 release 配置中不兼容，保留跳过状态，不计入通过数。
五个真实 `CheckRun` 均为 `COMPLETED`／`SUCCESS`。

R24 给固定的 KJ 源码增加两份 patch：HTTP/1.0 默认关闭修补及其回归。
回归在修补前得到 139 通过／1 失败，修补后得到 140 通过／0 失败。
Container owning 测试覆盖 EOF 到达前的连接复用和显式 keep-alive。
原生资格不证明 Open Compute 中原始 P23 失败已经修复。

不可变 Release 的十项资产已通过签名、GitHub digest 和 `SHA256SUMS` 校验。
四目标 archive／binary、架构、build-inputs、协议 v4 与 catalog 均已核实。
校验只在内存中下载和解压，没有在高水位宿主写入原生 archive 或 executable。
资产证据为 `.temp/release-0.4.0-actual/workerd-release-r23/formal-current-release-40902084555/native-r24-verified-assets.json`。
证据 SHA-256 为 `f44d73a7dc087a3e09816e164d2bcb1afd9700363681a7f06769b14022c3de13`。
正式 lock 与父仓库 gitlink 同步到 R24；当前产品、coverage、Final 和发行包仍待验收。

## R23 发布与输入校验（2026-10-26）

[R23 原始构建](https://github.com/elliothux/workerd/actions/runs/40199209196)有三个平台成功。
原始 Darwin x64 job 在 Rust lint 的配置分析阶段失败，因为通用 lint 配置还加载了不支持该目标的 C++ 检查。
[Darwin x64 恢复构建](https://github.com/elliothux/workerd/actions/runs/40216315540)仅重建缺少的平台，并通过全部必需检查。
四个平台均通过完整原生 Rust 套件、八个 reactor 目标、Rust lint、七个 Container 目标和十一项 Root 回归。
原始失败和恢复构建中的发布失败保留原状态。

[最终发布任务](https://github.com/elliothux/workerd/actions/runs/40269642389)复用四份精确产物并成功发布。
该任务跳过 build；发布重试没有触发编译。
校验器核对五个成功的真实 `CheckRun`，分别来自三个原始 build、一个恢复 build 和一个最终 publish。
必需资格步骤全部成功；可选跳过步骤保留原始 `SKIPPED` 状态。

不可变 Release 的十项资产已通过签名、GitHub digest 和 `SHA256SUMS` 校验。
四目标 archive／binary、架构、build-inputs、协议 v4 与 catalog 均已核实。
资产证据为 `.temp/release-0.4.0-actual/workerd-release-r23/publish-only-collection/verified-assets.json`。
证据 SHA-256 为 `af01d3d10f503c83e0dfc526f9fd1d6a3f81d6e3f24ccee812da2b126b41e14e`。
正式 lock 已更新；binary source 与发布工作流 revision 分别记录。
原生资格不替代父仓库真实产品、coverage、最终 Gate 或三平台发行包资格。

## R22 发布与输入校验（2026-10-22）

[R22 四平台 CI](https://github.com/elliothux/workerd/actions/runs/39721082079)的 4 个 build 和 publish 均成功。
每个平台通过 7 个原生 owning target 和 11 项 Root 回归。
GitHub GraphQL 的 5 个 `CheckRun` 均为 `COMPLETED`／`SUCCESS`。
4 个 build 各返回完整 14 项步骤，publish 返回完整 7 项步骤。
必需资格步骤全部成功；可选跳过步骤保留原始 `SKIPPED` 状态。

不可变 Release 的 10 项资产已通过签名、GitHub digest 和 `SHA256SUMS` 校验。
四目标 archive／binary、架构、build-inputs、协议 v4 与 catalog 均已核实。
资产证据为 `.temp/p23-full64/workerd-release-r22/verified-assets.json`，SHA-256 为
`f00e7f8aeb60afd56b7ed129f8094a56b43fcc24a5864aef47772fae16ccfbc8`。
正式 lock 和父仓库 gitlink 已更新。
原生资格不证明当前公开目录恢复、完整 65 项、coverage、最终 Gate 或三主机发行包通过。

## R21 发布与输入校验（2026-10-17）

[R21 四平台 CI](https://github.com/elliothux/workerd/actions/runs/39042535973)的 4 个 build 和 publish 均成功。
每个平台通过 7 个原生 owning target 和 11 项 Root 回归。
本地 GC 回归先失败，修复后通过；输出完成后的 handle 释放断言也通过。
GitHub GraphQL 的 5 个 `CheckRun` 均为 `COMPLETED`／`SUCCESS`。
4 个 build 各返回完整 14 项步骤，publish 返回完整 7 项步骤。
必需资格步骤全部成功；两个 Linux job 的可选 macOS linker 步骤为 `SKIPPED`。
校验器保留原始 REST 与 GraphQL 响应，不把跳过步骤改为成功。

不可变 Release 的 10 项资产已通过签名、GitHub digest 和 `SHA256SUMS` 校验。
四目标 archive／binary、架构、build-inputs、协议 v3 与 catalog 均已核实。
资产证据为 `.temp/p23-full64/workerd-release-r21/verified-assets.json`，SHA-256 为
`7fba9357f54fe74b4aa217e90f44c59c8f0e7190d5d4908d2f15422b5f6d0e0d`。
正式 lock 和父仓库 gitlink 已更新；新 pin 的真实产品验收仍待完成。
历史 coverage、Final 与三主机 package 不覆盖这次原生修改。

## R20 发布与输入校验（2026-10-15）

[R20 四平台 CI](https://github.com/elliothux/workerd/actions/runs/38644356217)的 4 个 build 和 publish 均成功。
每个平台通过 7 个原生 owning target 和 11 项 Root 回归。
新增回归覆盖异步 lease policy I/O 后的 snapshot capture。
GitHub GraphQL 的 5 个 `CheckRun` 均为 `COMPLETED`／`SUCCESS`。
4 个 build 各返回完整 14 项步骤，publish 返回完整 7 项步骤。
必需资格步骤全部成功；两个 Linux job 的可选 macOS linker 步骤为 `SKIPPED`。
校验器保留原始 REST 与 GraphQL 响应，不把跳过步骤改为成功。

不可变 Release 的 10 项资产已通过签名、GitHub digest 和 `SHA256SUMS` 校验。
四目标 archive／binary、架构、build-inputs、协议 v3 与 catalog 均已核实。
资产证据为 `.temp/p23-full64/workerd-release-r20/verified-assets.json`，SHA-256 为
`a0a886bde110bb0ca5da9cd6736b9e681d67fd414772ee012860c7b0031c4e5c`。
正式 lock 和父仓库 gitlink 已更新；完整产品、coverage、Final 与三主机 package 仍待完成。

## R19 发布与输入校验（2026-10-13）

R19 的正式 revision 为 `d9b8d6ce40e0bbd3b9233acad4d1516cbab28e3b`。
tag 为 `v1.20260930.0-open-compute-r19.d9b8d6ce4`。
[R19 四平台 CI](https://github.com/elliothux/workerd/actions/runs/38463100288) 的 4 个 build job 和 publish 均成功。
四个平台均通过以下 7 个 owning target：

- `//src/workerd/server:host-container-test@`
- `//src/workerd/server:container-client-test@`
- `//src/workerd/api:container-test@`
- `//src/workerd/api/tests:worker-loader-host-container-test@`
- `//src/workerd/api:sockets-test@`
- `//src/workerd/api/tests:socket-close-gc-test@`
- `//src/workerd/api/tests:outbound-interceptor-socket-test@`

每个平台另通过 10 项 Owner、named images、callback 与 inactivity 回归。
GitHub GraphQL 的 5 个 `CheckRun` 均为 `COMPLETED`／`SUCCESS`。
4 个 build 各返回完整 14 项步骤，publish 返回完整 7 项步骤。
校验器保留 REST 原始记录，使用 GraphQL 验证步骤状态。

R19 保持 private Container protocol v3。
不可变 Release 的 10 项资产已通过签名、GitHub digest 和 `SHA256SUMS` 校验。
四目标 archive／binary、架构、build-inputs 与 catalog 均已核实。
macOS ARM64 的版本、private protocol 与 exact catalog 已通过本地校验。
资产证据为 `.temp/p23-full64/workerd-release-r19/verified-assets.json`，SHA-256 为
`3e399e71a402d46e961ec83f15b84fe5eb640de41f9dbbf22dc3c4ab65536816`。
当前正式 pin 的四目标 URL 与 archive／binary 摘要见 [workerd 参考](../references/workerd.md#单独下载-workerd)。
R19 的真实产品、full65、coverage、Final 与三主机 package 验收仍待完成。
原生 CI 与资产校验不构成这些产品验收。

### R18 历史候选

[R18 CI](https://github.com/elliothux/workerd/actions/runs/38447450369) 的 Linux ARM64 build 成功。
原生资格中的首个 HTTP owning 用例违反 `StringPtr` 的 NUL 终止符契约。
同一目标的其他 55 项用例通过，另外 6 个目标通过。
Root 回归和后续记录步骤被跳过；R18 未通过全部正式资格。
R19 仅把这条断言改为精确字节数组比较。
R18 与 R19 的生产源码、workflow 和测试预算相同；历史失败证据保留。

## R17 发布与输入校验（2026-10-13）

当时正式 revision 为 `eca7215c8d5802e09746d424c8ce763b3763a82d`，tag 为 `v1.20260930.0-open-compute-r17.eca7215c8`。
[R17 四平台 CI](https://github.com/elliothux/workerd/actions/runs/38350702393) 的四个 build job、原生资格与 publish 均成功。
各平台均通过四个 owning target 和十项 Owner、named images、callback 与 inactivity 回归。
R16 保留 private tenant 的 Root idle policy，避免平台 repair RPC 重建 Actor 后重置 timeout。
R17 相对 R16 只修改 `.bazelrc` 的 Darwin x64 跨架构 medium 测试预算。
该预算从 30 秒设为 60 秒；产品超时、测试断言和 Root 回归清单未变。
R16 两次 Darwin x64 资格超时的日志保留；冷启动或挂起原因仍未确认。

不可变 Release 的 10 项资产已通过签名、GitHub digest 和 `SHA256SUMS` 校验。
四目标 archive／binary、架构、build-inputs 与 catalog 均已核实。
macOS ARM64 的版本、private protocol v3 与 exact catalog 输出已通过本地校验。
证据为 `.temp/p23-full64/workerd-release-r17/verified-assets.json`，SHA-256 为
`e57d6a897e60954ae53614f4095b24ec00b8c238643be26d401c341e8743a12d`。
R17 的四目标下载 URL 与摘要保留在上述资产证据。
R17 当时未完成真实产品、full65、coverage、Final 与三主机 package 验收。
原生 CI 与资产验证不构成这些产品验收。

## P23 原生 R12 历史候选（2026-10-13）

R12 源码已提交并推送为 `617f19427ad3e9c43d6a66d910090d1d26a616ed`。
候选 tag 为 `v1.20260930.0-open-compute-r12.617f19427`。
[R12 四平台 CI](https://github.com/elliothux/workerd/actions/runs/38275130171) 的 Linux ARM64 与 Darwin x64 原生 client 测试超时。
R12 已由 R15 替代，run 已取消；失败日志保留。
当时 formal lock 固定 R9；R12 未通过正式资产校验或完整产品验收。

R12 修复内部自动 monitor 延迟 inactivity stop 的问题。
新原生回归先在 R11 源码失败；修复后 9 项 Owner 回归通过。
回归包含无显式 monitor 的 1 秒 subject 和 120 秒 control。
模拟静默 90 秒后，subject 已清理，control 仍运行。
显式 monitor、`waitUntil()` 和活跃 egress 的原有回归继续通过。
R11 的四平台 run 已取消；本项目未下载或采用 R11 资产。

private exec 使用独立控制流记录实际子进程身份。
控制流依次返回启动、退出和输出完成证明。
原生 client 在第二帧完成 `exitCode`，在第三帧与 EOF 后正常结束输出。
缺失证明、截断和异常输出关闭会拒绝 `output()`；已确认的子进程退出码保持可读。
exec 不依赖应用镜像中的 shell 或可写目录。

本地原生 client 专项目标为 17/17 PASS，耗时 13.3 秒，保留原 15 秒预算。
API 专项用例覆盖正常输出、stdout 中断和 stderr 中断，并核对退出码。
同一 helper 源码的 36 个 Go 行为用例通过，另有 2 个辅助测试入口。
Go 语句覆盖率为 530/584，即 90.7534%；此数值不代表 Rust 行覆盖率。
上述检查不替代真实 Engine、正式 pin 或完整 workspace Gate。

### Named images R14 历史候选

R14 源码为 `79a3db287945f80b6e1235e21c3ac7f12029bf64`。
候选 tag 为 `v1.20260930.0-open-compute-r14.79a3db287`。
[R14 四平台 CI](https://github.com/elliothux/workerd/actions/runs/38281161897) 已由 R15 替代。
R14 的 Linux x64 build 成功，但同一 native client 用例在 15.3 秒后超时。
R14 仍使用旧测试替身，run 已取消；具体阻塞阶段尚未确定。
当时 formal lock 为 R9；R12 的 v2 资产不能用于 v3 验收。

可信 `getContainer()` 接收 named images 参数。
grant 保存 map 的独立副本；动态 Actor 使用该 map 构造现有 `Container.images` getter。
此改动不重写公开 Container API，也不向 Docker Broker 增加别名逻辑。
新接口要求 private Container protocol v3；v2 会忽略新增参数，不能通过新协议检查。
本地 named images Owner 回归和受影响的 API owning target 均成功退出。
回归核对 map 副本、不同 Owner、class 不匹配、Actor 重建和缺失参数拒绝。
TS strict 检查与 19 项 attachment/snapshot 测试通过。
这些检查不证明正式资产或完整产品已经通过。

R13 workflow 仍检查 v2，因此 run `38280974901` 已取消。
R14 单独提交修正 workflow 的协议检查，不重写 R13 提交或 tag。
本项目未下载或采用 R13 资产。
R14 未作为最终资产输入。

### R15 历史发布与输入校验（2026-10-13）

当时正式 revision 为 `a8d2411621ac0d8f4f4fcf92d2a5a731e84de3e9`，tag 为 `v1.20260930.0-open-compute-r15.a8d241162`。
[R15 四平台 CI](https://github.com/elliothux/workerd/actions/runs/38289685666) 的四个 build job、原生资格与 publish 均成功。
R15 相对 R14 只修改 `container-client-test.c++`，不修改生产源码、workflow 或测试超时。
callback 测试替身最多保留 16 条未用连接，并等待连接释放或对端关闭。
本地 client 目标在原 15 秒预算内通过，结果为 17/17 PASS，耗时 2.4 秒。
四平台各通过四个 owning target 和十项维护的 Owner、named images、callback 与 inactivity 回归。

attempt 1 的 Linux ARM64 runner 失联，日志未证明 OOM 或编译失败。
attempt 2 的四平台资格成功，但 publish 返回 HTTP 403。
目标提交相对默认分支含 workflow 修改，触发 GitHub 的 workflow 写权限规则。
默认分支正常快进到同一 R15 提交后，仅 publish 在 attempt 3 重跑并成功；四平台资格产物未重建。

不可变 Release 的 10 项资产已通过签名、GitHub digest 和 `SHA256SUMS` 校验。
四目标 archive／binary、架构与 build-inputs 已核实；catalog SHA-256 为
`a6a00dc87f3e246dbc29ee32a5ab06db2867ad165798b3f71b131cfc5c1281b4`，与正式 lock 一致。
macOS ARM64 的版本、private protocol v3 与 exact catalog 校验已通过。
证据为 `.temp/p23-full64/workerd-release-r15/verified-assets.json`，SHA-256 为
`8ed6c7e8bce3246b08dcd8823a990c1e81ebfe53edb062b09349c198a7dee5c8`。
当时正式 lock 更新为 R15。R15 的原生资格与资产记录不能替代后续 pin 的产品验收。

## P23 原生 R9 发布与输入校验（2026-10-11）

R9 源码已冻结为 `a8e3d0bd40dff4c3457d0eb75acd000601f4b7b2`，tag 为 `v1.20260930.0-open-compute-r9.a8e3d0bd4`。
[R9 四平台手动 CI](https://github.com/elliothux/workerd/actions/runs/38093474159) 的四个 build job、原生资格和 publish job 均已通过。
当时 formal lock 更新为 R9。不可变 Release 的 10 项资产已通过签名、GitHub digest 和 `SHA256SUMS` 校验。
四目标 archive／binary 摘要、架构和 build-inputs 已核实，catalog 与 formal lock 一致。
归档采用 GNU `gzip -9n`，mtime 为 0 且无文件名；macOS ARM64 的版本、私有协议和 exact catalog 校验已通过。

最终 candidate 的四项本地 native 回归均已通过：
cold warm-start、old execution retirement、idle owner 和 shared mapping retirement。
standalone client shutdown 撤销其独占映射；private client 只取消自身的 execution pipe。
Owner／grant 撤销清空共享映射，旧 client 退出不会删除当前 Owner 的回调。
host 在新的 private execution 启动成功后重建 tenant Actor，保留 Durable Object 的 SQLite storage。
失败或 already-running 路径不执行该重建。
固定 SDK `@cloudflare/containers@0.3.7` 按 `container.running` 注册拦截；该 package 未修改。
原生 owning batch 已通过；最终 batch 中未变产物复用先前结果，loader 实际执行并通过。

这些结果分别证明本地 native 行为和正式发布的原生资格。
正式 R9 的原版 cf＋SDK exact3 已 verified，范围和输入见 [P23 结果](../implemented/p23-cloudflare-containers.md#4-验证记录与待完成检查)。
2026-10-11，workspace coverage 按未变生产与配置字节获准复用，行覆盖率为 90.634663%。
原始 63 目标批次仍为失败；两个失败目标已未插桩复验通过，证据见上述 P23 结果。
最终 workspace Gate 为 failed：63 个目标中 62 个通过。
失败的 P21 原目标随后 scoped 3/3 PASS，首次失败原因仍未确认；原 Final 结果保持失败。
Linux ARM 与 Linux x64 已完成本次 package 范围。
2026-10-12，Darwin Q10 原四项 package 用例 4/4 PASS，耗时 270.28 秒。
测试进程已回收；两组 ACL 已恢复；临时账号、home 和 staging 已删除，账号进程为零。
Darwin package 构建 EXIT 0 不构成平台资格；原始记录为 `d10/result.json`。
历史 Q6 中，`xberg-tesseract` 因测试账号 PATH 缺少 `cmake` 所在目录而返回 ENOENT。
Q6 最终为 `ACCOUNT_CLEANUP_INCOMPLETE`；账号删除命令返回 255，产品用例未执行。

| R9 三平台 package | 当时状态                                                                           | 实际证据                                                                                                        |
| ----------------- | ---------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| Darwin ARM        | `d10` 构建 EXIT 0；Q10 原四项 4/4 PASS；两组 ACL 已恢复，临时账号与目录已删除      | 实际用例结果与清理状态见 P23 结果页                                                                             |
| Linux ARM         | 当前范围已完成：canonical 构建与独立身份复核通过；8 GiB 编译准备 EXIT 0            | 4 GiB／2 CPU package Gate 4/4 PASS，Dashboard smoke 2/2 PASS；不含 installer，证据见 P23 结果页                 |
| Linux x64         | 本次原四项范围完成：正式包构建与编译准备 EXIT 0；4 GiB／2 CPU 原四项 Gate 4/4 PASS | 独立身份、资源与清理复核 EXIT 0；原外层失败保留，未重跑产品；Rosetta 不证明原生 x64 CPU 资格，证据见 P23 结果页 |

三平台检查不包含 OCD 发布的五项安装／升级矩阵。
各项实际输入、退出结果与证据摘要统一由 P23 结果页记录。
当时 64 stable 成员为 `blocked`；snapshot、原生 inactivity timeout 与主进程日志采集不在该次验收范围。
以下 R8 与更早记录保留其原始输入和适用范围。

## P23 原生 R8 发布与输入校验（2026-10-10）

R8 源码已冻结为 `c189d6029707b38b99a716c4beb5adeef0ab866b`。
[R8 四平台手动 CI](https://github.com/elliothux/workerd/actions/runs/38055450809) 的四个 build job 和 publish job 均已通过。
构建 tag 为 `v1.20260930.0-open-compute-r8.c189d6029`。
正式 lock 已更新为 R8。Release 为不可变，10 项资产的签名、GitHub digest 和 `SHA256SUMS` 已校验。
四目标 archive／binary digest、架构和 build-inputs 均已核实。归档采用 GNU `gzip -9n`，mtime 为 0 且无文件名。
下载的 macOS ARM64 binary 已通过版本、私有 Container 协议和 exact catalog 输出校验。
四个平台的 native Container callbacks、attachment 和 Root 生命周期资格均通过。
这些结果不替代父仓库 coverage、完整产品 Gate 或支持平台的包验证。

本地原生回归已通过启动前注册、动态 `ctx.exports` outbound 回调和旧 facet 撤销。
Container 只在 owner 生命周期内保留原生回调 capability。
通用动态 Worker 的 RPC token 转移限制未变。
Root、tenant 和 SQLite 回归已验证实际反向 HTTP Response 正文。
该回归还验证 12 秒空闲、code-only facet 更新和停止后私有再次启动。
R8 回归验证 owner 退休后应用与 sidecar 保持运行。
重建 owner 使用同一租约，启动与删除计数不变。
再次空闲 12 秒后，SQLite 计数继续增长。
Engine 端使用明确的协议 fixture；该结果不能替代真实 Docker 产品 Gate。

R5 复现 Root 空闲后丢失 Container 状态，因此 R5 构建已取消。
R6 缺少 SDK 首次启动前的 outbound 注册和动态回调 capability，因此 R6 构建也已取消。
R7 复现 owner 退休时删除持久租约的应用与 sidecar，因此 R7 构建已取消。
三次构建的 tag、源码历史和取消证据保留。R8 使用新提交与新 tag。
P23 正式 12 秒空闲检查、完整产品 Gate 和稳定 64 项仍未完成验收。
快照 API 仍按当前交付范围阻断；原生回归不会提升公开能力状态。

## R4 发布与输入校验（2026-10-04）

用户授权后，候选 `e98a3e843` 已推送并完成四平台优化构建。原 publish job 对 artifact 的 binary 路径假设错误；
源码修正为 `fdcb8c97f`，发布使用原 run 的成功产物恢复，release tag 仍指向实际 binary 的构建 revision。
没有把 workflow 修正提交标为二进制来源。10 项 release asset 的 GitHub digest、`SHA256SUMS`、四目标
archive/binary digest、目标架构、逐目标 build-inputs 与逐字节一致 catalog 已校验；macOS ARM64 输入已
通过 canonical `prepare-workerd.ts --archive` 准备。发布与下载证据保存在 `.temp/p21-workerd-release-r4/`。
workerd fork 的其他 22 个 GitHub workflow 已在 repository settings 禁用，只保留
`Open Compute binaries` 的 `workflow_dispatch` 手动发布入口；当前没有待取消的其他 run。
上游 workflow 源码与历史 run 保留，不执行其自动/其他手动 CI。

正式 lock 唯一 CLI 字段为 `cfVersion`，固定 cf `1.0.0-beta.12`；Vite plugin 为 `2.0.0-beta.sha-52b0dc0e9`，
匹配 workers-sdk revision `52b0dc0e99b5bbdb86c06ed53fb703048efc36cf`。该 SDK revision 是开发工具来源，
不替代 Python `workers-runtime-sdk` 版本。新正式 binary 的 Runtime/PythonPreparation 两 profile 真实编译和 SDK scheduled prepare/restore 组件通过。
此前 R3 和开发候选的报告作为历史证据保留；P21 各 ordinary case 已有正式 pin 的 development pass，完整 workspace coverage 与最终单轮 Gate 仍需通过。

## 文档

本目录按交付顺序编号：W1 为 Dynamic Worker Loader，W2 为 Standard limits。历史平台 P9/P10 记录中的编号
保留为当时的阶段名称，当前方案与链接统一使用本目录名称。

| 文档                                                                                     | 职责                                                                                              |
| ---------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| [W1 原生 Loader 方案](../implemented/w1-native-limits-loader.md)                         | 接口复用、capability 边界、fork 维护与完成结果                                                    |
| [W1 Dynamic Workers / Worker Loader](../implemented/w1-dynamic-workers-worker-loader.md) | public binding、原生 JS API、namespace、动态 Worker 与产品验收合同                                |
| [W2 Workers Standard limits](../implemented/w2-standard-limits.md)                       | Standard limits、公开配置/API、可观察行为和自恢复的完成合同                                       |
| [W3 用户可扩展原生 Binding](../implemented/w3-user-extensible-native-bindings.md)        | `ocd` 名字/path 注册、Service binding `services + props`、Provider 直连与生命周期；不管理扩展版本 |
| [此前 stock workerd 可行性复核](../implemented/p10-worker-loader-feasibility.md)         | 保留旧 pin 的 No-Go 实测；不作为当前 fork 路线的禁令或完成证据                                    |

本目录保存尚未完成的 workerd 设计与 fork 维护入口。源码基线、fork 交付方式和内部实现分工以本目录为准；
W1/W2/W3 已完成合同不会因允许 fork 而降低；W3 不把用户 Provider 解释为第二个 workerd 或第二套 authority。

返回[文档索引](../README.md)。
