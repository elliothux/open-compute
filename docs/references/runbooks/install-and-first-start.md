# 安装与首次启动

触发信号：新主机尚未生成平台身份，或 readiness 从未成功。影响面是整台单节点平台。

优先下载并审阅正式 [`scripts/install.sh`](../../../scripts/install.sh)，然后用 `sudo sh install.sh` 完成默认的
system-wide 安装：binary 位于 `/usr/local/bin/ocd`，不含 secret 的 receipt 位于同一 `/usr/local` prefix。
安装器会在任何 release 网络请求前同时预检 binary 和 receipt 目录；权限不足时给出 system-wide 与 per-user
两条精确命令，不会先下载再暴露原始 `mkdir` 错误。无需 system service 的用户级安装可把
`OPEN_COMPUTE_INSTALL_PREFIX` 设为 operator 拥有的显式绝对 prefix（例如 `/home/operator/.local`），binary 与
receipt 仍保持在同一 prefix。也可手工下载 GitHub Release 资产并按 `SHA256SUMS` 校验后安装。安装脚本不创建配置、
data-dir、token 或 OS service。

只读诊断与准备：可先用 `ocd config init --data-dir /var/lib/open-compute` 向 stdout 输出模板并手工保存为
`/etc/open-compute/config.toml`（不要覆盖已有文件）。默认 object authority 使用 Local；只有明确选 S3 时才替换。
配置、凭据、数据目录与 Local object root 由专用服务账户拥有。
若 Git remote 需要被本机以外的客户端使用，必须把 `[artifacts].public_origin` 配成 operator 拥有、可从客户端访问的精确 HTTP(S) origin；默认 `http://127.0.0.1:8787` 只适合本机访问。该字段不得包含 credential、path、query 或 fragment，TLS 与反向代理策略由 operator 负责。

```sh
ocd --config /etc/open-compute/config.toml config check --json
ocd capabilities --json
```

允许的 mutation：在主机上完成首次初始化并启动托管实例：

```sh
sudo ocd setup --yes
# 或项目目录：
ocd setup --config ./compute.toml --yes
# 若已有配置且只需启动：
ocd start --config /etc/open-compute/config.toml
# 或前台：
ocd --config /etc/open-compute/config.toml run
```

首次启动取得数据目录排他锁后生成平台身份、数据库和 master key，离线解压并验证内嵌
runtime，随后打开并检查唯一 object authority、在 canary 成功后提交不可变 binding、编译系统配置并启动 workerd。Local 不启动 object server 或 rclone；运行时不安装或下载任何工具。
预期 `/health/live` 和 `/health/ready` 均成功。用 `ocd dashboard` 打开 Dashboard（一次性登录，不复制长期 token）。

普通 doctor 不初始化目录。需要已有数据和身份的完整诊断应在首次成功运行、正常停机后执行：

```sh
ocd --config /etc/open-compute/config.toml doctor --full --json
```

停止条件：master key、object authority/fingerprint、runtime digest、权限或空间检查失败。不要反复生成 key、切换 backend，
也不要以外部 workerd 或重新下载绕过错误。回滚为停止进程并保留 config、key、data-dir 与 object root。
验证包括一次 smoke Worker 请求、重启后读取，以及停机后的完整 doctor。
