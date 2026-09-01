# 文档索引

`docs/` 根目录只保留仍需实施或验收的计划，以及跨阶段持续维护的总方案。已经完成并有证据的阶段
设计放在 [implemented](implemented/README.md)；稳定接口、测试规则和运维手册放在
[references](references/README.md)。历史 PASS 只证明对应 revision 和输入，不能替代当前实现与 Gate。

## 待实施

| 文档 | 当前状态 |
| --- | --- |
| [Operator API 与可选 Dashboard](operator-api-dashboard.md) | 方案完成，尚未收敛管理路径、强制管理员鉴权或实现 React SPA |

已完成的 `ocd` Day1 命名改造见[设计归档](implemented/ocd-day1-rename.md)与
[完成记录](implemented/ocd-day1-rename-results.md)。

## 待验收

| 文档 | 当前状态 |
| --- | --- |
| [P1 剩余验收](p1-release-acceptance.md) | 长时 soak 与正式发行演练尚无完成证据 |
| [Runtime 跨平台发行验收](runtime-layout-release-acceptance.md) | CI、跨平台、特权 egress 与正式发行资格尚未执行 |
| [Cloudflare Workflow 远端 differential](cloudflare-runtime-compatibility-acceptance.md) | 本地实现完成；托管端因 credential 条件尚未运行 |
| [Static Assets / Service Binding 远端资格](p3-assets-service-bindings-acceptance.md) | 两项核心实现已归档；直接 Cloudflare differential 尚未执行 |

## 跨阶段总方案

- [单机 Cloudflare Workers Platform 总方案](open-compute-workerd-platform.md)：核心 Day1 实现已经
  完成，但文档同时维护当前平台架构、能力边界及上述未完成资格，因此暂不作为单一已完成阶段归档。

归档不能靠修改状态标签完成。移动前必须有实现和验收证据；只剩外部、长时或发行资格时，应把它们
拆到独立 active acceptance，再归档已经完成的核心设计。
