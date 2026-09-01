# P1 剩余验收计划

记录日期：2026-08-28。状态：下列验收尚无完成证据，本次仅从已完成的核心实现文档中拆出，
没有执行新的 soak、打包、部署或发行演练。

[P1 本地结果](implemented/p1-results.md)记录了 P1.0–P1.7 核心实现及本地回归通过，
对应 [阶段设计](implemented/p1-platform-hardening.md)已经归档。该记录同时明确：长时 soak 和
发行演练未执行，不能把核心实现完成写成完整 release qualification 通过。

本计划验收的是“单机发行物能否稳定安装、运行和恢复”，不是 Cloudflare API 行为对齐。后者由
[P3.4 conformance](implemented/p3-4-cloudflare-conformance.md)及各产品 Gate 负责；release qualification 通过
不能把未验证 contract 写成 supported，反之应用 workload 未运行也不否定发行形态。

## 剩余工作

- [ ] 1 小时 developer mixed soak：在明确的源码、正式 workerd pin、配置和主机基线上，
  验证组合负载与故障恢复，记录资源增长、错误、恢复时间和实际持续时间。
- [ ] 24 小时 release-candidate mixed soak：保留完整运行身份、故障计划、稳定性指标和失败证据；
  不能用 10 分钟 smoke 或普通 Gate 的通过结果代替。
- [ ] 当前发行形态的操作演练：针对明确目标平台和已授权生成的单个 `ocd`，验证隔离环境
  首次启动、service/container 运行、备份和 fresh-host restore，以及当前发行替换与失败恢复流程；
  版本和公开 artifacts 必须来自[版本与发布流程](references/releasing.md)，不能用手工旁路产物替代。
- [ ] 汇总实际命令、基线、持续时间、逐项结果和限制；只在约定验收实际完成后归档本计划及新结果。

这些任务需要在执行前核对现行脚本和支持范围。历史 P1 设计中的旧发行布局、旧平台版本迁移、
G0 必跑关系和旧路径不构成新的兼容义务；按已验收的 [Day1 清理要求](implemented/day1-architecture-cleanup.md)与
[测试规则](references/testing.md)选择当前有效的验证路径，不为验收恢复已淘汰的实现。

## 执行边界

- 长时验收放在实现与审查完成、源码冻结后，不作为每次开发迭代的前置条件。
- 打包、runtime 下载、部署、提权及现有数据变更仍需相应授权；本计划不替代用户授权。
- 不覆盖已有数据、发行物或失败记录；使用隔离环境，保留实际失败原因，不能重试到通过后隐藏前次结果。
- [P1.8 调查](implemented/p1-8-results.md)保留其当时的 No-Go 事实；后续 Day1 Cloudflare runtime
  改造已经重新实现并验收 hibernatable WebSocket。发行验收继续使用当前 capability/Gate，不把旧报告
  当成现行功能边界。
- 历史验证报告保持原始结果。本计划的完成记录使用新的源码/输入身份，不改写原报告来补齐未运行项。
