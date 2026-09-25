---
title: "Master key 丢失"
---

触发信号：`master_key_mismatch`、key file 丢失或 secret decrypt canary 失败。影响面是 control 中所有加密 secret、snapshot MAC 和灾备恢复。

只读诊断：停止所选作用域的 daemon，先从 operator 独立备份取得**同一** master key，再通过已登记 `compute.toml` 的临时外部引用检查 fingerprint 是否与 snapshot/control identity 匹配。以下为 system 默认实例示例：

```sh
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml doctor --json
/opt/open-compute/ocd --system --config /var/lib/open-compute/instances/default/compute.toml backup inspect --snapshot 0198f000-0000-7000-8000-000000000001 --verify --json
```

允许的 mutation 仅是把备份的**同一** key 以 mode 0600 恢复到实例配置引用的文件，或暂用外部引用核验后再放回实例目录；不得生成新 key 覆盖旧权威。如果数据目录也丢失，按 [全新主机恢复](/zh/docs/ocd/incidents/fresh-host/) 的 S3/Local 分支执行。预期 fingerprint、decrypt canary 和 manifest MAC 同时匹配；不得把 key 写入 snapshot。没有同一 key 就停止恢复。回滚错误引用并保留 evidence；通过 doctor full、tenant secret binding 和重启验证。
