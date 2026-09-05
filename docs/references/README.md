# 维护中的参考文档

接口与运维以本目录和当前实现为准；历史验收见[完成索引](../implemented/README.md)，
未实现设计与剩余资格分别见[文档索引](../README.md)和[验收索引](../acceptance/README.md)。

## 开发与接口

| 文档 | 用途 |
| --- | --- |
| [CI 构建性能](ci-build-performance.md) | 轻量开发 CI、并行发行、缓存与失败构建保留，以及实际耗时研究 |
| [workerd 上游 issue / PR 核验](workerd-upstream.md) | 已合并能力、standalone 缺口、补丁范围与升级回归重点 |
| [测试节奏](testing.md) | 单轮调度、case discovery、并行隔离、覆盖率与最终验收 |
| [Cloudflare 兼容矩阵](cloudflare-compatibility.md) | 当前实现 capability、方法、目标缺口、非目标产品、deviation 与 conformance verdict |
| [能力偏差](p1-deviations.md) | 当前 capability deviation ID 与实际支持边界 |
| [P6 v4 管理合同](../implemented/p6-cloudflare-v4-wrangler-compatibility.md) | 当前 `/client/v4`、固定 Wrangler、官方 SDK、multipart/Assets 与资源 API 声明子集 |
| [P6 本地完成记录](../implemented/p6-cloudflare-v4-wrangler-compatibility-results.md) | 实际执行的 P6 本地检查、证据与明确未验收项 |
| [P6 远端差分验收](../acceptance/p6-cloudflare-v4-differential-acceptance.md) | 仍需 Cloudflare credentials 的管理资源、SDK、Assets 与 hosted cleanup 资格 |
| [Fuzz 所有权](p1-fuzz-ownership.md) | 各类输入的测试归属和回归要求 |
| [单二进制分发与部署](single-binary.md) | 构建输入、离线启动、资源物化和发行契约 |
| [版本与发布流程](releasing.md) | 稳定版本、tag 约束、CI/release workflow、四平台 assets、校验与失败处理 |

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

runbooks 由 `ocd` 在编译时内嵌。仓库目录调整不改变 `ocd docs <name>` 的手册名称，
修改路径时必须同步资源读取和相关测试。操作权限、Day1 设计及实际支持范围仍以
[AGENTS.md](../../AGENTS.md) 和当前实现为准。
