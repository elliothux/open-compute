# Master key 丢失

触发信号：`master_key_mismatch`、key file 丢失或 secret decrypt canary 失败。影响面是 control 中所有加密 secret、snapshot MAC 和灾备恢复。

只读诊断：停止 service，并检查 recovery key 的 fingerprint 是否与 snapshot/control identity 匹配：

```sh
/opt/open-compute/ocd --config /etc/open-compute/recovery.toml doctor --json
/opt/open-compute/ocd --config /etc/open-compute/recovery.toml backup inspect --snapshot 0198f000-0000-7000-8000-000000000001 --verify --json
```

允许的 mutation 仅是把 operator 独立备份的同一 key 以 mode 0600 放到 `/etc/open-compute/recovery-master.key`，然后按 [fresh-host restore](/zh/ocd/incidents/fresh-host)。预期是 fingerprint、decrypt canary 和 manifest MAC 同时匹配；不得生成新 key 覆盖旧 platform，也不得把 key 写入 data-dir snapshot。停止条件是没有同一 key：无法恢复。回滚是移除错误 key 引用并保留 evidence。验证是 decrypt canary、manifest MAC、doctor full、tenant secret binding 与重启。
