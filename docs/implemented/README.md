# 已实现阶段文档

2026-08-28 按现有源码和既有验证记录归档。这里保存已完成阶段的设计与相关结果，
不是待执行任务清单，也不表示本次重新运行了测试，或当前工作树的全部未提交修改已经验收。
阶段范围和限制以各自的验证记录为准。

## 归档索引

| 阶段 | 设计 | 完成依据与限制 |
| --- | --- | --- |
| G0 | [workerd 可行性验证](g0-workerd-runtime-validation.md) | [结果](g0-results.md)：Conditional Go，精确 `D-abort` 限制仍保留；一次性探测已完成 |
| P0.1 | [Platform Foundation](p0-1-platform-foundation.md) | [P0.5 验证记录](p0-5-r2.md)记录 P0.1 三轮回归；[真实进程测试](../../crates/service/tests/p0_1_gate.rs)已存在 |
| P0.2 | [Workers Runtime](p0-2-workers-runtime.md)、[阶段 API 矩阵](p0-2-api-matrix.md) | [P0.3 验证记录](p0-3-resource-binding-framework.md)记录 P0.2 三轮回归；[Worker Gate](../../crates/service/tests/p0_2_runtime_gate.rs)已存在 |
| P0.3 | [Resource 与 Binding Framework](p0-3-resource-binding-framework.md) | 文内记录 RB-01 至 RB-18、P0.2 回归和完整检查通过 |
| P0.4 | [KV](p0-4-kv.md) | 文内记录已实现与验证；保持当时声明的本地 KV 支持范围 |
| P0.5 | [R2](p0-5-r2.md) | 文内记录三轮 Gate、相关回归和完整检查通过 |
| P0.6 | [D1](p0-6-d1.md) | 文内记录 D1 实现、三轮 Gate、相关回归和完整检查通过 |
| P0.7 | [Durable Objects](p0-7-durable-objects.md) | 文内记录三轮 Gate 通过；不据此宣称支持 hibernatable WebSocket |
| P0.8 | [Scheduler 与 DO Alarms](p0-8-scheduler-do-alarms.md) | 文内记录 alarm、恢复矩阵及 P0 回归通过 |
| P1.0–P1.7 | [平台加固](p1-platform-hardening.md) | [本地结果](p1-results.md)：核心实现与本地回归完成；长时 soak/发行演练见独立的[剩余验收计划](../p1-release-acceptance.md) |
| P1.8 | [WebSocket hibernation 调查](p1-8-results.md) | 调查完成，结论 No-Go；不宣称 hibernation 功能已实现 |
| P2.1 | [Scheduler 多 Workload 内核](p2-1-scheduler-hardening.md) | 文内记录 aggregate 与 coverage 通过；不扩大 P1.8 支持范围 |
| P2.2 | [Queue Producer](p2-2-queue-producer.md) | [本地结果](p2-2-results.md)：Conditional Go，DO producer 按 output-gate 限制拒绝 |
| P2.3 | [Queue Consumer 与 Cron](p2-3-queue-consumer-cron.md) | [Gate 结果](p2-3-gate-results.md)：Go |
| P2.4 | [Workflow Core](p2-4-workflow-core.md) | [Gate 结果](p2-4-gate-results.md)：Conditional Go，DO 内 create 拒绝 |
| P2.5 / P2 Exit | [Workflow Durable Waiting](p2-5-workflow-durable-waiting.md) | [最终结果](p2-5-gate-results.md)：P2.5 Conditional Go、P2 Exit PASS；DO 内 mutation 等限制未扩大 |

归档包括阶段设计、阶段 API 矩阵和已完成的调查/验证结果。G0 自动生成报告保持原始字节；其余历史报告仅在需要时
调整相对链接，实际命令、轮数、日期、摘要、结论和未验证项不因归档修改。

## 后续实现验收

以下记录来自对应实现任务的实际验收，区别于上面的历史资料整理。

| 日期 | 已完成范围 | 证据与边界 |
| --- | --- | --- |
| 2026-08-28 | [单二进制分发](single-binary-distribution.md) | darwin-arm64 单文件首启/重启/恢复、完整检查、90.20% 行覆盖率和最终 P0.1/P0.2 三轮通过；未正式发布、签名、公证或验证其他平台 |
| 2026-08-29 | [Runtime 包与测试流程整理](runtime-and-test-layout.md) | [实测与验收](runtime-and-test-layout-results.md)：未跟踪 dist 可复现构建、POC 收敛、统一并行调度；六目标串行/四并行 148.10/70.98 秒，workspace 690 用例、90.16% 覆盖率、最终 23 目标各三轮通过；跨平台和正式发行未验证 |

## 仍在维护或尚未完成

- [P1 剩余验收](../p1-release-acceptance.md)：仅追踪尚无完成证据的长时 soak 与发行演练，
  不把已完成的 P1 核心实现重新列为待实现。
- [平台方案](../open-compute-workerd-platform.md)：Next.js/vinext 目标尚未完成平台验收。
- [Day1 架构清理](../day1-architecture-cleanup.md)：除已归档的 runtime 布局与测试流程外，其余清理仍待实施。
- [Runtime 跨平台发行验收](../runtime-layout-release-acceptance.md)：CI、特权 egress 和正式发行资格尚未执行，不回写为本机已通过。
- 测试、能力偏差、fuzz 所有权、部署和运维手册统一放在 [docs/references](../references/README.md)。

## 使用规则

归档文档中的历史目录、命令、实现模型和兼容约束只说明当时的阶段设计与证据。
当前架构与测试策略以 [AGENTS.md](../../AGENTS.md)、现行实现和维护中的文档为准；
不能为了复现旧文档而保留过时的内部实现、目录别名或重复测试。

以后每个实现 goal 完成约定验收后，在交付前归档相应设计及完成记录，更新本索引、入站链接、
相对链接和相关代码/报告生成器的路径。部分完成、缺少验收或仍混有未完成目标的文档不整体归档。
