# 磁盘压力

触发信号：readiness reason 为 `DISK_SOFT_LIMIT` / `DISK_HARD_LIMIT`、`platform_disk_emergency_headroom_bytes` 接近零或 mutation 返回 507。影响面是可能扩大 data-dir 的 Worker、KV、D1、DO、R2 staging 和 snapshot；Local object backend 还单独受 `[storage]` free-space 阈值约束。

只读诊断：

```bash
/opt/open-compute/ocd --config /etc/open-compute/platform.toml doctor --json
/usr/bin/df -k /var/lib/open-compute
```

若选择 Local object backend，也对 operator 配置的 object root 所在 filesystem 执行只读 `df`；该绝对路径不会出现在 health、metrics 或 support bundle 中。

允许的 mutation 仅限产品 API 已知 owner 的 delete/GC、精确 snapshot delete，以及扩容相应 filesystem；先停止大 upload。不得手删 Local envelope、multipart、marker 或 lock。预期是 hard pressure 下新写 fail closed，而 reads、doctor 和 emergency cleanup 保持有界；maintenance 会持续重测 Local object filesystem。无法辨认文件 owner、read-only filesystem、SQLite/object I/O error 或 emergency headroom 为零是停止条件。回滚是撤销容量策略变更而不是恢复已删除 tenant data。验证是两套适用 headroom 回升、doctor 通过、507 消失以及 staging/multipart/reservation 回到稳态。
