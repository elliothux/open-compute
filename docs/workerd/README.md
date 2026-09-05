# workerd 原生运行时方案

状态：**方案已选定，原生实现与验收待完成**。2026-09-05 用户确认接受维护自己的 workerd fork 并重新编译。
P1/P2 不再以“等待上游合并后才能开发”为实施前提；这不代表当前运行时已支持 limits 或 public Loader。


2026-09-06 调整交付顺序：先完成 P1 原生 Loader，再实现 P2 Standard limits。P1 的范围不包含默认
CPU/内存/subrequest enforcement 或 custom limits；显式 limits 必须由原生 API 拒绝，不能静默忽略。
P1 仍须完成 namespace/权限隔离、结构大小限制、in-flight 计数、缓存与生命周期及正式 pin 验收。
该子集不宣称完整 Cloudflare 资源限制兼容，也不保证失控代码不会影响同进程邻居；P2 完成后消除此偏差。

## 源码与运行时基线

**后续 workerd 修改统一基于仓库中的 [`third_party/workerd/`](../../third_party/workerd/)。**
该目录是用户 fork 的 Git submodule，由根目录 [`.gitmodules`](../../.gitmodules) 登记远端，父仓库 gitlink 固定源码提交。
不要另建一份 workerd 实现、复制到其他目录，
或为了匹配旧的测试二进制而重置这个 checkout。

| 项目 | 2026-09-05 核对结果 |
| --- | --- |
| [workerd 上游 issue / PR 核验](../references/workerd-upstream.md) | 已合并能力、standalone 缺口、补丁范围与升级回归重点 |
| fork origin | <https://github.com/elliothux/workerd> |
| upstream 项目 | <https://github.com/cloudflare/workerd> |
| fork checkout HEAD | `dd8133e9b9656fb39f1434247a80aa7a249ee204` |
| HEAD 提交说明 | `Release 2026-09-05` |
| fork working tree | 本次记录时 clean；不据此推断与 upstream 没有差异 |
| open-compute 当前正式 pin | `v1.20260830.1` / `e9dda5963aba7ee4323960db795690ec78fec118` |
| 正式 pin authority | [`packages/runtime/workerd.lock.json`](../../packages/runtime/workerd.lock.json) |

源码 checkout 与当前正式 pin **不是同一 revision**。旧二进制的结果不能作为 fork 的测试结果；fork 的
`--version` 也不能替代源码身份与二进制摘要。正式切换必须完成构建、固定来源和协议、更新所有 pin 消费者及验证。
此次迁移保留上述 HEAD、`main` 分支和 origin，没有编译 fork、更新 lock 或部署服务。

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

仅使用当前正式 archive 构建平台时不要求初始化 submodule。源码辅助的 conformance 校验在子仓库初始化后，
通过 `git show <正式 pin revision>:<path>` 读取固定版本，不把开发 checkout 当作正式运行时或 npm types 基线；
缺少所需 Git 对象时校验失败，不自动下载或改用 HEAD。后续源码与 pin 升级需保持这些对象可获取。

先读取 fork 的
[开发规则](../../third_party/workerd/AGENTS.md)和修改组件已有的规则，保持其 Bazel/C++/测试布局。

## 文档

本目录按交付顺序编号：P1 为 Dynamic Worker Loader，P2 为 Standard limits。历史平台 P9/P10 记录中的编号
保留为当时的阶段名称，当前方案与链接统一使用本目录名称。

| 文档 | 职责 |
| --- | --- |
| [原生 limits 与 Loader 实施方案](native-limits-loader.md) | 接口复用、必须修改的内部路径、预算和 capability 边界、实施顺序与 fork 维护 |
| [P1 Dynamic Workers / Worker Loader](p1-dynamic-workers-worker-loader.md) | public binding、原生 JS API、namespace、动态 Worker 与产品验收合同 |
| [P2 Workers Standard limits](p2-workers-standard-limits.md) | 管理面、Version、运行时限制及产品验收合同；含之前的局部实施记录 |
| [此前 stock workerd 可行性复核](../implemented/p10-worker-loader-feasibility.md) | 保留旧 pin 的 No-Go 实测；不作为当前 fork 路线的禁令或完成证据 |

本目录是用户指定的 active design 目录。源码基线、fork 交付方式和内部实现分工以本目录为准；P1/P2 的
Cloudflare 可观察合同不会因允许 fork 而降低。完成实现和约定验收后再归档到 `docs/implemented/`，不保留旧路径占位。

返回[文档索引](../README.md)。
