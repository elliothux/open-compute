# 常用命令

以 `ocd --help` 和当前二进制为准。全局选择器：

- `--config <path>` — 精确配置路径；相对值只按启动时 cwd 解析，不从 parent 或 `$HOME` 搜索；配置内部路径按配置文件的 canonical directory 解析。
- `--instance <id>` — 精确的已注册短实例 ID（与 `--config` 互斥）。
- `--no-update-check` — 本次调用跳过升级提醒与异步更新检查刷新。

`--instance` 与 `--config` 在任何文件或 registry 访问前由 Clap 拒绝同时出现。

## 配置发现

命令需要配置且未提供 `--config` / `--instance` 时，发现顺序为：

1. 启动 cwd 中精确的 `./compute.toml`；
2. `/etc/open-compute/config.toml`；
3. 否则失败，列出检查过的路径并提示 `ocd setup`。

高优先级文件存在但无法加载时 fail closed，不会回退到低优先级路径。`ocd run` 不接受 `--instance`。

全局命令（不走普通配置发现）：`--help`、`--version`、`docs`、`licenses`、`instances`、`target`、`setup`、`upgrade`、`uninstall`、`worker bundle`。`wrangler` 使用自身的精确 local-instance 或 remote-target 选择。

## `instances`

列出已注册的本机实例（system + 当前用户 registry）。JSON 含 `instance_id`、`state`、`config`、`service`。在 control socket（P11.2）落地前，列表中的状态为 `stopped`，即使前台 `ocd run` 正在运行。

```sh
ocd instances
ocd instances --json
```

## `target`

管理当前用户显式的远程 Wrangler target。add 会校验严格的 target name、规范化 HTTPS `/client/v4` URL（只有 loopback 可用 HTTP）、canonical account ID，以及绝对路径、owner-only、权限 `0600` 的 deployer-token file。registry 只存 file reference，永不保存 token value。

```sh
ocd target add company-prod \
  --api-base-url https://compute.example.com/client/v4 \
  --account-id 0123456789abcdef0123456789abcdef \
  --token-file /absolute/path/deployer.token
ocd target list [--json]
ocd target show company-prod [--json]
ocd target test company-prod [--json]
ocd target remove company-prod
```

只有 `test` 发网络请求并打开 token file。remove 保留外部 token file。

## `wrangler`

选择 open-compute authority，核对 capability 公布的 Wrangler 精确 pin，然后用最近的项目内 Wrangler 替换 `ocd`。从 Wrangler command 开始的参数原样传递。

```sh
ocd wrangler deploy --env dev
ocd --instance k7m2r wrangler tail --env staging
ocd wrangler --target company-prod --project /srv/workers/api deploy --env production
ocd wrangler -- --version
```

`--target`、全局 `--instance`、全局 `--config` 两两互斥。`--project` 同时设置 executable search root 和 child working directory；省略时两者都从调用 cwd 开始。launcher 成功后保留 TTY、signal、stdout/stderr 和 Wrangler exit status。详见 [Wrangler 项目与部署目标](/zh/workers/projects)。

## `docs`

列出或打印打进二进制的运维手册。仓库目录怎么改都不改变手册名。

```sh
ocd docs
ocd docs install-and-first-start
```

手册名（无 `.md`）：`backup-and-retention`、`collect-support-bundle`、`disk-pressure`、`fresh-host-restore`、`install-and-first-start`、`master-key-loss-and-recovery`、`s3-outage`、`scheduler-recovery`、`sqlite-corruption`、`current-release-recovery`、`workerd-crash-loop`。

站点页面是给运维读的正文；`ocd docs` 输出的是内嵌 runbook。两者命令应一致。若 runbook 示例写成 `platform.toml`，仍用你的绝对 `--config` 路径。

## `licenses`

打印打进本可执行文件的许可证（Open Compute 与内嵌 Cloudflare workerd）。

```sh
ocd licenses
```

## `capabilities`

打印版本化的产品与发行契约。使用配置发现或 `--config` / `--instance`。`--json` 输出 `schema_version`、`release`、`runtime`、`products`、`limits`。读法见[兼容性](/zh/platform/compatibility)。

```sh
ocd capabilities --json
ocd --config /etc/open-compute/config.toml capabilities --json
```

## `config init` / `config check`

```sh
ocd config init --data-dir /var/lib/open-compute
ocd config check
ocd --config /etc/open-compute/config.toml config check --json
```

`init`：相对 `--data-dir` 按启动 cwd 解析后写成绝对路径；完整 starter TOML 打到 stdout；不创建文件或 secret。成功的 JSON check 形如 `{"schema_version":1,"command":"config_check","result":"ok"}`；人类输出为 `CONFIG_OK`。省略 `--config` 时 `check` 走配置发现。

## `run`

以前台进程启动平台。首次运行在取得锁后生成身份、数据库和 master key，并物化内嵌 runtime。不接受 `--instance`。

```sh
ocd --config /etc/open-compute/config.toml run
ocd run   # 使用 ./compute.toml 或 /etc/open-compute/config.toml
```

## `doctor`

