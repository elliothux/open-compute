# P11：ocd 安装、实例与本机运维体验

状态：Day 1 产品合同与架构设计完成；P11.1–P11.4 源码已落地（配置发现、实例选择、control socket、systemd/launchd、`setup`/`dashboard` 一次性登录、`install.sh`/`upgrade`/`uninstall`、异步 update-check、Dashboard 升级轮询合同）。正式 runner 上的跨平台安装冒烟、真实 systemd/launchd 注册，以及单轮 workspace Gate / coverage 验收仍待完成；在 DoD 证据齐备前不移入 `docs/implemented/`。

本文定义 `ocd` 的安装、配置发现、交互式初始化、实例选择、系统服务、Dashboard 登录、升级与卸载体验。
目标是让一个正式发布的单文件 `ocd` 同时承担前台服务进程和本机管理 CLI 两种角色，不增加常驻 manager daemon，
不改变每个运行实例对自身配置、data-dir、SQLite、object authority 和 workerd 子进程的唯一所有权。

本文描述的是目标行为。P11.1–P11.4 源码路径已按本文落地；正式安装冒烟、真实 OS service 验收与 workspace Gate
完成前，文档仍不得宣称未验证的一行安装命令、自动更新承诺或“可管理所有实例”。现有运维文档应与本文交叉核对。

Worker 项目如何复用上游 Wrangler 进行本地开发、选择本机/远程 target 和部署，由
[P12 Wrangler 项目开发与部署体验](p12-wrangler-project-workflow.md) 细化。P11 只拥有 daemon 和本机 instance 运维边界。

## 1. 产品目标与非目标

P11 的目标：

- 提供正式安装脚本，把目标平台的单文件 `ocd` 安装到系统全局 `PATH`；
- 提供显式的 `ocd upgrade` 和 `ocd uninstall`；
- 支持当前目录 `compute.toml` 和系统配置的确定性发现，同时保留显式 `--config`；
- 使用 systemd 或 launchd 管理后台实例和开机启动，不实现传统 self-daemonize；
- 为每个配置路径派生稳定实例 ID，并列出、选择和管理多个实例；
- 在未显式选择时，让实例命令自动选择当前唯一运行实例；
- 使用 `ocd dashboard` 打开正确实例的 Dashboard，并安全完成自动登录；
- 在 Dashboard 内检查更新并执行升级：升级开始后页面轮询至成功，自开始执行起 5 分钟超时；
- 提供交互式 `ocd setup` 和采用推荐默认值的 `ocd setup --yes`；
- 在每次 CLI 调用前使用缓存给出升级提醒，并在冷却时间到期后异步刷新正式版本信息；
- 保持 daemon 启动离线、单文件发行、secret reference、数据完整性和现有安全边界。

P11 Day 1 不提供：

- Windows 服务或 Windows 安装器；
- 集群级实例发现、远程主机管理或中心控制平面；
- 自动后台更新、daemon 启动时检查更新或 runtime 下载；
- 扫描进程表后根据裸 PID 接管、停止或删除未知进程；
- 自动迁移、覆盖或删除既有配置、数据目录、数据库、object authority 或 secret；
- 通过 URL、argv、日志或注册表传递长期 admin token；
- package-manager 仓库的发布与维护。由 Homebrew、APT 等包管理器安装的副本必须继续由对应包管理器升级。

## 2. 目标命令面

```text
ocd [--no-update-check] <command> ...

ocd run [--config <path>]
ocd start [--instance <id> | --config <path>]
ocd stop [--instance <id> | --config <path>]
ocd restart [--instance <id> | --config <path>]
ocd status [--instance <id> | --config <path>] [--json]
ocd logs [--instance <id> | --config <path>] [--follow]
ocd instances [--json]
ocd dashboard [--instance <id> | --config <path>] [--no-open]

ocd setup [--config <path>] [--system] [--yes]
ocd instance remove --instance <id>

ocd upgrade [<version>] [--dry-run] [--no-restart]
ocd uninstall
```

