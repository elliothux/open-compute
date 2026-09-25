---
title: "运行与运维"
description: "配置、监控、备份、升级和恢复一台 open-compute 主机。"
---

一个作用域内的 `ocd run` 进程可管理多个显式登记的实例。每个实例独占配置指定的数据目录、SQLite、对象权威和受监督的固定 workerd child；数据目录不得重叠。

## 日常操作

```sh
ocd instances
ocd status
ocd logs --follow
ocd restart
```

`ocd status` 报告所选 user/system 作用域的 daemon 存活状态。`ocd instances` 从控制 socket 读取实时实例状态；daemon 离线时才把显式清单项显示为 stopped。作用域锁仍被占用而控制 socket 不可用时会报错，不会误报 stopped。

使用 `ocd instance start|stop|restart <id-or-name>` 独立管理已登记实例，不改变其 `autostart` 意图。`status` 与 `instances` 只选择当前用户或显式 `--system` 作用域，不接受 `--instance` 或 `--config`。

登记多个实例后，实例级命令必须显式选择目标。例如用 `ocd --instance <id-or-name> dashboard` 打开 Dashboard。详见[实例](/zh/docs/ocd/instances/)和 [Dashboard](/zh/docs/ocd/dashboard/)。

`/health/live` 表示进程存活，`/health/ready` 表示是否可接收流量。ready 失败时先诊断再重启：

```sh
ocd doctor
ocd doctor --full
ocd capabilities --json
```

`doctor --full` 会发生显式 mutation：执行 object-storage canary，并临时编译、启动和停止 runtime。

## 配置与数据

默认 setup 归当前用户所有，配置位于 `~/.open-compute/instances/default/compute.toml`，并由显式 `[data].path` 指向旁边的 `data`。`ocd setup --system --yes` 是显式的 system 方案，配置位于 `/var/lib/open-compute/instances/default/compute.toml`。两者都把精确 config 登记进对应作用域的 `ocd.toml`；`instances/` 不是发现机制。root 未传 `--system` 会被拒绝。

secret 必须引用环境变量或 owner-only 文件，不能内联。Local object storage 是默认值；S3 是显式选择的替代 authority，不是 runtime fallback。

暴露 listener 或选择 S3 前阅读[平台配置](/zh/docs/ocd/configuration/)。公网路由由共享 [Gateway](/zh/docs/gateway/) 负责。本地原生扩展按实例登记，说明见[扩展](/zh/docs/extension/)。

## 备份、升级与恢复

备份是离线、经过认证的全平台 snapshot。停止实例后创建并验证 snapshot，并在全新 data directory 中演练恢复。参见[备份与保留](/zh/docs/ocd/backup/)和[故障手册](/zh/docs/ocd/incidents/)。

```sh
ocd upgrade --dry-run
ocd upgrade
```

upgrade 会先下载并验证目标，再让 staged target binary 校验所有 active 已登记 config，并在只读 SQLite snapshot 上试跑 migration，之后才替换已安装 binary。binary 与 receipt 的 digest-bound backup 会保留到 restart/readiness 成功；正常 restart 失败时自动恢复旧 release。如果中断留下 backup，后续 upgrade 会 fail closed，直到 operator 执行 `ocd upgrade --restore`。

upgrade 与 uninstall 处理所选 daemon 作用域中全部显式登记。已登记 config 缺失或无效会使 `ocd.toml` fail closed，并在替换 binary 前停止；清单故意不保存可供 fallback 的身份或数据副本。`ocd uninstall` 注销这些实例后删除 receipt-owned program；未显式 purge 时保留 config 与 data。

删除数据始终需要显式、不可逆的 purge：

```sh
ocd purge --instance <id> --dry-run
ocd purge --instance <id> --yes
ocd --config /exact/path/config.toml purge --yes
ocd uninstall --purge --yes
```

purge 只在所选作用域 daemon 离线时打印并验证完整计划；配置已变化、symlink、filesystem root/home、hard link 或特殊文件、daemon 仍持有作用域、与其他已登记实例共享或重叠的数据根都会被拒绝。Local objects 固定在实例数据根内，随该根删除；S3 authority 始终保留并提示手工处理。
