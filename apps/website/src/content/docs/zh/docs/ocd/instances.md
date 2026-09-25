---
title: "实例"
description: "在一个有作用域的 ocd daemon 下登记并运行彼此隔离的 open-compute 实例。"
---

一个 `ocd` daemon 持有所选 user 或 system 作用域的共享 listener、Gateway、作用域锁和 instance registry。每个已登记实例分别拥有自己的精确 `compute.toml`、`[data].path`、InstanceId、SQLite authority、object authority、凭据、缓存，以及受监督的 workerd 与 Provider 进程。

Cloudflare-compatible `/client/v4` wire 将该身份称为 `account_id`；`ocd` CLI、Dashboard 和 open-compute 私有接口称为 `instance_id`。二者指向同一份 authority。

## Registry

`<OCD_DIR>/ocd.toml` 是唯一受管 registry，不复制 identity 或 data path：

```toml
[[instances]]
config = "instances/default/compute.toml"
autostart = true

[[instances]]
config = "/srv/open-compute/staging/compute.toml"
autostart = false
```

daemon 不扫描 `instances/`。身份和数据位置来自精确登记的配置及其 SQLite authority；已登记的数据根不能重叠。

## 创建与登记

首次主机配置会创建 daemon 作用域及第一个实例：

```sh
ocd setup --yes
```

通过运行中的 daemon 创建另一个实例：

```sh
ocd instance setup --name staging --yes
```

使用 `--config` 与 `--data-dir` 独立指定绝对路径；`--autostart=false` 与 `--start=false` 可关闭对应默认行为。登记一个已经初始化、且不应被重写的配置：

```sh
ocd --config /srv/open-compute/staging/compute.toml instance add
```

## 操作

```sh
ocd instances
ocd instance start staging
ocd instance stop staging
ocd instance restart staging
ocd instance remove staging
```

`start`、`stop` 和 `restart` 不修改持久化的 `autostart`。`remove` 会停止实例并仅移除 registry entry，配置和数据保持不变。

实例范围命令接受 `--instance <id-or-name>` 或精确的 `--config <path>`。两者均未提供时，只会选择作用域内唯一的已登记实例；零个或多个实例必须显式选择。`ocd start`、`stop`、`restart`、`status`、`logs` 等 daemon 命令作用于整个所选作用域。

继续阅读[架构与职责边界](/zh/docs/ocd/architecture/)、[配置](/zh/docs/ocd/configuration/)、[Dashboard](/zh/docs/ocd/dashboard/)和 [CLI 指南](/zh/docs/cli/)。
