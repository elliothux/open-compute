# P1：平台加固

> 状态：P1.0 至 P1.7 核心实现及本地验证已完成，按该范围归档；见 [P1 验证记录](./p1-results.md)。

已完成阶段的维护摘要；当前支持范围见[兼容矩阵](../references/cloudflare-compatibility.md)。

## 实现与不变量

- Capability 和 deviation 提供可查询支持范围；当前 authority 是 capability manifest 与维护中的兼容矩阵。
- 写入 admission、空间 reservation 和磁盘保护统一约束会增加持久状态的操作。
- 整机 snapshot 由离线 CLI 独占 data-dir lock；项目 SQLite 使用一致备份，DO 数据在 workerd 停止后复制。
- Snapshot manifest 固定发行、schema、对象 authority 与 key fingerprint；master key 由 operator 独立保管。
- Restore 先验证 snapshot 和对象内容，再恢复到空目标的 staging 并发布；不能覆盖现有业务目录。
- 当前开发 schema 直接按 Day1 修订；旧升级／回退设计不构成兼容承诺。恢复流程见维护中的 runbook。
- Secret hygiene、恶意输入、隔离、crash recovery 与 support bundle 由实际产品路径验证。
- P1.0–P1.7 核心实现和本地验证已完成；长时 soak、发行演练留在 [P1 验收计划](../acceptance/p1-release-acceptance.md)。
- P1.8 调查记录见 [原始结果](p1-8-results.md)，不代替当前 WebSocket capability。

## 源码入口

- [`crates/storage/src/platform_snapshot.rs`](../../crates/storage/src/platform_snapshot.rs)
- [`crates/service/src/backup_cli.rs`](../../crates/service/src/backup_cli.rs)
- [`crates/service/src/backup_retention.rs`](../../crates/service/src/backup_retention.rs)

## 验收依据

历史结论、实际命令与未验证项见[验收记录](p1-results.md)。

当前测试入口与规则见[测试手册](../references/testing.md)。
