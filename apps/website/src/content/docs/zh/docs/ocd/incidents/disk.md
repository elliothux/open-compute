---
title: "磁盘压力"
---

触发信号：readiness reason 为 `DISK_SOFT` 或 `DISK_HARD`、`platform_disk_emergency_headroom_bytes` 接近零，或 mutation 返回 `storage_pressure`/507。影响面是可能扩大本地状态的 Worker、KV、D1、DO、object staging 和 snapshot。使用 Local object backend 时，objects 固定在实例数据目录的 `<data.path>/objects`。

只读诊断：

```sh
/opt/open-compute/ocd --config /var/lib/open-compute/instances/default/compute.toml doctor --json
/usr/bin/df -k /var/lib/open-compute
# Local objects 位于所选实例的数据目录下。
```

允许的 mutation 仅限已知 owner 的 delete/GC、精确 snapshot delete，以及扩容受影响的 filesystem；先停止大 upload。预期是 hard pressure 下新写 fail closed，而 reads、doctor 和 emergency cleanup 保持有界。无法辨认文件 owner、read-only filesystem、SQLite I/O error 或 emergency headroom 为零是停止条件；不要删除 DB、WAL、DO、Local object envelope、multipart journal、marker 或 lock 文件。回滚是撤销容量策略变更而不是恢复已删除 tenant data。验证是所有适用 filesystem 的 headroom 回升、doctor 通过、507 消失以及 staging/reservation 回到稳态。
