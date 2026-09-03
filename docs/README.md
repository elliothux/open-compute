# 文档索引

`docs/` 根目录只保留仍需实施或验收的计划，以及跨阶段持续维护的总方案。已经完成并有证据的阶段
设计放在 [implemented](implemented/README.md)；稳定接口、测试规则和运维手册放在
[references](references/README.md)。历史 PASS 只证明对应 revision 和输入，不能替代当前实现与 Gate。

## 待实施

| 文档 | 当前状态 |
| --- | --- |
| [P7 Workers Logs 与 realtime tail](p7-workers-logs-realtime-tail.md) | 设计完成；stock workerd Tail 采集、`wrangler tail`、Workers Logs persistence/Telemetry query 与 Dashboard Live Tail 尚未实施 |
| [P8 Workers Standard limits](p8-workers-standard-limits.md) | 设计完成；structural limits、Version settings 与 stock workerd runtime enforcer 尚未实施，CPU/subrequest/memory/startup/connection 当前受 `OC-WKR-LIMIT-001` 阻断 |
| [P9 Dynamic Workers / Worker Loader](p9-dynamic-workers-worker-loader.md) | 合同与架构完成；`worker_loaders` v4/Version 支持受 upstream stock workerd nested-loader、limits 与 bounded-cache G0 阻断；Workers for Platforms 不在范围内 |
| [P10 Cloudflare Artifacts](p10-cloudflare-artifacts.md) | Day 1 合同与架构完成；标准 v4/Worker binding/Git Smart HTTP 受进程内 Git engine G0 阻断；不把现有内部 ArtifactStore 或 LynxOS 文件夹伪装成 Cloudflare Artifacts |
| [P11 Cloudflare Browser Run](p11-browser-run.md) | Day 1 合同与架构完成；标准 binding/Quick Actions/DevTools/CDP 通过 operator-owned 外部 Browser Provider 执行，受真实 stock-workerd/package/provider G0 阻断；正式 open-compute 发布仍是单个 `ocd` |

P6 本地核心已经归档：见 [Cloudflare v4 API 与固定客户端兼容设计](implemented/p6-cloudflare-v4-wrangler-compatibility.md)
和[完成记录](implemented/p6-cloudflare-v4-wrangler-compatibility-results.md)；仍需外部账号的托管端资格单独保留在
[P6 远端差分验收](p6-cloudflare-v4-differential-acceptance.md)。Operator API 与 Dashboard 已完成并归档：见
[设计文档](implemented/operator-api-dashboard.md)、
[完成记录](implemented/operator-api-dashboard-results.md)与 **Implementation GO** 复审（[`CR.md`](../CR.md)）。
当前证据包括真实 Cloudflare Dashboard 对比、Playwright **31/31**、live SDK **12/12**、Rust 行覆盖率
**90.14%**，以及用户指定的最终单轮 workspace Gate **42/42 targets、835/835 cases**。

已完成的 `ocd` Day1 命名改造见[设计归档](implemented/ocd-day1-rename.md)与
[完成记录](implemented/ocd-day1-rename-results.md)。
P5 核心实现见 [Vectorize 与 AI Search](implemented/p5-vectorize-ai-search.md)、
[Xberg 文档解析](implemented/p5-7-xberg-document-parsing.md)及[完成记录](implemented/p5-vectorize-ai-search-results.md)。

## 待验收

| 文档 | 当前状态 |
| --- | --- |
| [P1 剩余验收](p1-release-acceptance.md) | 长时 soak 与正式发行演练尚无完成证据 |
| [Runtime 跨平台发行验收](runtime-layout-release-acceptance.md) | CI、跨平台、特权 egress 与正式发行资格尚未执行 |
| [Cloudflare Workflow 远端 differential](cloudflare-runtime-compatibility-acceptance.md) | 本地实现完成；托管端因 credential 条件尚未运行 |
| [Static Assets / Service Binding 远端资格](p3-assets-service-bindings-acceptance.md) | 两项核心实现已归档；直接 Cloudflare differential 尚未执行 |
| [P5 剩余发行验收](p5-release-acceptance.md) | P5 本地核心已归档；可复现 benchmark report、四平台、完整 parser process matrix、托管 rich-document differential 与正式 package 尚待完成 |
| [P6 Cloudflare v4 与固定客户端远端差分](p6-cloudflare-v4-differential-acceptance.md) | P6 本地核心已归档；当前环境没有 Cloudflare credentials，新 P6 管理资源、官方 SDK 与 Assets 托管端资格尚未执行；workspace/coverage 总验收按阶段约定延后到 P9 |

## 跨阶段总方案

- [单机 Cloudflare Workers Platform 总方案](open-compute-workerd-platform.md)：核心 Day1 实现已经
  完成，但文档同时维护当前平台架构、能力边界及上述未完成资格，因此暂不作为单一已完成阶段归档。

归档不能靠修改状态标签完成。移动前必须有实现和验收证据；只剩外部、长时或发行资格时，应把它们
拆到独立 active acceptance，再归档已经完成的核心设计。
