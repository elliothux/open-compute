# 安装与首次启动

触发信号：新主机尚未生成平台身份，或 readiness 从未成功。影响面是整台单节点平台。

只安装匹配 OS/CPU 的一个 `platformd` 文件到 `/opt/open-compute/platformd`，
校验发行方提供的 SHA-256。没有相邻 workerd、runtime 或 share 目录要求。

只读诊断与准备：`platformd config init --data-dir /var/lib/open-compute` 向 stdout 输出模板。
把它保存为新的 `/etc/open-compute/config.toml`（不要覆盖已有文件），设置 S3 endpoint、
bucket 和 env/file 凭据引用。配置、凭据与数据目录由专用服务账户拥有。

```sh
/opt/open-compute/platformd --config /etc/open-compute/config.toml config check --json
/opt/open-compute/platformd capabilities --json
```

允许的 mutation：预置专用账户和可写 data-dir，配置 S3 authority 后运行：

```sh
/opt/open-compute/platformd --config /etc/open-compute/config.toml run
```

首次启动取得数据目录排他锁后生成平台身份、数据库和 master key，离线解压并验证内嵌
runtime，随后检查 S3、编译系统配置并启动 workerd。运行时不安装或下载任何工具。
预期 `/health/live` 和 `/health/ready` 均成功。

普通 doctor 不初始化目录。需要已有数据和身份的完整诊断应在首次成功运行、正常停机后执行：

```sh
/opt/open-compute/platformd --config /etc/open-compute/config.toml doctor --full --json
```

停止条件：master key、S3 authority、runtime digest、权限或空间检查失败。不要反复生成 key，
也不要以外部 workerd 或重新下载绕过错误。回滚为停止进程并保留 config、key 和 data-dir。
验证包括一次 smoke Worker 请求、重启后读取，以及停机后的完整 doctor。
