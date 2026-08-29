# 当前 release 恢复

触发信号：当前二进制损坏、主机恢复，或 schema/运行时身份校验失败。影响面是平台数据目录、
当前 executable/runtime pin 和快照中的全部权威状态。项目没有跨历史开发版本的升级产品路径。

只读诊断：核对二进制 capability 的完整 release 身份及当前配置，不修改已有数据库：

```bash
/opt/open-compute/platformd capabilities --json
/opt/open-compute/platformd --config /etc/open-compute/platform.toml doctor --json
```

允许的 mutation：经 operator 确认后停止 service，使用同一 release 的已验证二进制，
按[全新主机恢复](fresh-host-restore.md)将经过认证的快照恢复到明确指定的新目录。
恢复前必须满足 source release、配置策略、master key、S3 authority 和完整 schema 身份校验。
不得直接覆盖、降级、自行修复或清空现有目录。发行构建、下载 runtime 和替换二进制仍需单独批准。

预期是当前 schema 被严格验证，恢复只发布完整的新目录并写入 `last-restore.json`。
未知或不匹配 schema、checksum、release/pin、配置策略、快照签名或 key 是停止条件；
不存在通过升级命令绕过检查的路径。

回滚当前数据状态同样使用同一 release 的已验证快照和全新目录，并接受该快照的明确 RPO；
恢复不会撤销快照之后的外部副作用。Worker 的部署 promote/rollback 是另一个仍然支持的产品操作，
不等同于平台历史版本升级。验证包括只读 doctor、当前产品 smoke、重启写读及 snapshot inspect；
没有实际执行的验证不得记为成功。
