# open-compute 文档与参考

本目录保存持续维护的接口、测试、发布和运维资料。活动方案与历史实现从[文档索引](../README.md)进入。

## 文档位置

| 位置 | 内容 |
| --- | --- |
| `docs/*.md` | 仍需实施的方案 |
| `docs/workerd/` | 仍需实施的原生 workerd 方案 |
| `docs/implemented/` | 已完成需求的精简结果和当时证据 |
| `docs/releases/` | 已发布版本的用户可见 release notes |
| `docs/acceptance/` | 核心实现完成后剩余的真实环境资格 |
| `docs/blocked/` | 被外部条件阻塞且当前无法继续的方案 |
| `docs/references/` | 持续维护的接口、测试、发布和运维资料 |

不要为普通单篇文档建专用目录，也不要在多个位置复述同一规则；上表按文档生命周期划分的目录除外。

## 状态与移动

- `planned`：最终行为已决定，代码尚未全部落地；
- `implemented`：代码已落地，但列出的真实环境或生命周期检查尚未完成；
- `verified`：明确记录的检查已经成功退出；
- `accepted limitation`：已知限制及影响被当前产品接受；
- `blocked`：外部前置条件阻止继续，并写明恢复条件。

有待实现内容时留在 `docs/` 或 `docs/workerd/`。实现完成后先删掉计划过程，再把精简结果移入 `implemented/`；正式 release notes 放入
`releases/`；只剩资格时把资格拆到 `acceptance/`。只有真正无法继续的外部阻塞才进入 `blocked/`。路径变化同步更新索引、生成器和链接，
不保留 redirect、stub 或旧副本。

## 编号

需求、实施、验收和阻塞文档必须沿用同一个编号，文件名使用小写编号前缀，不允许无编号文件：

| 前缀 | 范围 | 示例 |
| --- | --- | --- |
| `P` | open-compute 产品阶段 | `p12-wrangler-project-workflow.md` |
| `W` | workerd 子项目阶段 | `w2-standard-limits.md` |
| `I` | GitHub issue 实施批次 | `i1-github-issues-1-3.md`、`i2-github-issue-4-r2-upload.md` |
| `G` | 一次性调查或 Gate 研究 | `g1-test-repetition.md` |
| `Q` | 质量专项 | `q0-code-quality-2026-09-08.md` |

子阶段和插入主线之间的补充阶段继续使用所属序列，例如 `P2.6` 写作 `p2-6-*`。同一需求从活动方案移动到
`implemented/`、`acceptance/` 或 `blocked/` 时编号不变。只有各目录 `README.md`、持续维护的
`references/` 与 `runbooks/`、以及以 SemVer 命名的 `releases/` 不使用上述编号。

## 内容与证据

- 先写用户结果和会影响以后修改的边界；只在必要时补 ownership、失败语义、非目标和证据。
- `implemented/` 不是计划归档：不保留调研、候选方案、实施顺序、完整 schema/API、逐文件任务、逐项测试矩阵或完成的 TODO。
- 原始命令输出和失败 artifact 留在 `.temp/` 或 CI，不复制进长期文档；文档只记录日期、输入、结论和定位证据所需的标识。
- 只有成功退出的检查才能写成通过。历史 PASS 只证明当时输入，不代表当前工作树自动通过。
- 当前接口和支持面由源码、机器可读合同及本目录的维护文档拥有；历史实现文档不覆盖它们。
- 文档改动至少运行 `git diff --check` 并核对链接、状态和 verified 声明；过期内容直接删除，Git 保存历史。
- 新增或移动文档时核对上述目录中除 `README.md` 外不存在无编号文件。

## 维护资料

| 文档 | 用途 |
| --- | --- |
| [Cloudflare 兼容矩阵](cloudflare-compatibility.md) | 当前支持面和 deviation |
| [能力偏差](p1-deviations.md) | 当前 deviation ID 与边界 |
| [测试节奏](testing.md) | Gate、case discovery、覆盖率和验收 |
| [单二进制分发](single-binary.md) | 构建输入、离线启动和发行合同 |
| [发布流程](releasing.md) | 版本、tag、release workflow 和校验 |
| [CI 构建性能](ci-build-performance.md) | 当前 CI、并行和缓存取舍 |
| [workerd 上游](workerd-upstream.md) | 上游能力与 fork 升级重点 |
| [Vinext 输入](vinext-input-validation.md) | 固定输入与离线校验 |
| [Fuzz 所有权](p1-fuzz-ownership.md) | 输入 corpus 与回归归属 |

运维手册位于 [`runbooks/`](runbooks/)，由 `ocd` 编译时内嵌。调整路径或名称时必须同步资源读取和测试。
