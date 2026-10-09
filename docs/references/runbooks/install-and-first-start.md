# 安装与首次启动

触发信号：新主机尚未生成平台身份，或 readiness 从未成功。影响面是整台单节点平台。

优先下载并审阅正式 [`scripts/install.sh`](../../../scripts/install.sh)，然后以普通用户运行。默认 binary 位于
`/absolute/user-home/.local/bin/ocd`，不含 secret 的 receipt 位于运行 UID 的 `<home>/.open-compute/install-receipt.json`；需要时安装器更新支持的 shell rc，
否则打印一条精确的 PATH 命令。通过非 root 用户的 `sudo` 安装才默认使用 `/usr/local`，receipt 则位于 `/var/lib/open-compute/install-receipt.json`，并归该用户所有。显式 `OPEN_COMPUTE_INSTALL_PREFIX` 和
`OPEN_COMPUTE_INSTALL_DEST` 只改变 binary 位置，不改变 OCD 数据根。安装器会在任何 release 网络请求前预检 binary
和 receipt 目录，下载临时文件位于 `<OCD_DIR>/tmp/`；不创建 config、实例、token 或 OS service。也可手工下载 GitHub Release 资产并按
`SHA256SUMS` 校验后安装。

默认 user setup 在 Linux/macOS 均使用运行 UID 的 `<home>/.open-compute/` 作为 OCD_DIR，在其 `instances/default/compute.toml` 创建首个显式实例配置，并把 `[data].path` 写为该配置旁的 `data/`。`instances/` 不被扫描或自动登记，数据目录可改为外置绝对路径。user service 随登录启用，不隐式开启 systemd lingering；注销后可以停止。默认 object authority 使用 Local；
只有明确选 S3 时才替换。
若 Git remote 需要被本机以外的客户端使用，必须把 `[artifacts].public_origin` 配成 operator 拥有、可从客户端访问的精确 HTTP(S) origin；默认 `http://127.0.0.1:8787` 只适合本机访问。生成的 remote 路径为 `/git/<instance_id>/<namespace>/<repo>.git`，路径 ID 只选择实例，仓库 token 仍须独立授权。该字段不得包含 credential、path、query 或 fragment，TLS 与反向代理策略由 operator 负责。

只读诊断：

```sh
ocd config check --json
ocd capabilities --json
```

允许的 mutation：在主机上完成首次初始化并启动托管实例：

```sh
ocd setup --yes
# daemon 启动后，在项目目录创建额外实例：
ocd instance setup --config ./compute.toml --data-dir ./data --yes
# 需要 boot-level、privileged port 或 host-wide service 时：
curl -fsSL https://open-compute.dev/install.sh | sudo sh
sudo ocd setup --system --yes
```

首次启动取得数据目录排他锁后生成平台身份、数据库和 master key，离线解压并验证内嵌
runtime，随后打开并检查唯一 object authority、在 canary 成功后提交不可变 binding、编译系统配置并启动 workerd。Local 不启动 object server 或 rclone；运行时不安装或下载任何工具。
预期 `/health/live` 和 `/health/ready` 均成功。用 `ocd dashboard` 打开 Dashboard（一次性登录，不复制长期 token）。
`ocd stop` 只有在 service inactive、control socket 消失且 data-dir lock 已释放后才成功；此后可立即运行 offline doctor
或再次 start。30 秒内未 quiescent 会报错，不会提前输出成功。

启用公网 Gateway 时，在 `ocd.toml` 的 `[gateway]` 配置共享 `ingress_ipv4`/`ingress_ipv6`、`https_listen`、`challenge_dns_listen` 及可选 operator `[[gateway.caddy]]`；每个实例的 `compute.toml` 只在 `[public_gateway]` 声明独占 `base_domain`。先用所选实例的 `config gateway-dns-plan` 取得固定记录和端口计划。公网路径必须把 TCP 443 转发到共享 `https_listen`，并把 UDP/TCP 53 转发到共享 `challenge_dns_listen`；平台本身不占用 TCP 80。配置 DNS 后依次运行：

```sh
ocd --config ./compute.toml config gateway-dns-verify
ocd caddy validate
ocd caddy reload
ocd caddy status
```

`gateway-dns-verify` 只读验证递归解析、委派、CAA 和 challenge DNS；`caddy status` 报告同一 daemon 的 DNS、child、配置摘要和 TLS readiness。`[[gateway.caddy]]` 文件相对 `ocd.toml` 解析，只允许 operator 管理的标准 Caddyfile；其额外域名、可选 TCP 80、后端和 DNS 由 operator 负责。Gateway 证书/ACME 状态在 `<OCD_DIR>/gateway/`，不进入实例备份。公网 qualification 需要可从 Internet 到达的 TCP 443 与 UDP/TCP 53。

普通 doctor 不初始化目录。需要已有数据和身份的完整诊断应在首次成功运行、正常停机后执行：

```sh
ocd doctor --full --json
```

