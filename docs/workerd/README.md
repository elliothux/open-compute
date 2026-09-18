# workerd 原生运行时方案

W1 的逐 surface 复核见[兼容审查记录](../implemented/w1-worker-loader-compatibility-review.md)。

状态：**W1/W2/W3 与正式 pin 均已完成产品 qualification**。W2 的 Wrangler/v4 配置、Dynamic Worker ceiling、原生执行、公开错误、
isolate 摘除与 supervisor 自恢复已在同一 Day1 路径完成资格化。四个平台的源码 revision、二进制与 digest
由 formal lock 固定。2026-09-05 用户确认接受维护自己的 workerd fork 并重新编译。
W1/W2/W3 不再以“等待上游合并后才能开发”为实施前提；public Loader 已接入原生 fork。W2 同时交付原生
ResourceLimits、超限 isolate 摘除，以及 generation-fenced supervisor 功能性探活与自动恢复。

2026-09-06 调整交付顺序：先完成 W1 原生 Loader，再实现 W2 Standard limits。W1 的范围不包含默认
CPU/内存/subrequest enforcement 或 custom limits；显式 limits 必须由原生 API 拒绝，不能静默忽略。
W1 已完成 namespace/权限隔离、结构大小限制、in-flight 计数、缓存与生命周期及正式 pin 验收。
W2 通过请求限额、isolate 摘除和执行器自恢复三层机制消除失控代码影响同进程邻居的已知故障，并完成
Wrangler、v4 Settings、Dynamic Worker ceiling、官方错误分类和完整产品验收。

## 源码与运行时基线

**后续 workerd 修改统一基于仓库中的 [`third_party/workerd/`](../../third_party/workerd/)。**
该目录是用户 fork 的 Git submodule，由根目录 [`.gitmodules`](../../.gitmodules) 登记远端，父仓库 gitlink 固定源码提交。
不要另建一份 workerd 实现、复制到其他目录，
或为了匹配旧的测试二进制而重置这个 checkout。

| 项目                                                              | 2026-09-18 核对结果                                                                                                                                                                                                       |
| ----------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [workerd 上游 issue / PR 核验](../references/workerd-upstream.md) | 已合并能力、standalone 缺口、补丁范围与升级回归重点                                                                                                                                                                       |
| fork origin                                                       | <https://github.com/elliothux/workerd>                                                                                                                                                                                    |
| upstream 项目                                                     | <https://github.com/cloudflare/workerd>                                                                                                                                                                                   |
| fork checkout HEAD                                                | `40937077470ed7edec082329d3a10e4195b402cb`（已推送 fork `main`）                                                                                                                                                          |
| upstream base                                                     | `679c09e5eea0af8a04062e1875e99c75af532e3b`（upstream `v1.20260918.1`）                                                                                                                                                    |
| HEAD 提交说明                                                     | `540983b18` W1 Loader、`5465cdfd9` W2 Standard limits、`19046b1d2` W3 native bindings；`409370774` 为手动正式二进制构建                                                                                                   |
| fork working tree                                                 | 本次记录时 clean；相对 upstream ahead 4                                                                                                                                                                                   |
| fork source tree                                                  | `011ea688c739ceed685d224505719a405e8da71b83748e22fc007698684ba98c`（`sha256(git-ls-tree-r-full-tree)`）                                                                                                                   |
| W3 runtime                                                        | `workerd 2026-09-18`；native Provider FD/Cap'n Proto unary/stream 回归通过                                                                                                                                                |
| open-compute 当前正式 pin                                         | `v1.20260918.1-open-compute-w3.40937077` / `40937077470ed7edec082329d3a10e4195b402cb`（四平台输入均来自 [run 35318268548](https://github.com/elliothux/workerd/actions/runs/35318268548)；成功后以 `bun run build` 验证） |
| 正式 pin authority                                                | [`packages/runtime/workerd.lock.json`](../../packages/runtime/workerd.lock.json)                                                                                                                                          |

源码 checkout 与当前正式 pin 已统一到上述 revision。旧二进制的结果不能作为 fork 的测试结果；fork 的
`--version` 也不能替代源码身份与二进制摘要。正式切换必须完成构建、固定来源和协议、更新所有 pin 消费者及验证。
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
与相关平台代码和 docs 一起提交。父仓库只保存子仓库提交 ID，不会保存未提交的 fork 文件改动。
共享父仓库更新前必须确保被引用的提交已在 fork 远端可获取；push 仍需相应外部写入授权。
迁移后子仓库 Git 元数据由父仓库 `.git/modules/third_party/workerd/` 管理，旧源码目录不保留副本或别名。

构建平台默认使用 [share/workerd](../../share/workerd/README.md) 中 Git LFS 管理的三个正式平台固定二进制；
由根 build 生成并验证正式 archive，不要求初始化 submodule。源码辅助的 conformance 校验在子仓库初始化后，
通过 `git show <正式 pin revision>:<path>` 读取固定版本，不把开发 checkout 当作正式运行时或 npm types 基线；
缺少所需 Git 对象时校验失败，不自动下载或改用 HEAD。后续源码与 pin 升级需保持这些对象可获取。

先读取 fork 的
[开发规则](../../third_party/workerd/AGENTS.md)和修改组件已有的规则，保持其 Bazel/C++/测试布局。

## 文档

本目录按交付顺序编号：W1 为 Dynamic Worker Loader，W2 为 Standard limits。历史平台 P9/P10 记录中的编号
保留为当时的阶段名称，当前方案与链接统一使用本目录名称。

| 文档                                                                                     | 职责                                                                                       |
| ---------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------ |
| [W1 原生 Loader 方案](../implemented/w1-native-limits-loader.md)                         | 接口复用、capability 边界、fork 维护与完成结果                                             |
| [W1 Dynamic Workers / Worker Loader](../implemented/w1-dynamic-workers-worker-loader.md) | public binding、原生 JS API、namespace、动态 Worker 与产品验收合同                         |
| [W2 Workers Standard limits](../implemented/w2-standard-limits.md)                       | Standard limits、公开配置/API、可观察行为和自恢复的完成合同                                |
| [W3 用户可扩展原生 Binding](../implemented/w3-user-extensible-native-bindings.md)        | `ocd` 名字/path 注册、Wrangler `services + props`、Provider 直连与生命周期；不管理扩展版本 |
| [此前 stock workerd 可行性复核](../implemented/p10-worker-loader-feasibility.md)         | 保留旧 pin 的 No-Go 实测；不作为当前 fork 路线的禁令或完成证据                             |

本目录保存尚未完成的 workerd 设计与 fork 维护入口。源码基线、fork 交付方式和内部实现分工以本目录为准；
W1/W2/W3 已完成合同不会因允许 fork 而降低；W3 不把用户 Provider 解释为第二个 workerd 或第二套 authority。

返回[文档索引](../README.md)。
