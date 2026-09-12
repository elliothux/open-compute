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

`ocd stop` 在 operator boundary 是同步的：只有 service manager 已报告 inactive、control socket 已消失且 data-directory lock 可取得时，才输出 `INSTANCE_STOPPED`。超过 bounded timeout 仍未 quiescent 会报错，因此成功返回后立即执行 `doctor` 或 `start` 不会与旧进程竞争。

存在多个已注册 instance 时使用 `--instance <id>`，或使用 `--config <absolute-path>` 选择精确配置。两个 selector 互斥。

`/health/live` 表示进程存活，`/health/ready` 表示是否可接收流量。ready 失败时先诊断再重启：

```sh
ocd doctor
ocd doctor --full
ocd capabilities --json
```

`doctor --full` 会发生显式 mutation：执行 object-storage canary，并临时编译、启动和停止 runtime。

## 配置与数据

默认 setup 归当前用户所有。Linux 使用 `$XDG_CONFIG_HOME/open-compute/config.toml` 与 `$XDG_DATA_HOME/open-compute`，未设置时分别落到 `~/.config` 与 `~/.local/share`；macOS 使用 `~/Library/Application Support/open-compute/config.toml` 及其 `data` 子目录。`ocd setup --system --yes` 是显式的 system 方案，使用 `/etc/open-compute/config.toml` 和 `/var/lib/open-compute`。root 未传 `--system` 会被拒绝。

secret 必须引用环境变量或 owner-only 文件，不能内联。Local object storage 是默认值；S3 是显式选择的替代 authority，不是 runtime fallback。

暴露 listener 或选择 S3 前阅读[平台配置](/docs/zh/ocd/configuration/)。

## 备份、升级与恢复

备份是离线、经过认证的全平台 snapshot。停止实例后创建并验证 snapshot，并在全新 data directory 中演练恢复。参见[备份与保留](/docs/zh/ocd/backup/)和[故障手册](/docs/zh/ocd/incidents/)。

```sh
ocd upgrade --dry-run
ocd upgrade
```

upgrade 只验证并重启属于当前 executable、且升级前 active 的 registration。已停止且 config 缺失、变更或无效的 registration 会报告 ID、path、错误码和精确 `instance unregister` 命令，但不阻止替换 binary；active registration 无效则在替换前失败并给出同样可操作的身份信息。`ocd uninstall` 只停止和注销 owned instance，删除 receipt-owned program，并逐项打印保留的 config、data 和 object path；属于另一个 executable 的 registration 不会被修改。

删除数据始终需要显式、不可逆的 purge：

```sh
ocd purge --instance <id> --dry-run
ocd purge --instance <id> --yes
ocd --config /exact/path/config.toml purge --yes
ocd uninstall --purge --yes
```

purge 会在停止 service 前打印并验证完整计划；配置已变化、symlink、filesystem root/home、hard link 或特殊文件、仍存活的 control socket、与其他 registered instance 共享或重叠的 root 都会被拒绝。只有能证明唯一 ownership 的 Local object root 会被删除；S3 authority 始终保留并提示手工处理。
