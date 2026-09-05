# 安装与首次启动

触发信号：新主机尚未生成平台身份，或 readiness 从未成功。影响面是整台单节点平台。

## 装这一个文件

只安装匹配 OS/CPU 的一个 `ocd` 文件到 `/opt/open-compute/ocd`，校验发行方提供的 SHA-256。没有相邻 workerd、runtime 或 share 目录要求。运行时不会安装或下载任何工具，也不要在旁边再放一份 workerd。

发行物是单文件，但首次运行会在 data-dir 里解压并校验内嵌 runtime（`data/runtime/packages/<payload-sha256>/`）。这不是「磁盘上永远只有这一个文件」。data-dir 与（macOS 上）staging 所在文件系统必须允许执行。Linux 官方 workerd 需要 glibc 2.35+；容器示例用 Ubuntu 24.04，不用 scratch/Alpine。

## 配置与账户

只读诊断与准备：`ocd config init` 向 stdout 输出模板，不初始化文件或密钥：

```sh
/opt/open-compute/ocd config init --data-dir /var/lib/open-compute > /etc/open-compute/config.toml
```

把它保存为**新的** `/etc/open-compute/config.toml`（不要覆盖已有文件）。生成的单机默认配置直接使用 `/var/lib/open-compute/objects` 下的 Local 对象存储；若要使用 S3，则把 `[storage]` variant 替换成 endpoint、bucket 与 env/file 凭据引用。配置、凭据、数据目录及 Local object root 都由专用服务账户拥有。

本站点与 README、安装手册、systemd/container 示例统一使用 `/etc/open-compute/config.toml`。相对 `--config` 只按启动 cwd 解析；生产 service definition 仍应使用绝对路径。部分内嵌 runbook 仍把示例写成 `/etc/open-compute/platform.toml`；那只是文件名示例，不是另一套配置格式。macOS launchd 示例用 `/usr/local/etc/open-compute/config.toml` 和 `/usr/local/var/open-compute`。

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml config check --json
/opt/open-compute/ocd capabilities --json
```

`--help`、`--version`、`capabilities`、`docs`、`licenses`、`config init/check` 都不物化 runtime。

## 第一次 `run`

允许的 mutation：预置专用账户、可写 data-dir 及选定的 Local 或 S3 object authority 后运行：

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml run
```

首次启动取得数据目录排他锁后生成平台身份、数据库和 master key，打开唯一 object authority，离线解压并验证内嵌 runtime，执行 object/R2 capability 检查，再提交不可变 authority binding，随后编译系统配置并启动 workerd。Local 不启动 object-server sidecar。预期 `/health/live` 和 `/health/ready` 均成功。

不要在同一 data-dir 上起第二个 `ocd`（`DATA_DIR_IN_USE`）。

## `doctor --full` 的时机

普通 `doctor` 不初始化目录。需要已有数据和身份的完整诊断，应在**首次成功运行并正常停机之后**执行：它持有数据目录排他锁，并做 object-storage canary 与临时 runtime cycle；S3 保留 provider/TLS 检查，Local 检查 format、fsync 与剩余空间。

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml doctor --full --json
```

不要在首次初始化前要求 `doctor --full` 成功。

## 停止条件与回滚

停止条件：master key、object authority/fingerprint、runtime digest、权限或空间检查失败。不要反复生成 key、切换 backend，也不要以外部 workerd 或重新下载绕过错误。回滚为停止进程并保留 config、key、data-dir 与 object root。

验证包括一次 smoke Worker 请求、重启后读取，以及停机后的完整 doctor。
