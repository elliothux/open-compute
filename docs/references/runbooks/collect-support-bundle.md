# 收集 support bundle

触发信号：需要离线诊断 release、doctor、metrics、schema 或最近 receipt。影响面仅是本地新增一个 bounded tar；不会自动上传。

只读诊断：确认输出父目录存在、目标文件不存在且不是 symlink：

```bash
/opt/open-compute/platformd --config /etc/open-compute/platform.toml doctor --json
```

允许的 mutation：

```bash
/opt/open-compute/platformd --config /etc/open-compute/platform.toml support-bundle --output /var/tmp/open-compute-support-20260826.tar --json
/usr/bin/shasum -a 256 /var/tmp/open-compute-support-20260826.tar
```

预期 archive mode 0600，内容仅含 allowlisted release、redacted policy、doctor、metrics、schema、bounded events/receipts 和文件摘要。secret canary、size cap、目标已存在或路径不规范是停止条件；不要绕过 scanner 或手工添加 DB、DO、bundle、request body、key/credential。回滚是由 operator 对这个精确文件执行受控销毁。验证是校验输出 SHA-256、离线查看 entry 名并复核无 secret。
