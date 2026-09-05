# 安装与首次启动

触发信号：新主机尚未生成平台身份，或 readiness 从未成功。影响面是整台单节点平台。

只安装匹配 OS/CPU 的一个 `ocd` 文件到 `/opt/open-compute/ocd`，
校验发行方提供的 SHA-256。没有相邻 workerd、runtime 或 share 目录要求。

只读诊断与准备：`ocd config init --data-dir /var/lib/open-compute` 向 stdout 输出模板。
把它保存为新的 `/etc/open-compute/config.toml`（不要覆盖已有文件）。默认 `[storage]` 直接使用
`/var/lib/open-compute/objects` 的 Local authority；只有明确选 S3 时才替换成 endpoint、bucket 和 env/file 凭据引用。
配置、凭据、数据目录与 Local object root 由专用服务账户拥有。

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml config check --json
/opt/open-compute/ocd capabilities --json
```

允许的 mutation：预置专用账户、可写 data-dir 与选定的 Local/S3 object authority 后运行：

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml run
```

首次启动取得数据目录排他锁后生成平台身份、数据库和 master key，离线解压并验证内嵌
runtime，随后打开并检查唯一 object authority、在 canary 成功后提交不可变 binding、编译系统配置并启动 workerd。Local 不启动 object server 或 rclone；运行时不安装或下载任何工具。
预期 `/health/live` 和 `/health/ready` 均成功。

普通 doctor 不初始化目录。需要已有数据和身份的完整诊断应在首次成功运行、正常停机后执行：

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml doctor --full --json
```

停止条件：master key、object authority/fingerprint、runtime digest、权限或空间检查失败。不要反复生成 key、切换 backend，
也不要以外部 workerd 或重新下载绕过错误。回滚为停止进程并保留 config、key、data-dir 与 object root。
验证包括一次 smoke Worker 请求、重启后读取，以及停机后的完整 doctor。
