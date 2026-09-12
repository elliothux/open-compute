# 安装与首次启动

触发信号：新主机尚未生成平台身份，或 readiness 从未成功。影响面是整台单节点平台。

优先下载并审阅正式 [`scripts/install.sh`](../../../scripts/install.sh)，然后以普通用户运行。默认 binary 位于
`/absolute/user-home/.local/bin/ocd`，不含 secret 的 receipt 位于同一 `/absolute/user-home/.local` prefix；需要时安装器更新支持的 shell rc，
否则打印一条精确的 PATH 命令。root 调用才默认使用 `/usr/local`。显式 `OPEN_COMPUTE_INSTALL_PREFIX`、
`OPEN_COMPUTE_INSTALL_DEST` 和 `OPEN_COMPUTE_RECEIPT_PATH` 始终优先。安装器会在任何 release 网络请求前预检 binary
和 receipt 目录，不创建 config、data-dir、token 或 OS service。也可手工下载 GitHub Release 资产并按
`SHA256SUMS` 校验后安装。

默认 user setup 在 Linux 使用 XDG config/data root（未设置时为 `/absolute/user-home/.config/open-compute/config.toml` 与
`/absolute/user-home/.local/share/open-compute`），macOS 使用 `/absolute/user-home/Library/Application Support/open-compute/config.toml` 与其 `data`
子目录。user service 随登录启用，不隐式开启 systemd lingering；注销后可以停止。默认 object authority 使用 Local；
只有明确选 S3 时才替换。
若 Git remote 需要被本机以外的客户端使用，必须把 `[artifacts].public_origin` 配成 operator 拥有、可从客户端访问的精确 HTTP(S) origin；默认 `http://127.0.0.1:8787` 只适合本机访问。该字段不得包含 credential、path、query 或 fragment，TLS 与反向代理策略由 operator 负责。

只读诊断：

```sh
ocd config check --json
ocd capabilities --json
```

允许的 mutation：在主机上完成首次初始化并启动托管实例：

```sh
ocd setup --yes
# 或项目目录：
ocd setup --config ./compute.toml --yes
# 需要 boot-level、privileged port 或 host-wide service 时：
curl -fsSL https://open-compute.dev/install.sh | sudo sh
sudo ocd setup --system --yes
```

首次启动取得数据目录排他锁后生成平台身份、数据库和 master key，离线解压并验证内嵌
runtime，随后打开并检查唯一 object authority、在 canary 成功后提交不可变 binding、编译系统配置并启动 workerd。Local 不启动 object server 或 rclone；运行时不安装或下载任何工具。
预期 `/health/live` 和 `/health/ready` 均成功。用 `ocd dashboard` 打开 Dashboard（一次性登录，不复制长期 token）。
`ocd stop` 只有在 service inactive、control socket 消失且 data-dir lock 已释放后才成功；此后可立即运行 offline doctor
或再次 start。30 秒内未 quiescent 会报错，不会提前输出成功。

普通 doctor 不初始化目录。需要已有数据和身份的完整诊断应在首次成功运行、正常停机后执行：

```sh
ocd doctor --full --json
```

停止条件：master key、object authority/fingerprint、runtime digest、权限或空间检查失败。不要反复生成 key、切换 backend，
也不要以外部 workerd 或重新下载绕过错误。回滚为停止进程并保留 config、key、data-dir 与 object root。
验证包括一次 smoke Worker 请求、重启后读取，以及停机后的完整 doctor。
