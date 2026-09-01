# S3 故障

触发信号：readiness/degraded reason、S3 operation error、artifact/R2/snapshot timeout。影响面包括 Worker bundle cold activation、R2、immutable backup 和整机 snapshot；已缓存且已验证的对象可能继续服务。

只读诊断：

```bash
/opt/open-compute/ocd --config /etc/open-compute/platform.toml doctor --json
/opt/open-compute/ocd --config /etc/open-compute/platform.toml backup list --json
```

允许的 mutation 是恢复同一 endpoint/region/bucket/prefix 的网络或 credential 权限，再显式执行 `doctor --full --json` canary。不得临时切换 provider/prefix，也不得把失败 upload 当作 committed。预期是错误稳定、重试有界、manifest 未提前出现。authority fingerprint 变化、bucket 丢失、短读或 checksum 错误是停止条件。回滚是恢复原 credential/config。验证包括 S3 canary、immutable sample、R2 marker 和 snapshot verify。