现有 `config`、`doctor`、`capabilities`、`backup`、`support-bundle`、`scheduler`、`worker`、`docs` 和
`licenses` 命令继续存在，但按第 4 节分类后使用统一 selector。`--instance` 与 `--config` 是全局互斥参数；
Clap 在任何文件读取、registry 查询或外部操作前拒绝两者同时出现。

`ocd run` 始终以前台进程运行并直接接收 SIGINT/SIGTERM。`ocd start` 负责注册、enable 和启动 OS service。
后台模式不 fork、不写 pidfile，也不绕过 systemd/launchd 的进程组、日志、restart 和开机启动语义。

## 3. 配置发现

### 3.1 启动时顺序

未通过实例 ID 启动一个已注册实例时，配置解析顺序固定为：

1. 显式 `--config <path>`；
2. 启动 cwd 中精确的 `./compute.toml`；
3. 系统配置 `/etc/open-compute/config.toml`；
4. 均不存在时返回稳定错误，列出检查过的路径并提示 `ocd setup`。

只检查启动时的当前目录，不遍历父目录，不搜索 `$HOME`，不读取环境变量指定的隐式配置路径。显式相对路径和
`./compute.toml` 都只相对启动 cwd 解析一次。配置内部的相对路径继续相对实际打开文件的 canonical parent 解析。

若高优先级文件存在但无法安全打开、解析或校验，命令必须返回该错误，不能回退到低优先级文件。例如损坏的
`./compute.toml` 不能被系统配置静默遮蔽。

`ocd start --instance <id>` 从受信 registry 取得该实例已经登记的 canonical config path；它不重新运行文件名发现。
`ocd run` 不接受 `--instance`，避免把一个 managed service 实例意外以前台第二份进程启动。

### 3.2 文件名角色

- `compute.toml` 是当前目录自动发现的产品配置名；
- `/etc/open-compute/config.toml` 是系统级默认配置；
- `share/default-config.toml` 仍是构建时内嵌模板，不是运行时搜索目标；
- 显式 `--config` 可以使用任意文件名，文件名不改变 schema 或行为。

P11 是 Day 1 行为替换，不保留旧的隐式搜索规则、alias 文件名或双重 fallback。现有显式 `--config` 调用继续直接工作，
因为它就是新规则的最高优先级显式配置 selector。

### 3.3 `compute.toml` 命名决策

当前实现并没有默认发现 `open-compute.toml`：CLI 只接受任意文件名的显式 `--config`，运维文档已主要使用
`/etc/open-compute/config.toml`。因此 P11 实际上是新增项目目录约定，而不是为已有 runtime default 做兼容改名。

