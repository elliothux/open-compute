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
| 2026-08-29 | [按用例选择验收轮数](test-repetition.md) | 完整 workspace 690 用例一次、43 个时序用例追加两次，70 个宿主执行/776 次用例全部通过；90.15% 行覆盖率，同条件追加轮墙钟减少 16.85%；重启后 macOS 签名服务仍崩溃一次并自行恢复，系统缺陷未根治，跨平台和正式发行未验证 |
| 2026-08-29 | [Day1 架构清理](day1-architecture-cleanup.md) | [验收记录](day1-architecture-cleanup-results.md)：清单 1–17 与 artifact GC 完成；coverage 32 目标通过、90.02% 行覆盖率；最终完整 32 目标 / 662 用例一轮及 17 目标 / 42 个时序用例追加两轮全部通过 |
| 2026-08-29 | [P3.1 Static Assets 与框架产物导入](p3-1-static-assets.md) | Worker + Assets、Assets-only、上传恢复、不可变路由与 binding 本地产品矩阵完成；33 个 coverage 目标、90.11% 行覆盖率和最终 workspace 一轮 + 17 个时序用例两轮通过；direct Cloudflare differential 见独立验收计划 |
| 2026-08-30 | [P3.2 Service Binding 与原生 Worker 调用](p3-2-service-bindings.md) | hard/product/events/recovery 四个 stock-workerd target、原生 RPC 生命周期与 SIGKILL 恢复完成；90.11% 行覆盖率、最终 834/834 case 通过；direct Cloudflare differential 见独立验收计划 |
| 2026-08-30 | [P3.3 Workers Cache、Cache API 与 Images](p3-3-workers-cache-images.md) | 声明的单节点支持面 Platform Go：完整 37 目标 coverage 通过、Rust 行覆盖率 90.10%，最终完整 37 目标一轮及 19 个登记时序目标追加两轮全部通过；Cloudflare differential、跨平台发行与第三方应用 qualification 未纳入结论 |
| 2026-09-01 | [Cloudflare Runtime 全量兼容改造](cloudflare-runtime-compatibility.md)、[P3.4 conformance](p3-4-cloudflare-conformance.md) | [完成报告](cloudflare-runtime-compatibility-results.md)：2,097 个 stable members、1,585 `supported`、512 `supported_with_deviation`、`blocked=0`；193/193 JS、802/802 单轮 workspace cases、90.17% Rust 行覆盖率；七项 hosted differential 已通过，Workflow hosted qualification 与正式发行/跨平台资格仍为明确限制 |
| 2026-09-01 | [P4 Next.js/vinext 应用资格验证](p4-nextjs-vinext-qualification.md)、[P4.0 build reproducibility 调查](p4-nextjs-vinext-p4-0-results.md)、[结果](p4-nextjs-vinext-results.md) | 原跨 source-build Hard Gate 已按 Cloudflare Worker Version/Deployment 语义撤回；固定 artifact 的 Wrangler/importer inventory 79/79 对齐，20/20 selected mandatory 通过，Cloudflare/open-compute runner 各 15/15，双端精确清理完成；197/197 JS、90.17% Rust 行覆盖率、最终 894/894 case executions 通过；Application Go 不替代 Platform verdict |
| 2026-09-01 | [`ocd` Day1 命名改造](ocd-day1-rename.md) | [完成记录](ocd-day1-rename-results.md)：唯一 production binary/CLI/daemon 为 `ocd`，project/docs origin 为 `https://open-compute.dev`，launchd identity 为 `dev.open-compute.ocd`；198/198 JS、90.18% Rust 行覆盖率、完整单轮 Gate 40/40 targets 与 802/802 cases 通过；追加时序轮按用户指定不作为完成条件，未执行正式发行或跨平台验证 |
| 2026-09-03 | [Operator API 与可选 Dashboard](operator-api-dashboard.md) | **Implementation GO**（[`CR.md`](../../CR.md)）：真实 `dev-test.sh`/`ocd` 与 Cloudflare Dashboard 对比完成；服务端 catalog filter/sort、全产品管理闭环、Kumo/响应式、Playwright **31/31**、live SDK **12/12**、Rust 行覆盖率 **90.14%**；用户指定的最终单轮 Gate **42/42 targets、835/835 cases**，详见[完成记录](operator-api-dashboard-results.md) |

## 仍在维护或尚未完成

- [P1 剩余验收](../p1-release-acceptance.md)：仅追踪尚无完成证据的长时 soak 与发行演练，
  不把已完成的 P1 核心实现重新列为待实现。
- [平台方案](../open-compute-workerd-platform.md)：核心 Day1 实现与 P3 本地 conformance 已完成；该文档
  继续维护当前架构、能力边界和仍未结束的 hosted/release qualification，因此暂留 active 根目录。
- [Static Assets / Service Binding 远端资格](../p3-assets-service-bindings-acceptance.md)：两项核心设计已
  归档，只继续追踪尚未执行的 direct Cloudflare differential。
- [Cloudflare Workflow 远端 differential](../cloudflare-runtime-compatibility-acceptance.md)：本地 Day1
  runtime 与七项 hosted fixture 已完成，只追踪因 Wrangler OAuth `10000` 未运行的 Workflow 托管端资格。
- [Runtime 跨平台发行验收](../runtime-layout-release-acceptance.md)：CI、特权 egress 和正式发行资格尚未执行，不回写为本机已通过。
- active 文档的完整分类见 [docs 索引](../README.md)。
- 测试、能力偏差、fuzz 所有权、部署和运维手册统一放在 [docs/references](../references/README.md)。

## 使用规则

归档文档中的历史目录、命令、实现模型和兼容约束只说明当时的阶段设计与证据。
当前架构与测试策略以 [AGENTS.md](../../AGENTS.md)、现行实现和维护中的文档为准；
不能为了复现旧文档而保留过时的内部实现、目录别名或重复测试。

以后每个实现 goal 完成约定验收后，在交付前归档相应设计及完成记录，更新本索引、入站链接、
相对链接和相关代码/报告生成器的路径。部分完成、缺少验收或仍混有未完成目标的文档不整体归档。