默认只读。`--full` 授权选定 object-authority canary 与临时 workerd compile/start/stop。`--json` 输出版本化报告。见[健康检查](/zh/ocd/health)。省略 `--config` 时走配置发现。

```sh
ocd --config /etc/open-compute/config.toml doctor --json
ocd --config /etc/open-compute/config.toml doctor --full --json
```

## `backup`

离线全平台快照。

| 命令                                                                       | 作用                                        |
| -------------------------------------------------------------------------- | ------------------------------------------- |
| `backup create --name <label>`                                             | 创建并完整校验已提交快照                    |
| `backup list`                                                              | 列出本平台已认证已提交快照                  |
| `backup inspect --snapshot <uuid> [--verify]`                              | 检查一个；`--verify` 哈希每个 object        |
| `backup delete --snapshot <uuid>`                                          | 删除该快照拥有的 objects；manifest 最后删   |
| `backup retention-plan --keep-last <n> [--max-age-seconds] [--keep-label]` | 仅规划；不删除                              |
| `backup cleanup-incomplete`                                                | 清理超过 grace 的未完成上传                 |
| `backup restore --snapshot <uuid>`                                         | 恢复到**空的**新 data-dir                   |
| `backup cleanup-restore --staging <uuid>`                                  | 按失败 receipt 精确清理 staging             |
| `backup attest-restore-smoke --snapshot <uuid> --passed`                   | 记录产品 smoke 已通过；不能代替实际跑 smoke |

使用配置发现或 `--config` / `--instance`；均接受 `--json`。流程见[备份与保留](/zh/ocd/backup)与[事故手册](/zh/ocd/incidents/)。

## `setup`

首次主机初始化：创建配置和 `0600` Bearer token 文件；选择立即启动时才注册实例并启动托管服务。拒绝覆盖；不依赖既有配置发现。

```sh
ocd setup --yes
ocd setup --config ./compute.toml --yes
ocd setup   # TTY 交互；非 TTY 需传 --yes
```

不带 `--config` 的 `--yes` 使用系统默认：`/etc/open-compute/config.toml`、`/var/lib/open-compute`、本地对象存储、loopback 监听、启用 Dashboard。应由未来运行服务的非 root 账户通过 `sudo ocd setup --yes` 执行；生成的 system service 仍以该非 root 账户运行。项目级 `ocd setup --config ./compute.toml --yes` 使用 user scope，data-dir 为 `./.data/open-compute`。交互 setup 当前只支持本地对象存储；S3 需显式编写 TOML。配置写入 master key 路径但不预写密钥；首次 `ocd run` 生成。

## `start` / `stop` / `restart` / `status` / `logs` / `dashboard`

由 OS service manager 管理的生命周期（Linux systemd / macOS launchd）。在线选择顺序：`--instance`、`--config`，或唯一运行中的实例。

```sh
ocd start --config /etc/open-compute/config.toml
ocd status --json
ocd stop --instance k7m2r
ocd logs --instance k7m2r
ocd dashboard --instance k7m2r
ocd instance remove --instance k7m2r   # 要求已停止；保留配置与数据
```

`start` 会在需要时注册实例、安装 unit/plist、enable 并启动；已有注册会保留持久化 ID 和 service scope。start/restart 只有在 live control socket 与 `/health/ready` 同时 ready 后才成功。前台 `ocd run` 会发布相同的 generation descriptor 与 control socket，供 status/stop 使用。

`ocd dashboard` 通过 control socket 申请一次性 login code，并打开 `/operator/#login=<code>`（或用 `--no-open` 只打印 URL）。Dashboard 将 code 兑换为短期 browser session；长期 admin token 不再写入 `sessionStorage`。

## `upgrade` / `uninstall`

正式二进制生命周期。下载权威为 GitHub Releases（`elliothux/open-compute`）；`scripts/install.sh` 在 `$PREFIX/share/open-compute/install-receipt.json` 写入不含 secret 的 install receipt。

```sh
ocd upgrade --dry-run
ocd upgrade                 # 最新稳定版
ocd upgrade 0.1.1           # 精确稳定 SemVer
ocd upgrade --no-restart    # 只替换二进制；managed 实例仍跑旧 inode 直到重启
ocd uninstall               # 只删 receipt 证明所有权的二进制与 receipt；有已注册实例则拒绝
```

`upgrade` 拒绝降级、预发布、checksum 不匹配、receipt 所有 binary 的字节被替换，以及 package-manager 安装。替换 binary 前会验证所有已注册配置，并记录当时 active 的实例；只重启这些 active 实例，等待 control socket、HTTP readiness 与目标 release version，同时保持 stopped 实例停止。某实例重启失败则停止后续重启并保留诊断。`uninstall` 永不删除配置、secret 或数据。

管理类 CLI 可从用户 cache 在 stderr 打印一行升级提醒，并在 cache 过期时用当前可执行文件的绝对路径拉起分离的 `ocd __update_check` helper。`ocd run` 永不发起网络刷新。Dashboard Platform 只检查严格更高的稳定版本并展示主机侧 `ocd upgrade [version]` 命令，不执行或轮询升级。
