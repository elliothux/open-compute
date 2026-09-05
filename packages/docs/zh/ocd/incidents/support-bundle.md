# 收集 support bundle

触发信号：需要离线诊断 release、doctor、metrics、schema 或最近 receipt。影响面仅是本地新增一个 bounded tar；不会自动上传。

只读诊断：确认输出父目录存在、目标文件不存在且不是 symlink：

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml doctor --json
```

允许的 mutation：

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml support-bundle --output /var/tmp/open-compute-support-20260826.tar --json
/usr/bin/shasum -a 256 /var/tmp/open-compute-support-20260826.tar
```

预期 archive mode 0600，内容仅含 allowlisted release、redacted policy、doctor、metrics、schema、bounded events/receipts、文件摘要，以及脱敏后的 object backend kind、format version、authority fingerprint 和 capacity state。secret canary、size cap、目标已存在或路径不规范是停止条件；不要绕过 scanner 或手工添加 DB、DO、object payload、object path/key、request body、customer encryption key 或 provider credential。回滚是由 operator 对这个精确文件执行受控销毁。验证是校验输出 SHA-256、离线查看 entry 名，并复核不存在 secret 或 tenant object identifier。