可选 Browser Run 需要 operator 显式准备完整 `chrome-headless-shell` 安装。仓库准备工具复制已安装浏览器，
提取同一 executable 的离线 DevTools、协议和许可证，不下载浏览器，也不执行 Playwright 浏览器安装：

```sh
bun scripts/prepare-browser.ts --source /abs/installed/chrome-headless-shell --dest /abs/prepared/browser
```

在所选实例的 `compute.toml` 中填写全部容量。以下只是配置示例，operator 按本机资源选择数值：

```toml
[browser]
max_sessions = 4
max_pending_acquires = 4
acquire_timeout_ms = 10000
command_timeout_ms = 10000
max_connections = 8
max_frontend_requests = 64
max_actions = 2
max_body_bytes = 1048576
max_result_bytes = 16777216
max_download_bytes = 16777216
max_download_files = 16
max_message_bytes = 16777216
max_queued_messages = 256
max_history_entries = 1000
history_retention_ms = 86400000

[browser.backend]
kind = "managed"
executable = "/abs/prepared/browser/chrome-headless-shell"
browser_idle_timeout_ms = 1000
shutdown_grace_ms = 100
```

若客户端在其它机器访问 Browser API，在 `[browser]` 设置 `public_origin = "https://control.example.com"`。
它必须是 operator 管理的精确 HTTP(S) origin，不能包含凭据、路径、query 或 fragment；
反向代理需把 `/client/v4/` HTTP/WebSocket 路径转发到 daemon control listener；上游 Host 使用该 listener 的地址和端口，保留独立鉴权。
平台由此生成 HTTP/HTTPS、WS/WSS 和 Live View URL，不读取请求的 Host/Forwarded 来决定外部地址。
未配置时使用绑定的 loopback control listener。Live View 路径携带实例 account ID，JWT 继续独立校验实例、generation、session 与 target。

managed 按需启动，每 instance 一个进程组，每 session 独立临时 context；生产启动保持离线，
不搜索 PATH、不接受任意 flags、不关闭 Chrome 原生 sandbox，也不叠加外层 sandbox。
平台限制 CDP 宿主文件能力，下载只能写入所分配的临时目录；字节预算每 100 ms 检查，硬容量限制由 operator 卷配额提供。
不承诺 Chrome 主进程被攻破后的宿主文件隔离。临时登录态和下载随 session/context/generation 回收而删除，不纳入持久备份。
历史元数据同时受 `max_history_entries`（1–100,000 条）和 `history_retention_ms`（1–31,536,000,000 ms）限制，
关闭、maintenance 与重启清理过期或超量 closed/lost 记录；存活及 closing session 不受该预算影响。
这两个字段必填，示例保留最多 1,000 条、24 小时；`history()` 只返回终态记录。
无法确定原因的 lost session 使用 Cloudflare 官方 GraphQL 枚举 `0 / Unknown`，不会导致整页 history 返回 501。
固定 Playwright 的下载落盘发生在浏览器端；Worker VFS 的 `Download.path()` 不等于宿主文件路径。
`saveAs()`、`createReadStream()` 的文件字节交付未提供，不能用 `acceptDownloads: true` 推断这些 API 可读到文件。
该限制及 `connectionStartTime` 的客户端类型差异已于 2026-10-08 作为 acceptance 接受；
具体影响见[Browser Run 兼容矩阵](../cloudflare-compatibility.md#browser-run)。

外部 CDP 模式用 `kind = "cdp"` 和 `url = "http://127.0.0.1:9222"` 替换整个 `[browser.backend]`，
不保留 managed 字段。它转发目标的原生行为；目标进程、profile、文件访问和隔离由 operator 管理。
原生 endpoint 不对租户暴露，生产中应由 operator 保护该入口。两个 backend 互斥；未配置可用 backend 的实例拒绝 Browser binding。
`/json` 默认使用 operator 的 `default_generation_model`；请求 `custom_ai` 时接受 1–3 个已配置 generation alias，
复用现有 Provider 客户端，按顺序尝试请求凭证，不需要另建 AI 网关。其余 Quick Actions 不依赖 AI provider。
鉴权后的 `/metrics` 以 `instance_id` 区分实例，提供 `browser_operations_total`、
`browser_operation_duration_seconds`、`browser_in_flight_operations` 和 `browser_active_sessions`。
标签只包含固定操作和结果分类；不包含 URL、session/target ID、CDP payload 或请求凭证。
caller 取消后的 native cleanup 可继续执行，in-flight 指标不代表后台清理或进程 CPU/内存。
daemon 的 `metrics.max_series` 现在至少为 857；多实例还需要为所有实例的固定 series 预留容量。
已验证客户端路径和已知 CF 差异见 [Cloudflare 兼容矩阵](../cloudflare-compatibility.md#browser-run)。

停止条件：master key、object authority/fingerprint、runtime digest、权限或空间检查失败。不要反复生成 key、切换 backend，
也不要以外部 workerd 或重新下载绕过错误。回滚为停止进程并保留 config、key、data-dir 与 object root。
验证包括一次 smoke Worker 请求、重启后读取，以及停机后的完整 doctor。
