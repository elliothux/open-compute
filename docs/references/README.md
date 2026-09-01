# 参考文档

这里保存持续维护的接口说明、测试规则和操作指南。文件在此目录不表示相关功能尚未实现；
未完成的工作计划位于 `docs/`，已完成的阶段设计和结果位于 [implemented](../implemented/README.md)。

仓库的产品目标是一个可单机部署的 Cloudflare Workers Platform 兼容基础设施。公开能力以
capability、deviation、固定 Cloudflare contract 和真实产品 Gate 为准；vinext 等框架只提供应用
qualification。目标产品、upstream types、single-latest runtime 和管理面边界见已完成的
[Cloudflare Worker Runtime 全量兼容改造](../implemented/cloudflare-runtime-compatibility.md)；总体架构见
[平台总方案](../open-compute-workerd-platform.md)，契约目录、portable differential 和双 verdict
规则见已完成的[P3.4 方案](../implemented/p3-4-cloudflare-conformance.md)。尚未完成的 Cloudflare
Workflow 远端资格只记录在[剩余验收计划](../cloudflare-runtime-compatibility-acceptance.md)。

## 开发与接口

| 文档 | 用途 |
| --- | --- |
| [测试节奏](testing.md) | 默认完整一轮；最终完整一轮 + 时序补两轮，用例清单校验、并行/独占与覆盖率；[原布局实测](../implemented/runtime-and-test-layout-results.md) |
| [Cloudflare 兼容矩阵](cloudflare-compatibility.md) | 当前实现 capability、方法、目标缺口、非目标产品、deviation 与 conformance verdict |
| [能力偏差](p1-deviations.md) | 当前 capability deviation ID 与实际支持边界 |
| [Fuzz 所有权](p1-fuzz-ownership.md) | 各类输入的测试归属和回归要求 |
| [单二进制分发与部署](single-binary.md) | 构建输入、离线启动、资源物化和发行契约 |

P0.2 的阶段性支持范围见归档的 [API 矩阵](../implemented/p0-2-api-matrix.md)；
不要用该阶段“尚未接入产品绑定”的描述替代当前平台的 capability 输出。

## 运维手册

- [安装与首次启动](runbooks/install-and-first-start.md)
- [备份与保留](runbooks/backup-and-retention.md)
- [全新主机恢复](runbooks/fresh-host-restore.md)
- [当前 release 恢复](runbooks/current-release-recovery.md)
- [磁盘压力](runbooks/disk-pressure.md)
- [SQLite 损坏](runbooks/sqlite-corruption.md)
- [S3 故障](runbooks/s3-outage.md)
- [workerd 崩溃循环](runbooks/workerd-crash-loop.md)
- [Master key 丢失与恢复](runbooks/master-key-loss-and-recovery.md)
- [Scheduler 恢复](runbooks/scheduler-recovery.md)
- [收集 support bundle](runbooks/collect-support-bundle.md)

runbooks 由 `platformd` 在编译时内嵌。仓库目录调整不改变 `platformd docs <name>` 的手册名称，
修改路径时必须同步资源读取和相关测试。操作权限、Day1 设计及实际支持范围仍以
[AGENTS.md](../../AGENTS.md) 和当前实现为准。
