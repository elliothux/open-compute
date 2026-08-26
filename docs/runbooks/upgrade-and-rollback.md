# 升级与回滚

触发信号：已批准的 N 到 N+1 release。影响面是所有 project-owned SQLite schema、facade 和 workerd pin。

只读诊断：停止 service，用旧 binary 创建并 verify rollback snapshot，再用新 binary 检查：

```bash
/opt/open-compute-old/bin/platformd --config /etc/open-compute/platform.toml backup create --name before-upgrade-20260826 --json
/opt/open-compute-new/bin/platformd --config /etc/open-compute/platform.toml upgrade check --from-snapshot 0198f000-0000-7000-8000-000000000001 --json
```

允许的 mutation：

```bash
/opt/open-compute-new/bin/platformd --config /etc/open-compute/platform.toml upgrade apply --from-snapshot 0198f000-0000-7000-8000-000000000001 --json
/opt/open-compute-new/bin/platformd --config /etc/open-compute/platform.toml doctor --full --json
```

预期是每个 DB 到达 release.json 的 target tuple 并写 `last-upgrade.json`。unsupported source、未来/混合 schema、checksum、snapshot 或 workerd compatibility 错误是停止条件。migration 一旦开始，不允许旧 binary 直接读取新 schema；回滚必须先按 fresh-host 流程恢复 pre-upgrade snapshot，再切回旧 package，并接受明确 RPO。验证包括 P0 smoke、重启写读与 snapshot inspect。
