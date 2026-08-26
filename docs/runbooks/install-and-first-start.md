# 安装与首次启动

触发信号：新主机尚未生成平台身份，或 `/health/ready` 从未进入 `ready`。影响面是整台单节点平台。

只读诊断：确认发行目录来自同一个已校验包，然后运行：

```bash
/opt/open-compute/bin/platformd --config /etc/open-compute/platform.toml config check --json
/opt/open-compute/bin/platformd --config /etc/open-compute/platform.toml capabilities --json
```

允许的 mutation：预置 `/var/lib/open-compute` 的专用 owner、在 `/etc/open-compute/platform.toml` 配置同一 S3 authority，并运行：

```bash
/opt/open-compute/bin/platformd --config /etc/open-compute/platform.toml doctor --full --json
/opt/open-compute/bin/platformd --config /etc/open-compute/platform.toml run
```

预期输出是 config `ok`、doctor 所有 required check 为 `ok`，随后 readiness 为 `ready`。任何 master-key、S3 authority、runtime digest 或权限错误都是停止条件；不要反复生成 key。回滚是停止进程并保留原 data-dir、config 和 key。验证包括 `/health/live`、`/health/ready`、一次 reserved smoke Worker 请求和一次重启后读取。
