# 已完成实现与验证记录

当前架构见[平台总览](open-compute-workerd-platform.md)。本目录保存维护摘要和实际验收记录；历史 PASS 只适用于记录中的输入。
当前 API／偏差见[兼容矩阵](../references/cloudflare-compatibility.md)，执行规则见[测试手册](../references/testing.md)。

## 平台与产品

| 范围 | 维护文档 | 历史验收与限制 |
| --- | --- | --- |
| P0.1 平台基础 | [实现](p0-1-platform-foundation.md) | [P0.5 回归记录](p0-5-r2.md) |
| P0.2 Worker | [实现](p0-2-workers-runtime.md) | [P0.3 回归记录](p0-3-resource-binding-framework.md)、[阶段 API 矩阵](p0-2-api-matrix.md) |
| P0.3 Binding | [实现与验证](p0-3-resource-binding-framework.md) | 资源 lifecycle、capability 与部署引用 |
| P0.4 KV | [实现与验证](p0-4-kv.md) | Namespace SQLite、streaming、backup／restore |
| P0.5 R2 | [实现与验证](p0-5-r2.md) | 对象、multipart 与资源隔离；后端演进见 P8 |
| P0.6 D1 | [实现与验证](p0-6-d1.md) | SQLite 执行、事务、backup 与恢复 |
| P0.7 Durable Objects | [实现与验证](p0-7-durable-objects.md) | 原生 facet、对象 generation 与持久存储 |
| P0.8 Alarms | [实现与验证](p0-8-scheduler-do-alarms.md) | Object authority 与 scheduler 投影 |
| P1 平台加固 | [实现](p1-platform-hardening.md) | [结果](p1-results.md)；长时／发行资格另列 |
| P2.1 Scheduler | [实现与验证](p2-1-scheduler-hardening.md) | 多 workload、公平性与恢复 |
| P2.2 Queue Producer | [实现](p2-2-queue-producer.md) | [结果](p2-2-results.md)：当次 Conditional Go |
| P2.3 Consumer/Cron | [实现](p2-3-queue-consumer-cron.md) | [结果](p2-3-gate-results.md)：Go |
| P2.4 Workflow Core | [实现](p2-4-workflow-core.md) | [结果](p2-4-gate-results.md)：当次 Conditional Go |
| P2.5 持久等待 | [实现](p2-5-workflow-durable-waiting.md) | [结果](p2-5-gate-results.md)：P2.5 Conditional Go、P2 Exit PASS |
| P3.1 Static Assets | [实现与验证](p3-1-static-assets.md) | Direct hosted differential 另列 |
| P3.2 Service Binding | [实现与验证](p3-2-service-bindings.md) | Direct hosted differential 另列 |
| P3.3 Cache/Images | [实现与验证](p3-3-workers-cache-images.md) | 声明单节点范围的 Platform Go |
| Runtime 兼容 / P3.4 | [兼容实现](cloudflare-runtime-compatibility.md)、[Conformance](p3-4-cloudflare-conformance.md) | [结果](cloudflare-runtime-compatibility-results.md)；Workflow hosted 资格另列 |
| P4 Next.js/vinext | [Qualification](p4-nextjs-vinext-qualification.md) | [结果](p4-nextjs-vinext-results.md)：Application Go |
| Dashboard | [实现](operator-api-dashboard.md) | [结果](operator-api-dashboard-results.md)：Implementation GO；管理协议见 P6 |
| P5 Vectorize/AI Search | [实现](p5-vectorize-ai-search.md)、[文档解析](p5-7-xberg-document-parsing.md) | [结果](p5-vectorize-ai-search-results.md)；parser／hosted／发行资格另列 |
| P6 v4 管理面 | [合同](p6-cloudflare-v4-wrangler-compatibility.md) | [结果](p6-cloudflare-v4-wrangler-compatibility-results.md)；保留实际未验收项 |
| P7 Logs/Tail | [实现与验证](p7-workers-logs-realtime-tail.md) | Implementation GO；扩展资格另列 |
| P8 Local/S3 | [实现与验证](p8-local-s3-object-backend.md) | Implementation GO；未执行发行／跨平台资格 |

## 工程改造

| 范围 | 实现与证据 |
| --- | --- |
| 单二进制分发 | [实现与验证](single-binary-distribution.md) |
| Runtime／测试布局 | [职责与断言归属](runtime-and-test-layout.md)、[结果](runtime-and-test-layout-results.md) |
| 测试轮数调查 | [历史测量与失败记录](test-repetition.md)；现行要求以测试手册为准 |
| Day1 清理 | [维护边界](day1-architecture-cleanup.md)、[结果](day1-architecture-cleanup-results.md) |
| ocd 命名 | [契约](ocd-day1-rename.md)、[结果](ocd-day1-rename-results.md) |
| GitHub Issues #1–#3 | [请求体、Assets multipart、Cron generation 修复与验收](github-issues-1-3.md) |
| GitHub Issue #4 | [R2 上传优化与验收](github-issue-4-r2-upload.md) |

## 已结束调查

| 调查 | 结果 |
| --- | --- |
| G0 workerd | [摘要](g0-workerd-runtime-validation.md)、[原始报告](g0-results.md)：Conditional Go，保留 `D-abort` 限制 |
| P1.8 hibernation | [原始报告](p1-8-results.md)：当时 No-Go，不代表当前能力 |
| P4 build reproducibility | [原始报告](p4-nextjs-vinext-p4-0-results.md)：保留原调查及后续判定依据 |
| P10 stock Loader | [原始报告](p10-worker-loader-feasibility.md)：No-Go；后续原生实现见 [workerd 方案](../workerd/README.md) |

未完成设计见[文档索引](../README.md)，剩余资格见[验收索引](../acceptance/README.md)。
原始报告的命令、日期、摘要、结果与失败证据保持原样；阶段设计不构成历史配置、协议或测试入口的兼容义务。
