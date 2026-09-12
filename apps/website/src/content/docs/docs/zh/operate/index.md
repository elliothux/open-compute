---
title: "运行与运维"
description: "配置、监控、备份、升级和恢复一台 open-compute 主机。"
---

一个 `ocd` 进程拥有一个平台配置、一个 data directory、一个 SQLite authority、一个 Local 或 S3 object authority，以及一个受监督的固定 workerd child。禁止两个实例共用同一个 data directory。

## 日常操作

```sh
ocd instances
ocd status
ocd logs --follow
ocd dashboard
ocd restart
```

存在多个已注册 instance 时使用 `--instance <id>`，或使用 `--config <absolute-path>` 选择精确配置。两个 selector 互斥。

`/health/live` 表示进程存活，`/health/ready` 表示是否可接收流量。ready 失败时先诊断再重启：

```sh
ocd doctor
ocd doctor --full
ocd capabilities --json
```

`doctor --full` 会发生显式 mutation：执行 object-storage canary，并临时编译、启动和停止 runtime。

## 配置与数据

推荐的系统安装使用 `/etc/open-compute/config.toml` 和 `/var/lib/open-compute`。secret 必须引用环境变量或 owner-only 文件，不能内联。Local object storage 是默认值；S3 是显式选择的替代 authority，不是 runtime fallback。

暴露 listener 或选择 S3 前阅读[平台配置](/docs/zh/ocd/configuration/)。

## 备份、升级与恢复

备份是离线、经过认证的全平台 snapshot。停止实例后创建并验证 snapshot，并在全新 data directory 中演练恢复。参见[备份与保留](/docs/zh/ocd/backup/)和[故障手册](/docs/zh/ocd/incidents/)。

```sh
ocd upgrade --dry-run
ocd upgrade
```

upgrade 会验证已注册配置，并只重启升级前 active 的实例。`ocd uninstall` 只移除 receipt-owned 安装文件，永远不会删除平台配置、secret 或数据。