`compute.toml` 可用，但是一个通用名称，不能声称全局零冲突。截至 2026-09-07 的 exact-name 公开搜索未发现主流
开发工具把仓库根目录 `compute.toml` 定义为自动发现标准；可见占用主要是已命名空间化的专用文件，例如
[Tribunus](https://docs.tribunus.dev/getting-started/installation/compute/) 的 `~/.config/tribunus/compute.toml`、
[Mazemaker](https://mazemaker.online/architecture) 的 `~/.mazemaker/compute.toml` 以及
[Burette](https://github.com/SergeiNikolenko/Burette/blob/main/docs/configuration.md) 的
`apps/desktop/src-tauri/permissions/compute.toml`。这些不会与 P11 只读启动 cwd 的规则自然重叠。

若某个项目根目录确实已有其他工具的 `compute.toml`，`ocd` 必须把它当作高优先级配置并在 schema 不匹配时
明确失败，不能静默转向系统实例。用户可离开该目录执行、改名无关文件，或用显式 `--instance` / `--config`
解除歧义。在单机小量实例的目标下，这个可见、可恢复的冲突风险可接受，不需要保留 `open-compute.toml`
alias 或同时搜索两个文件名。

## 4. 命令分类与目标选择

### 4.1 全局命令

以下命令不选择实例，也不读取 cwd 或系统配置：

- `--help`、`--version`；
- `docs`、`licenses`；
- `instances`；
- `upgrade`、`uninstall`；
- `worker bundle`。

### 4.2 在线实例命令

`status`、`stop`、`restart`、`logs`、`dashboard` 以及后续明确声明为 online 的管理命令使用以下顺序：

1. 显式 `--instance <id>`；
2. 显式 `--config <path>`，canonicalize 后派生实例 ID；
3. 未提供 selector 且恰有一个运行实例时，选择该实例；
4. 未提供 selector 且有多个运行实例时，立即失败并输出每个实例的 ID、状态、配置路径和 listener；要求重试时传
   `--instance`，不能继续用 cwd 或系统配置消除歧义；
5. 没有运行实例时，按第 3.1 节发现 `./compute.toml` 或系统配置，并给出适合该命令的 stopped/not-started 结果。

因此，显式 selector 永远不会被当前运行实例覆盖；“唯一运行实例优先”仅适用于没有 `--instance` 和 `--config` 的调用。

### 4.3 配置命令

`config check`、带配置的 `capabilities` 等纯配置命令按第 3.1 节发现配置，不要求实例已注册或正在运行。
它们只读取并校验配置，不创建 registry、service、data-dir、secret 或 runtime 文件。

### 4.4 离线维护命令

当前需要独占 data-dir 的 `backup`、restore、scheduler recovery 等操作可以通过 `--instance`、`--config` 或第 3.1 节
选择配置，但发现对应实例正在运行时必须返回稳定错误并提示显式停止。它们不能自动停止服务，也不能把现有离线
authority 改成通过在线 daemon 代理。未来若某个操作获得单独设计和完整在线一致性合同，才可逐命令改为 online。

`doctor` 和 `support-bundle` 按实际实现能力标记为 config-only、online 或 offline，不能只为了统一表面 UX 隐式绕过
data-dir lock 或同时打开 SQLite authority。

## 5. 稳定实例 ID

实例身份只由最终打开的 canonical absolute config path 派生，不包含配置内容、版本、PID、data-dir、端口或启动时间：

```text
digest = SHA-256("open-compute/instance-id/v1\0" || canonical_config_path_bytes)
instance_id = crockford_base32(lowercase, shortest available prefix, minimum 5 characters)
```

要求：

- 同一文件通过不同相对路径或包含 symlink 的 parent 引用时得到同一 ID；配置 leaf 本身继续禁止 symlink；
- 修改配置内容不改变 ID；
- 移动配置文件有意产生新 ID，旧 registry entry 不自动重定向；
- 正常实例 ID 是 5 个小写 Crockford base32 字符，不带 `ocd-` 等前缀，例如 `k7m2r`；
- 5 个字符提供 25 bit 空间；按单机最多约 5 个实例计算，随机碰撞概率约为三百万分之一；
- registry 写入时比较完整 digest 与 canonical path；若 5 字符候选已被另一 path 占用，新实例依次扩展到 6、7 个字符，
  已存在实例的 ID 保持不变，不能覆盖或重命名；
- JSON 和人类输出使用 registry 中持久化的完整短 ID；CLI 不再额外省略或模糊匹配，避免 5 字符 ID 与碰撞扩展 ID 产生歧义；
- config path 不是 secret，但错误和 support bundle 仍按现有路径披露策略处理。

`platform_id` 是初始化后持久化平台 authority 的身份，`startup_id` 是一次进程 generation，二者都不能代替配置路径实例 ID。
实例 ID 用于本机选择和 service 命名，不进入 Cloudflare API、tenant identity 或持久化业务对象。

## 6. Registry 与运行状态

P11 增加一个安装层面的实例 registry，而不是从任意目录扫描配置或 data-dir。每条记录至少包含：

```text
schema_version
instance_id
canonical_config_path
service_scope
service_identifier
created_at
```

registry 不保存 token、credential、配置正文、PID、signed URL 或 object authority secret。system scope 使用
`/var/lib/open-compute/instances/`；user scope 使用平台对应的用户 state directory。目录和文件必须使用现有的
no-follow、owner、mode、atomic write、fsync 和 containment 规则，拒绝 symlink、宽松权限、未知 schema 和 ID/path 不一致。

每个运行进程还在受保护的 runtime directory 发布 generation descriptor 和 Unix domain control socket。descriptor 可包含
实例 ID、canonical config path、startup ID、platform ID、release identity、service scope、listener 和 readiness，但不含任何
secret。socket 使用文件权限和 peer credential 限制调用方。

`ocd instances` 组合 registry、OS service manager 状态、control socket 探针和 data-dir lock 诊断，输出：

```text
ID  STATE  VERSION  CONFIG  LISTENER  SERVICE
```

状态至少区分 `starting`、`ready`、`degraded`、`stopped`、`failed` 和 `stale`。registry 或 socket 内容不能成为向裸 PID
发信号的依据；managed 实例只通过 systemd/launchd 操作，foreground 实例通过已认证 control socket 请求优雅退出。
发现 PID 复用、socket owner 不匹配、release identity 不符或 stale descriptor 时报告并 fail closed。

“所有实例”指当前调用者可管理的 system registry 与当前用户 registry 中的实例。P11 不扫描其他用户私有目录，也不宣称
能发现从旧二进制、删除的配置或绕过 registry 启动的任意未知进程。

## 7. OS service 生命周期

Linux 使用 systemd，macOS 使用 launchd：

- system scope：systemd system unit / launch daemon，随系统启动；
- user scope：systemd user unit / launch agent，随用户会话启动；Linux boot-before-login 需要显式启用 linger，不默认修改；
- service identifier 包含完整或无碰撞编码的实例 ID；
- `ExecStart` / `ProgramArguments` 始终包含绝对 `ocd` 路径、绝对 `--config` 路径和 `run`；
- `ocd start` 在 service 定义不存在时生成、校验、原子安装并 enable，再启动；已运行时幂等返回当前状态；
- `ocd stop` 停止但保留 enable 与 registry；
- `ocd restart` 由 service manager 完成，等待新 generation readiness；
- `ocd instance remove` 要求实例已停止，删除 service 定义和 registry，但保留配置、secret 与 data-dir；
- service manager 返回成功后仍需通过 socket/HTTP readiness 核对，不能把“进程已 spawn”等同于平台 ready。

service 进程继续运行在明确的非 root 账户下。system setup 必须验证配置及其 parent 对该账户可访问，secret 文件严格
mode `0600` 并归运行账户所有。不能为了读取项目目录中的配置而把 daemon 改成 root，也不能把 secret 改成 world/group readable。

## 8. `ocd setup`

### 8.1 交互式流程

`ocd setup` 只询问会改变部署形态的常见决策：

1. system 或 user service scope；
2. 配置目标路径；
3. data-dir；
4. public/admin listener；
5. Local 或 S3 object backend；
6. 是否启用 Dashboard；
7. 是否立即注册、enable、start 并等待 readiness。

容量、超时和多数产品开关使用内嵌推荐值，不逐项提问。S3 只收集 endpoint、region、bucket、prefix 和 env/file credential
reference；不把 credential value 写入 TOML。setup 自动生成三个互不相同的高熵 Bearer token 和 master-key 目标，全部写入
受保护的 mode `0600` 文件，配置仅保存绝对 file reference。

在写入前输出不含 secret 的摘要。所有目标采用 exclusive create；任一配置、secret、service 或 registry 目标已存在时拒绝覆盖。
多文件生成使用 staging、完整静态校验、权限核对和可恢复发布顺序，失败时删除本次尚未发布的 staging，保留已经存在的任何文件。
setup 不重置数据库、不切换已有 object authority、不修复损坏状态。

### 8.2 一键推荐配置

`ocd setup --yes` 不交互，使用 system scope 推荐值：

- `/etc/open-compute/config.toml`；
- `/var/lib/open-compute`；
- direct Local object storage；
- loopback public/admin listener；
- Dashboard enabled；
- 自动生成 file-backed admin、deployer、read-only token；
- 安装并 enable system service，启动后等待 bounded readiness；
- 成功时打印实例 ID、配置路径、状态和 `ocd dashboard` 提示。

system setup 需要权限时明确失败并给出 `sudo ocd setup --yes`，不能自行弹出或隐藏 privilege escalation。默认端口、data-dir、
config 或 object root 已占用时失败并列出冲突；不能静默选择随机端口、复用另一实例的数据或覆盖文件。

项目级一键 setup 使用显式目标与 user scope，例如：

```sh
ocd setup --config ./compute.toml --yes
```

## 9. Dashboard 自动登录

`ocd dashboard` 的用户目标是无需复制 admin token 即可进入已选择实例。实现不能把长期 token 放进 URL、fragment、argv、
环境变量、日志、registry、剪贴板或浏览器持久存储。

目标流程：

1. CLI 按第 4.2 节选择一个 ready 实例；
2. CLI 通过该实例的受保护 control socket 请求一次性 login code；
3. daemon 生成随机、高熵、仅当前 startup generation 有效、最多使用一次且约 30 秒过期的 code；
4. CLI 使用平台标准 opener 打开 `/operator/#login=<one-time-code>`；argv 中只出现短期一次性 code；
5. Dashboard 向同源 endpoint 兑换当前实例的短期 browser session；
6. 成功后立即通过 `history.replaceState` 清除 fragment；
7. session 过期、daemon 重启、显式退出或权限撤销后失效。

browser session 不能等同于配置中的长期 admin token，也不能被 Cloudflare SDK token endpoint 列出。服务端必须执行同源、
CSRF、一次性消费、过期和 startup-generation 检查。Dashboard 的现有手工 token 登录可作为明确的 fallback，但不能继续把
长期 admin token 写入 `sessionStorage`；P11 应用短期 session 取代该行为。

`--no-open` 只输出可复制的一次性 URL；JSON 模式不得输出长期 token，并应明确 code expiry。非 loopback Dashboard 必须先经过
现有 admin listener 和 origin 安全策略验证，不能因为 CLI 在本机执行就放宽公开 listener 的认证。

### 9.1 Dashboard 内检查更新与执行升级

Dashboard 必须提供操作员可见的“检查更新”和“执行升级”入口，复用与 `ocd upgrade` 相同的正式 release identity、
checksum 与 package-manager-owned 拒绝规则；不能另造第二条下载通道。

行为合同：

1. **检查更新**：在已认证的短期 browser session 下，Dashboard 调用同源 operator endpoint，读取或触发与 CLI
   update-check cache 相同的正式稳定版本元数据。结果展示当前版本、可用版本（若有）和是否可升级；不得在 UI、
   network 面板约定以外泄露 token 或 release 签名材料。
2. **执行升级**：操作员确认后，Dashboard 触发与 `ocd upgrade` 等价的升级作业（默认最新稳定版，可指定精确
   SemVer）。升级仍是显式网络操作；作业由本机 `ocd` 执行原子替换与实例重启，Dashboard 不下载二进制到浏览器。
3. **轮询直到成功**：从操作员点击执行升级、服务端接受作业的时刻起，Dashboard 保持页面可用并周期性轮询同源
   升级状态 endpoint，直到状态变为成功（新 generation ready、release identity 已切换到目标版本）。
4. **5 分钟超时**：轮询截止时间为开始执行后的 5 分钟（300 秒）。超时后 UI 必须停止视为“进行中”的无限等待，
   展示超时/失败诊断，并提示操作员用 `ocd status` / `ocd upgrade --dry-run` 或 support bundle 继续排查；不得
   在超时后继续静默重试下载或自动再启动一次完整升级。
5. **重启窗口**：二进制替换并重启当前实例时，短暂 HTTP 失败是预期现象。轮询必须容忍连接拒绝/502/503，并在
   超时窗口内继续探测，直到 readiness 与版本 identity 同时满足成功条件，或达到 5 分钟截止。
6. **安全**：升级 endpoint 需要短期 browser session（或等价 admin 能力），执行 CSRF/同源检查；响应与日志不得
   包含长期 admin token。package-manager-owned 安装在检查与执行阶段都要明确失败。

验收至少覆盖：有更新/无更新的检查结果、执行后轮询至成功、执行后在 5 分钟内未 ready 的超时、升级中页面不把
长期 token 写入 URL 或 sessionStorage。

## 10. 安装、升级与卸载

### 10.1 安装脚本

正式 `install.sh`：

1. 识别支持的 OS/CPU，并拒绝未发布目标；
2. 从 `open-compute.dev` 解析到不可变 GitHub Release tag，下载该 tag 的 `release.json`、`SHA256SUMS` 和目标二进制；
3. 核对版本、target、Git revision、workerd lock/pin、文件名、大小和 SHA-256；
4. 在目标目录 staging，执行 `ocd --version` 与只读 release identity 检查；
5. fsync 后原子安装到 `/usr/local/bin/ocd`，不覆盖来源不明或其他 package manager 管理的文件；
6. 写入不含 secret 的 install receipt，记录来源、版本、摘要和安装方法。

安装脚本只安装 CLI，不隐式创建配置、data-dir、token、service 或实例；首次部署由 `ocd setup` 完成。文档可以给出一行安装命令，
同时必须提供下载、审阅、校验后执行的等价步骤。安装器不能成为第二个二进制镜像或绕过正式 release identity。

### 10.2 `ocd upgrade`

升级是用户显式发起的网络操作，daemon 正常启动和运行仍完全离线。命令：

- 默认选择最新稳定版，也可指定精确稳定 SemVer；
- `--dry-run` 只解析和验证目标 identity，输出将受影响的 binary 和实例；
- 拒绝 downgrade、预发布、未知 target、checksum/identity 不匹配和 package-manager-owned 安装；
- 下载到同一文件系统的私有 staging，先验证新 binary 和所有已注册配置，再原子替换；
- 默认依次重启 managed 实例并等待 readiness；`--no-restart` 只替换 binary 并明确报告实例仍运行旧版本；
- 某实例重启失败时停止后续重启并保留诊断，不自动用旧 binary 打开可能已被新版本接触的数据；
- 不修改配置、schema、data-dir、runtime pin 以外的发行身份或 operator 数据。

首次正式生产发布前仍遵循仓库 Day 1 schema 政策。P11 提供二进制升级通道，不因此承诺读取任意历史开发数据库或保留旧配置格式。

### 10.3 `ocd uninstall`

默认卸载行为：

- 若有 running 或 enabled managed instance，拒绝并列出实例；
- 要求先停止并使用 `ocd instance remove` 移除 service registration；
- 删除 install receipt 和由同一 receipt 证明所有权的 `/usr/local/bin/ocd`；
- 永不删除配置、secret、data-dir、SQLite、Local/S3 objects、backup 或失败证据。

数据清除不是 `uninstall` 的隐含选项。若后续提供 `purge`，必须单独设计精确 target、所有权证明、预览、确认与可恢复策略，
不能在 P11 中用一个 `--force` 顺带实现。

### 10.4 异步升级校验与提醒

每次 CLI 调用在执行实际命令前读取当前用户的 bounded update-check cache。若缓存证明存在更高的正式稳定版本，在 stderr
打印一行提示，例如：

```text
Update available: 0.1.1 (current 0.1.0). Run: sudo ocd upgrade
```

提醒不能改变命令 stdout、JSON schema 或退出码。非 TTY 调用默认不打印人类提示；需要升级信息的自动化使用稳定的结构化
`ocd upgrade --dry-run`。`upgrade` 和内部 update-check helper 不递归触发自身提醒。
参数解析成功后，`--help`、`--version` 和所有有效子命令都经过同一个 pre-command hook；未知参数等解析错误不运行 helper。

若缓存不存在或成功校验已超过 24 小时，普通管理 CLI 在继续执行当前命令的同时，使用当前 executable 的绝对路径直接
启动一个静默、detached、低优先级内部 helper；不得经 shell 或重新搜索 `PATH`：

1. helper 只从正式 GitHub Releases authority 获取 bounded release metadata，不下载二进制；
2. 连接、响应大小和总时长均有严格上限，不携带 config、instance、token、hostname 或其他本机信息；
3. 校验稳定 SemVer、release tag、`release.json` identity 和已发布 checksum 后，原子更新当前用户 cache；
4. helper 成功或失败都不阻塞、取消或改变当前命令；网络结果只在下一次 CLI 调用前展示；
5. 成功检查冷却 24 小时，失败检查至少冷却 1 小时，防止离线主机每次调用都访问网络；
6. `--no-update-check` 显式禁止本次提醒和刷新，供 air-gapped、测试和确定性自动化使用。

`ocd run`、由 systemd/launchd 启动的 daemon child 以及任何生产服务启动路径只读取已有缓存，绝不发起网络刷新或创建 updater
helper；这保持生产 startup offline。交互式 `ocd start` 可以异步刷新，因为网络行为属于调用方管理 CLI，实际 service child
仍完全离线。若当前命令很快退出，detached helper 继续独立完成 bounded metadata check，不向已退出命令的终端补写输出。

cache 是可删除的非权威提示状态，按用户存放在平台 cache directory，不进入 instance registry、data-dir、snapshot 或 support
bundle。损坏、未知版本、未来时间戳、symlink 或宽松权限 cache 一律忽略并安全重建，不能影响安装版本或 daemon admission。

## 11. 安全与失败语义

- config、registry、service definition、secret 和 binary 的发布都必须 no-follow、bounded、atomic、fsync，并拒绝覆盖非本次所有文件；
- registry 只帮助定位，不取代 data-dir flock、SQLite authority、platform identity 或 runtime generation 验证；
- 不根据 stale PID 发信号；所有生命周期操作通过已验证 control socket 或 OS service manager；
- setup/upgrade 的网络、提权、服务修改和重启在执行前输出影响范围；非交互模式仍返回机器可判定结果；
- 异步升级检查仅写非权威 cache，不阻塞命令、不自动下载/替换 binary，也不进入 daemon startup 网络路径；
- token、master key、S3 credential 和一次性 login code 不出现在长期日志、status、support bundle 或失败 receipt；
- 多实例必须拥有互异 data-dir、Local object root、public/admin listener 和 service identifier；setup/start 在 spawn 前检查确定性冲突；
- config path 移动不会自动改写 registry。用户显式 remove + start 新路径，避免两个 ID 指向一份数据；
- `ocd upgrade` 和安装脚本是唯一允许获取 `ocd` release 的路径，不得复用为 workerd 或运行时依赖下载器。

## 12. 代码与文档所有权

实施时保持直接 ownership：

- `crates/core`：`InstanceId`、canonical path 派生和稳定错误类型；
- `crates/storage`：复用安全文件、锁和 atomic write primitives，不持有 service manager 或 CLI policy；
- `crates/service`：配置/实例 selector、registry、control socket、setup、service manager adapter、Dashboard launch、upgrade/uninstall；
- `packages/dashboard`：一次性 code 兑换和短期 browser session；
- `scripts/`：最小安装脚本与 release-side 校验工具；
- `examples/systemd`、`examples/launchd`：由同一 service definition model 验证的示例，不维护第二套行为；
- `docs/references` 和 `packages/docs`：安装、CLI、配置、升级、卸载、服务和恢复 runbook；
- release workflow：发布并回读安装脚本、receipt schema 所需的 release metadata 和三个正式 binary。

不创建独立 manager crate、通用插件系统、跨机器 registry protocol 或抽象 process supervisor。只有在 systemd 与 launchd 的
差异无法由两个直接 adapter 清晰表达时，才增加窄的 service-manager trait。

## 13. 实施顺序

### P11.1：选择与身份

- 增加 `compute.toml` / system config discovery；
- 增加互斥 `--instance` / `--config` 和命令分类；
- 实现稳定实例 ID、registry schema 和 `ocd instances`；
- 更新 CLI help、错误和 JSON schema。

### P11.2：运行实例与服务

- 增加 runtime descriptor 和受保护 control socket；
- 实现 systemd/launchd adapter；
- 实现 `start/stop/restart/status/logs/instance remove`；
- 保持 `run` foreground 和原有 process ownership。

### P11.3：setup 与 Dashboard

- 实现交互式和 `--yes` setup；
- 生成 file-backed secret 和原子配置；
- 启动并等待 readiness；
- 实现一次性 Dashboard login 和短期 session；
- 实现 Dashboard 内检查更新、执行升级、轮询至成功与 5 分钟超时 UI/API。

### P11.4：分发生命周期

- 更新 release policy 和 asset manifest；
- 实现安装脚本、install receipt、upgrade、uninstall 和异步升级提醒；
- 为 Dashboard 升级入口提供与 CLI 相同的 upgrade/check API 与作业状态；
- 同步 systemd/launchd/container、英文/中文站点与内嵌 runbook。

每一阶段都先完成 focused tests 和静态检查。最终源码冻结后按仓库政策执行一次 coverage 和一次完整 workspace Gate；
安装、系统 service、升级和 Dashboard 登录必须在正式 Linux/macOS runner 上有真实进程覆盖，不能只靠字符串 snapshot。

## 14. 验收矩阵

最低回归覆盖：

- `--instance` / `--config` 互斥且在任何参数位置一致；
- 显式 config、cwd `compute.toml`、system config 和不存在时的完整优先级；
- 高优先级配置损坏时不 fallback；
- 同一路径的相对/绝对/parent-symlink 表达得到同一 ID，移动路径得到新 ID；
- registry collision、symlink、宽松权限、未知 schema 和 stale descriptor fail closed；
- 0/1/N 个运行实例的选择，N 个时输出完整 ID 并拒绝副作用；
- 两个真实实例使用不同 data-dir/listener 并行运行、重启和停止；
- PID reuse、旧 startup generation 和旧 control socket 不能被接管；
- systemd/launchd enable、boot/login start、stop、failure、日志与 orphan cleanup；
- setup 交互取消、`--yes`、目标存在、端口冲突、权限失败和中途失败均不留下半配置；
- Dashboard code 一次性、过期、跨实例、跨 generation、重放和日志/argv secret 扫描；
- Dashboard 检查更新与执行升级：有/无更新展示、轮询至成功、开始执行后 5 分钟超时、升级窗口内 HTTP 抖动可恢复；
- upgrade dry-run、checksum mismatch、错误 target、atomic replace、running-instance restart 和部分失败；
- update cache 首次缺失、fresh/stale、成功/失败冷却、离线、超时、损坏、非 TTY、`--no-update-check` 与 daemon 零网络；
- uninstall 拒绝活跃实例，成功卸载后配置、secret 与数据逐字节保留；
- daemon 冷启动无网络访问，仍只物化正式内嵌 workerd；
- 现有 config safety、data-dir lock、secret reference、restart/crash recovery 和单文件 release Gate 全部保持。

需要 privilege 的真实 system service 和安装路径测试放在隔离 CI runner；普通 workspace Gate 使用受控 fake root/service-manager
fixture 验证生成内容和失败语义，不能在开发机上隐式调用 sudo 或修改真实启动项。

## 15. Definition of Done

P11 只有同时满足以下条件才可移入 `docs/implemented/`：

1. 三个正式目标均可从正式 release 安装到全局 `ocd`，identity 与 checksum 验证通过；
2. 配置发现、selector、稳定实例 ID 和多实例歧义规则与本文一致；
3. Linux systemd 与 macOS launchd 的真实注册、开机/登录启动、停止、重启和日志验证完成；
4. `setup` 与 `setup --yes` 在全新主机路径上生成安全配置、启动并达到 readiness；
5. `ocd dashboard` 不暴露长期 admin token，并通过一次性/重放/跨实例测试；Dashboard 可检查更新并执行升级，
   升级开始后页面轮询至成功或 5 分钟超时；
6. `upgrade` 只消费正式不可变 release，验证后原子替换，并正确处理运行实例；
7. 每次人类 CLI 调用可从缓存提示新版本，异步刷新遵守冷却时间，且 daemon startup 保持零网络；
8. `uninstall` 和 `instance remove` 不删除 operator 配置、secret 或数据；
9. 所有 registry、socket、service、权限、PID reuse、crash/restart 和多实例安全回归通过；
10. 英文/中文文档、内嵌 runbook、示例、release manifest 和 CLI help 同步；
11. 静态检查、覆盖率和最终单轮 workspace Gate 按仓库政策通过，且没有遗留进程、listener、临时文件或 secret。

完成前，文档只能把这些能力标为 planned，不能发布未验证的安装一行命令、自动更新承诺或“可管理所有实例”的声明。
