# 文档索引

`docs/` 根目录只保留仍需实施或验收的计划，以及跨阶段持续维护的总方案。已经完成并有证据的阶段
设计放在 [implemented](implemented/README.md)；稳定接口、测试规则和运维手册放在
[references](references/README.md)。历史 PASS 只证明对应 revision 和输入，不能替代当前实现与 Gate。

## 待实施

| 文档 | 当前状态 |
| --- | --- |
| [Day 1 Cloudflare v4 API 与 Wrangler 子集兼容](day1-cloudflare-v4-wrangler-compatibility.md) | 设计完成；唯一 `/client/v4` 管理面、Wrangler/multipart/Assets/resource API 与 vendor namespace 尚未实施 |
| [Day 1 Workers Standard limits](day1-workers-standard-limits.md) | 设计完成；structural limits、Version settings 与 stock workerd runtime enforcer 尚未实施，CPU/subrequest/memory/startup/connection 当前受 `OC-WKR-LIMIT-001` 阻断 |
| [Day 1 Dynamic Workers / Worker Loader](day1-dynamic-workers-worker-loader.md) | 合同与架构完成；`worker_loaders` v4/Version 支持受 upstream stock workerd nested-loader、limits 与 bounded-cache G0 阻断；Workers for Platforms 不在范围内 |
| [Day 1 Workers Logs 与 realtime tail](day1-workers-logs-realtime-tail.md) | 设计完成；stock workerd Tail 采集、`wrangler tail`、Workers Logs persistence/Telemetry query 与 Dashboard Live Tail 尚未实施 |
| [P5 Vectorize 与 AI Search](p5-vectorize-ai-search.md) | Research/Day1 方案完成；Vectorize、AI provider、AI Search、恢复与兼容 Gate 尚未实现 |
| [P5.7 Xberg 文档解析](p5-7-xberg-document-parsing.md) | Research/Day1 方案完成；CF-compatible `env.AI.toMarkdown`/AI Search API、Xberg parser child、38-file corpus 与四平台 Gate 尚未实现 |

新的管理面与项目配置目标由 [Day 1 Cloudflare v4 API 与 Wrangler 子集兼容设计](day1-cloudflare-v4-wrangler-compatibility.md)
定义。Operator API 与 Dashboard 已完成并归档：见[设计文档](implemented/operator-api-dashboard.md)、
[完成记录](implemented/operator-api-dashboard-results.md)与 **Implementation GO** 复审（[`CR.md`](../CR.md)）。
当前证据包括真实 Cloudflare Dashboard 对比、Playwright **31/31**、live SDK **12/12**、Rust 行覆盖率
**90.14%**，以及用户指定的最终单轮 workspace Gate **42/42 targets、835/835 cases**。

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
