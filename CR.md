# P11 当前分支代码审查

审查基准：`docs/implemented/p11-ocd-operator-experience.md`、仓库 `AGENTS.md` 的 Day1 / 单机 SMB 约束，以及 `simplify` 技能。范围为当前工作树中的代码与文档变更。

初始审查仅做静态 review，未运行测试、coverage 或 Gate。2026-09-08 收到“按建议修复”后进入修复阶段；下方问题正文保留
初始证据，实际处理状态以“修复记录”为准。

## 2026-09-08 修复记录

本轮按个人 / 20 人以内团队的单机 self-deploy 优先级处理默认路径与低成本收敛项，没有为低概率并发或同 UID 本地攻击
引入额外协调器、持久化 job 系统或通用抽象。

| 状态 | 编号 | 处理结果 |
| --- | --- | --- |
| **已修复** | 1 | system service 固定使用经 `SUDO_USER` / UID / GID 校验的非 root 账户；system config 与 registry 保持 root 写、service 只读，`0600` secret/data-dir 归运行账户；user systemd 使用 `default.target`。 |
| **已修复** | 3 | managed daemon 与 `start --instance` 复用 registry 的持久化 ID、scope 和 service user，不再从路径平行推导。 |
| **已修复** | 4a、4b、5 | descriptor 从 `starting` 开始，随 supervisor 状态更新；start/restart 必须通过 live socket + HTTP readiness，upgrade 还核对目标 release；生产不接受 stale descriptor；control publish/poll 失败 fail closed。 |
| **已修复** | 6a、21 | setup 使用本次发布 ledger；激活前失败回滚本次文件、registry 和 service；选择不立即启动时不注册。启动已尝试后保留完整可重试安装，避免删除可能已经初始化的 data-dir authority。 |
| **已修复** | 8 | service definition 拒绝覆盖不同内容；systemd/launchd stop/restart/uninstall 状态转换收敛，关键 manager 命令失败向上传播。 |
| **已修复（简化）** | 10、17、23 | 删除 Dashboard upgrade mutation、进程内 job store、status polling 和 SDK start/status；Dashboard 只做版本检查并提示主机执行 `ocd upgrade`。 |
| **已修复** | 11 | binary mutation 前验证所有注册配置并冻结 active 集合；只重启原 active 实例、保持 stopped，逐个等待 socket + HTTP + target release readiness。 |
| **已修复（当前范围）** | 12 | destructive upgrade/uninstall 前验证 receipt path、regular/non-symlink 文件和当前 binary 完整 SHA-256；更窄的校验后路径替换 TOCTOU 留在低优先级加固。 |
| **已修复** | 14a、14b | release HTTP 支持 bounded redirect，并用同一总 deadline 包住 redirect、headers 和 body collection。 |
| **已修复** | 19 | `instances` / `status` 共用 live inspection，区分 starting/ready/degraded/stopped/failed/stale，manager 错误不再伪装成 stopped。 |
| **已修复** | 25、31 | update cache 拒绝错误 owner/mode 与未来时间；instance ops 统一调用 `InstanceRecord::instance_id()`。 |
| **部分修复** | 26、29、37 | newer 比较已统一，文档已按真实实现降级并删除 Dashboard 执行声明；完整 cache authority 合并、example/generated service template 统一仍延期。 |
| **明确延期** | 2、6b、7、9、13、15、16、18、20、22、24、27、28、30、32～39（除 31） | 维持原优先级判断；其中公开支持缺口已在 P11 文档明确收窄，低概率并发与纯 simplify 项不阻塞本轮单机主路径修复。 |

最终验证已完成：`bun run build`；fmt、Clippy、no-default-features、Rust 1.98 MSRV、metadata、dependency boundaries、
conformance 与 `git diff --check` 全部通过；coverage 为 **90.0047%**；源码冻结后的单轮 workspace Gate 通过，报告见
`.temp/gate-run/20260908T042240-09ec8be2/report.json`。第一次最终 Gate 暴露 login-code 并发测试只等待 500ms 的调度假设，
修正为 bounded completion wait 后 focused test、coverage 与最终 Gate 均通过；失败证据保留在
`.temp/gate-run/failed/20260908T035356-6b47f334/`。

## 结论

按 open-compute 的实际用户画像重新标定后，不能把所有问题都视为同一优先级。目标用户是个人，或团队不超过 20 人的小企业，通常只维护一台 self-hosted 主机：并发管理操作少、实例数少、操作者通常已拥有主机权限，也没有多节点协调需求。因此，并发 registry 写入、同 UID 本地攻击、罕见时间戳损坏和纯代码整洁问题都不应阻塞首轮交付。

本轮列出的默认路径高优先级问题已经修复，本地实现可以恢复 **Implementation GO**。修复没有为单机部署引入
持久化 Dashboard upgrade job、多进程 registry 协调器或跨节点状态机；相反，直接删除了 Dashboard 自升级半实现，
以 `ocd upgrade` 作为唯一 mutation authority。

剩余项按公开范围明确降级或延期：真实 systemd/launchd、正式 Release 三目标安装、全新主机真实 daemon 和双实例并行
属于外部资格证据，不由 fake/local Gate 冒充；registry/setup 的 no-replace TOCTOU、peer credential、完整 logs follow、
交互式 S3 和额外冲突预检仍按下表的中低优先级处理。它们不阻塞当前个人/小团队单机主路径，但也没有被写成已完成。

没有发现运行时继续自动搜索 `platform.toml` / `open-compute.toml` 的 alias 分支；手工 admin token 登录是 P11 文档明确允许的 fallback，因此本身不作为 Day1 违规。但下面列出的 descriptor/cache fallback、测试 stub 和双 service-template 路径没有同类合同依据。

## 面向单机 self-deploy 的优先级重评

优先级含义：

- **高**：默认路径或普通操作可稳定触发；会造成错误成功、服务不可管理、升级不可用、安全边界明显弱化，或数据/安装 authority 不一致。应在宣称 P11 完成前修复。
- **中**：属于公开支持路径，但需要特定配置、故障或较少使用的功能才触发。可以分阶段修复；若延期，必须收窄支持声明并给出安全、可恢复的失败语义。
- **低 / 可延期**：需要极低概率并发、本机同权限恶意行为、手工篡改或非标准布局；或者只是维护性简化。记录限制即可，不值得为单机 SMB 场景引入复杂状态机。

| 编号 | 重评优先级 | 预计成本 | 单机场景判断 |
| --- | --- | --- | --- |
| 1 | **高** | 中 | `setup --yes` 的默认 system scope 就会走到；以 root 运行不是长尾。 |
| 2 | 中 | 低～中 | `0600` socket 已挡住其他普通 UID；风险主要在权限配置错误、root 或未来放宽访问时。仍应补真实 peer credential，但不必先于默认生命周期问题。 |
| 3 | 中 | 中 | 默认 `/etc/open-compute/config.toml` 且无 hash collision 时正常；自定义 system path 或碰撞扩展 ID 才触发。碰撞部分是低概率，scope 漂移是明确支持路径。 |
| 4a：过早 ready / 无 HTTP probe | **高** | 中 | 每次 start/restart 都会走到，可稳定错误报告成功。 |
| 4b：stale descriptor fallback | 中 | 低 | 需要 crash/stale 文件；单机仍可能出现，但可在第一轮删掉 fallback，成本低。 |
| 5 | 中 | 低 | control socket 创建失败不是正常 happy path，但一旦发生会留下“服务运行、CLI 失效”的难诊断状态。 |
| 6a：setup rollback | **高** | 中 | 首次安装最容易遇到权限、service manager 或 readiness 失败；半配置会直接伤害小团队恢复体验。 |
| 6b：check-then-copy 并发覆盖 | **低 / 可延期** | 中 | 需要另一进程在极窄窗口创建同一路径；与用户举例的并发 registry 风险同类。可以先改成仅对明确 EXDEV fallback，完整 no-replace primitive 后做。 |
| 7a：多实例资源冲突预检 | 中 | 中 | 个人通常单实例；但小团队部署 2～5 个实例是 P11 明确场景。现有端口 bind/data-dir lock 能 fail closed，故不是最高优先级。 |
| 7b：交互式 S3 setup | 中 | 中 | Local 是默认且最常见；S3 对 SMB 有价值，但可以明确标为 CLI setup 暂不支持，保留手工配置路径，而不是假装完成。 |
| 8 | **高** | 中 | macOS stop/restart 是确定性错误；重复 `start` 覆盖 unit 也是普通运维动作，不依赖高并发。 |
| 9a：registry TOCTOU | **低 / 可延期** | 中 | 需要两个并发 `start/setup` 命中同一短 ID/路径窗口；单机小团队概率极低，现有 data-dir lock 还会阻止两个 daemon 同时拥有数据。应记录限制，不应为此引入复杂协调器。 |
| 9b：registry owner 校验 | 中低 | 低 | 需要本机权限/目录 ownership 异常；补 UID 校验成本不高，但不是首轮 blocker。 |
| 10 | **高** | 高 | 每次从 Dashboard 升级当前实例都会丢 job，功能主路径不成立。若修复成本不合算，应直接撤回 Dashboard 执行升级，只保留 CLI upgrade，而不是建设复杂持久化工作流。 |
| 11 | **高** | 中 | 每次正常 upgrade 都不等 readiness；单实例也会错误提前成功。多实例中启动 stopped 实例虽较少见，但同一根因可一起修。 |
| 12 | 中 | 低 | 需要 binary 在 receipt 生成后被替换/篡改；概率不高，但 digest 已存在且验证成本低，属于应顺手补齐的 destructive-action guard。 |
| 13 | 中 | 中～高 | 需要磁盘满、权限变化或 fsync/receipt 写失败；单文件可人工重装恢复，不值得做分布式事务，但需要一个简单本地 recovery receipt/旧 inode 方案。 |
| 14a：GitHub redirect | **高** | 低～中 | 正式 GitHub asset 下载的常见行为，CLI upgrade 很可能稳定失败。 |
| 14b：body 总时长 | 中低 | 低 | 只在慢速/恶意服务端触发；给 body collection 套同一 deadline 即可，不需要复杂网络层。 |
| 15 | 中 | 中 | 正常正式发布通常由可信 release workflow 生成，checksum 已覆盖主要字节风险；缺失的 identity/精确版本校验属于供应链加固，应在首个正式 release 前完成。 |
| 16 | 中低 | 低 | 只影响显式禁用 Dashboard 的部署；socket 仍是本地 `0600`。能力边界应收紧，但不先于默认 Dashboard 路径。 |
| 17 | 低 | 低～中 | 需要已授权 admin/browser session 发送 malformed mutation，且跨站拿不到 sessionStorage token。严格 request parsing 值得做；复杂 credential-kind CSRF 设计可延期或通过把升级移出 v4 API 简化。 |
| 18 | 低（开发安全） | 低 | 不影响部署运行，只会在开发机 Gate 与真实实例并存时破坏 control 目录。修复便宜，应改，但不属于产品 P11 blocker。 |
| 19 | **高** | 中 | `instances` 在所有正常调用中都输出假状态，是 P11 最核心的日常运维界面。 |
| 20 | 中 | 中 | logs 对小团队排障很重要，但可先保证 recent logs；`--follow` 可明确延期。macOS 当前完全不可用，完成声明必须收窄。 |
| 21 | 中低 | 低 | 仅影响交互 setup 中选择“不立即启动”的分支；修复直接且不需要新架构。 |
| 22 | 低 | 低 | session 短期、只存当前 tab；logout 后残余 TTL 风险有限。增加 revoke endpoint 是低成本完善，不是首轮 blocker。 |
| 23 | **高** | 低 | 每次 Dashboard upgrade polling 都可能触发请求风暴；修复只是稳定依赖/串行 timer。 |
| 24 | 低 / 可延期 | 中 | metadata helper 不处理 secret，也不自动升级；继承环境和未完全 detach 的实际影响有限。不要为了它引入重型 daemon/updater。 |
| 25 | 低 / 可延期 | 低 | 需要本机 cache 被手工篡改或时钟大幅回拨；cache 非权威且可删除。简单拒绝未来时间/mode 即可。 |
| 26 | 中 | 中 | 离线主机很常见，Dashboard 会经常走 cache fallback；旧 cache 把 downgrade 显示成 update 是可见错误。统一比较必须做，完整 authority 重构可稍后。 |
| 27 | 中 | 中 | 不立即破坏单机运行，但会造成 API/SDK 漂移。可用更简单的 operator endpoint 取代扩展 v4 surface，避免扩大正式 Cloudflare 合同。 |
| 28 | 低 / 可延期 | 低～中 | 只有非标准 `OPEN_COMPUTE_RECEIPT_PATH` 布局触发。更符合 Day1 的做法是删除该 override、收窄到一个正式布局。 |
| 29 | 中 | 低 | 小团队高度依赖文档，没有专职运维来识别过期说明。直接修正文档成本低，但不应单独阻塞代码迭代。 |
| 30 | 低（变更管理） | 低 | 不影响最终用户；应拆分以降低 review/Gate 成本。 |
| 31 | 中低 | 低 | 只有 registry 损坏/篡改时行为差异明显，但改用已有 `record.instance_id()` 很便宜。 |
| 32～39 | 低 / simplify | 低～中 | 主要是 bundle/API surface、重复代码和维护成本；不应抢占默认路径修复，但在触碰对应模块时顺手收敛。 |

### 建议本轮真正阻塞 P11 完成的集合

优先修：**1、4a、6a、8、10、11、14a、19、23**。

其中 10 的成本最高。针对单机 self-deploy，推荐的简单选择是：让 `ocd upgrade` 继续作为唯一执行升级的 authority，Dashboard 只显示版本并引导执行 CLI；等有一个不会被自身重启杀死的本地 helper 设计后，再恢复 Dashboard 一键执行。不要为了保留 UI 按钮引入持久化分布式 job machinery。

随后以低成本补齐：**4b、5、12、17 的 strict JSON、21、22、25、31**。

可以明确记录并延期：**6b、9a、24、28、32～39**。7a、7b、20、26、27 若延期，应同步收窄 P11 的“已实现”声明。

## 详细问题 1～18

### 1. system scope 服务会以 root 运行，且 user systemd unit 的启动 target 不正确

位置：`crates/service/src/service_manager.rs:106-149`、`crates/service/src/setup.rs:278-374`

`render_systemd_unit()` 不生成 `User=` / `Group=`，system unit 因而默认以 root 运行；system launch daemon 也没有 `UserName`。与此同时，system setup 创建的 `0600` secret 归执行 setup 的 root 所有，代码没有收集、校验或设置非 root service account。这个实现用 root 运行掩盖了 secret ownership 没有落地的问题，直接违反 P11 §7 和仓库单进程安全边界。

同一个 systemd 模板还无条件使用 `WantedBy=multi-user.target`；user unit 应绑定用户 manager 的 target（通常是 `default.target`），否则 `systemctl --user enable` 不保证登录时启动。

应让 service definition 按 scope 生成，system scope 明确且校验非 root 账户、配置 parent 和 secret ownership；user scope 使用正确的用户 target。

### 2. control socket 的 peer credential 校验是空实现

位置：`crates/service/src/instance_control.rs:599-625`

Linux 和 macOS 分支都直接返回当前进程 UID，没有读取连接方 UID。`authorize_peer()` 因而永远通过，注释中的 SO_PEERCRED / `getpeereid` 只是计划而不是实现。control socket 可以签发 Dashboard login code 和触发 shutdown，这不能以注释占位。

应在 Linux 使用 SO_PEERCRED，在 macOS/BSD 使用 `getpeereid`（或当前依赖提供的等价安全 API），取不到 credential 时 fail closed；不要保留“以后补 libc”的兼容分支。

### 3. daemon 没有使用 registry 中持久化的实例 ID 和 scope

位置：`crates/service/src/run.rs:757-783`

`ocd run` 每次都从配置路径重算默认 5 字符 ID，并用路径是否位于 `/etc/open-compute/` 猜 scope。它完全没有读取 registry：

- 碰撞后持久化的 6/7 字符 ID 会被 daemon 降回 5 字符，service manager 等待的 socket 永远不会出现；
- `ocd setup --system --config <自定义路径>` 注册为 system scope，但 daemon 会发布到 user runtime root；
- descriptor、service identifier 和 CLI selector 不再指向同一 generation。

应由 registry record 成为 managed run 的 ID/scope authority，或把已验证的持久化身份通过不含 secret 的明确 service 参数传入；不能重新推导一个平行身份。

### 4. readiness 在 workerd 启动前就发布为 ready，且等待逻辑没有执行要求的 HTTP probe

位置：`crates/service/src/run.rs:772-812,995-1055`、`crates/service/src/instance_ops.rs:192-228`

descriptor 在 `supervisor.start()` 之前写成 `ready`；Dashboard bootstrap 还要在 supervisor running 后异步执行。`wait_until_instance_ready()` 只检查 control descriptor 的字符串，不探测 `/health/ready`，所以 `setup/start/restart` 可以在 runtime 尚未可用时报告成功。

更严重的是，socket 没有回应时它会读取磁盘 `descriptor.json`；旧进程崩溃留下的 ready descriptor 也会立即通过。这是为了 fake manager 引入生产逻辑的测试 fallback，既不 fail closed，也违反 P11 对 socket + HTTP readiness 的双重要求。

应先发布 `starting`，随 health/supervisor 状态更新 descriptor；只有 live socket 身份验证成功且对应 HTTP readiness 成功时才返回 ready。fake manager 应通过 test-support seam 驱动同一协议，不应让生产代码接受 stale 文件。

### 5. 必需的 control plane 发布失败后 daemon 仍继续运行

位置：`crates/service/src/run.rs:784-812`

`InstanceControl::publish()` 失败只记录 warning 并设为 `None`，daemon 仍继续启动；轮询中的任何 `poll_once()` 错误也被丢弃。结果是平台可能提供业务流量，但 `start` 最终超时，`status/dashboard/foreground stop` 全部失效，留下“服务已运行、命令报失败”的半状态。

P11 已把 control socket 定义为唯一生产运维路径。启动阶段无法建立它应 fail closed；运行中失效至少要进入明确 degraded/failed 状态并保留诊断，不能静默丢错。

### 6. setup 不是 rollback-safe，并且 fallback copy 可覆盖并发创建的目标

位置：`crates/service/src/setup.rs:315-374,468-524`

三个 secret 依次发布，最后才发布 config；之后再注册、安装、enable、start、等待 readiness。任一步失败都不会回滚本次已经创建的 secret/config/registry/service。典型结果包括：只有部分 secret、已有 config 但没有 registry、registry 存在但没有 service、service 已运行但 setup 返回失败。

`publish_file()` 先 `refuse_existing()`，再尝试 hard-link；hard-link 失败后 `fs::copy()` 会默认截断一个在检查后并发创建的普通文件。它也把“跨文件系统”以外的 hard-link 错误全部降级为 copy，掩盖真实权限/安全错误。

应设计明确的 staged commit/rollback ledger，只删除本次确定创建的对象；最终发布使用真正的 exclusive/no-follow primitive，不能通过 check-then-copy 模拟 exclusive create。

### 7. setup/start 没有做多实例资源冲突预检，交互式 S3 也仍是占位

位置：`crates/service/src/setup.rs:226-243,278-374`、`crates/service/src/instance_ops.rs:83-137`

实现只检查目标文件是否存在，没有加载已注册实例并比较 data-dir、Local object root、public/admin listener。两个配置可以在 spawn 前就共享同一数据目录或端口，违反 P11 §11 的确定性冲突检查。

交互流程虽然询问 `local/s3`，选择 `s3` 后只返回 “currently supports only local”。P11 §8 明确要求收集 endpoint/region/bucket/prefix 和 credential reference；当前命令面不是完成实现。

应在写文件前构建完整 plan、加载所有相关配置并做冲突检查；要么完成 S3 路径，要么把 P11 状态和 CLI 提示明确降级为未实现，不能把提示项当成已支持能力。

### 8. service manager 的安装、停止、重启和移除语义会留下错误或 orphan 状态

位置：`crates/service/src/service_manager.rs:159-251,282-372`

存在多处合同破坏：

- `install()` 总是通过 `atomic_write` 替换现有 unit/plist，而 P11 要求只在缺失时创建并拒绝覆盖非本次所有文件；
- launchd `stop()` 使用 `bootout`，这会卸载 job，不是“停止但保持 enabled”；随后的 `restart()` 忽略 stop 错误并对已卸载 job `kickstart`；
- systemd/launchd `uninstall()` 都忽略 disable/bootout 失败，然后删除 definition 并返回成功；上层随后删 registry，可能留下仍加载的 orphan service；
- `systemctl_output()` 和 `journalctl` 不检查退出状态，失败可被解释为空输出/未运行。

应分别实现 manager 的正确 state transition，所有破坏 ownership 的失败都必须向上返回；安装前验证现有定义是否完全属于同一 record，而不是盲目覆盖。

### 9. registry 的“exclusive”写入存在 TOCTOU，可覆盖另一个并发注册

位置：`crates/service/src/instance_registry.rs:146-203,234-255`、`crates/storage/src/fs.rs:311-369`

registry 先 `list()` 分配短 ID，再用 `path.exists()` 检查；最终调用的 `atomic_write()` 使用 rename，而 rename 会替换已存在目标。两个并发 `ocd start/setup` 可以为不同配置选中同一候选，其中后写者覆盖先写者。

此外，所谓 `ensure_dir_secure` 只检查类型和 group/world write bits，没有验证 owner，达不到 P11 §6 的 owner contract。

应使用不会替换现有目标的原子提交（例如同目录 temp + no-replace rename/link），冲突后重新读取 registry 并扩展 ID；目录/entry 校验需要包含期望 UID/GID。

### 10. Dashboard 发起的 upgrade 在重启当前 daemon 时必然丢失 job authority

位置：`crates/service/src/release_upgrade.rs:438-471`、`crates/service/src/cloudflare_v4/vendor.rs:403-465`、`crates/service/src/upgrade_api.rs:70-188`

upgrade job 是当前 daemon 内的 `tokio::spawn` + `Mutex`。它替换 binary 后遍历 registry，并会通过 service manager 重启发起请求的当前实例；进程终止时任务和 job store 一起消失。新 generation 的 store 从 `idle` 开始，页面无法观察到旧 job 的 succeeded/failed，也不可能按合同同时确认新 release identity 和 readiness。

文档把“job store 不持久化”列成 accepted limitation，但这不是边缘恢复能力，而是 Dashboard 默认升级路径的必经步骤，因此不能作为完成状态接受。

应把升级协调放在不会被目标重启杀死的明确一次性 helper/OS job 中，并用 bounded、secret-free、持久化 receipt 表达进度；新 generation 从同一 authority 恢复和确认状态。

### 11. upgrade 会重启所有注册实例，不校验配置，也不等待 readiness

位置：`crates/service/src/release_upgrade.rs:266-376`

代码直接 `registry.list()`，binary 替换后对每条 record 调 `manager.restart()`：

- 原本 stopped 的实例也被启动；
- 替换前没有加载/验证所有已注册配置；
- restart 后没有调用 readiness wait，更没有验证新 generation 的 release identity；
- `mark_succeeded()` 在 manager 命令返回后立即执行。

这与 P11 §10.2 的 “先验证所有配置，只重启 managed/running 实例并等待新 generation ready” 不符，也会改变 operator 明确保持 stopped 的状态。

应在任何 binary mutation 前冻结 active/enabled 集合并校验全部受影响配置，替换后只恢复原状态，并逐个验证 socket + HTTP readiness + target release identity。

### 12. receipt 没有证明当前 binary 的字节所有权，upgrade/uninstall 可替换或删除来源不明文件

位置：`crates/service/src/install_receipt.rs:210-240`、`crates/service/src/release_upgrade.rs:227-235,379-427`

`require_upgradeable_receipt()` 只比较 method 和 path，从不计算当前 binary SHA-256 并与 receipt.sha256 比较。只要路径和一个旧/伪造 receipt 匹配，`upgrade` 会覆盖、`uninstall` 会删除该路径当前的任意普通文件；receipt 中最关键的 ownership digest 实际没有参与授权。

应 no-follow 打开当前 executable，验证 regular file、owner/mode、device/inode 语义和完整 digest，再允许 destructive operation；校验后到 rename/unlink 之间还要防止路径替换。

### 13. binary 与 receipt 更新不是一个可恢复事务，失败会留下不可升级状态

位置：`crates/service/src/release_upgrade.rs:306-329,619-704,379-427`

新 binary 先 rename 覆盖旧 binary，之后才写 receipt。receipt 写失败时旧 binary 已丢失，新 binary 对应旧 receipt；`uninstall` 也是先删 binary、后删 receipt。验证 staged binary 失败或 rename 前出错时，`.ocd-upgrade-*` 也没有 cleanup guard。directory fsync 失败被忽略。

这违反 P11 对可恢复发布、fsync 和 receipt ownership 的要求。应使用显式事务状态/恢复 receipt，保留可回滚的旧 inode 直到新 receipt 持久化，所有临时文件由 guard 精确清理，并传播 fsync 失败。

### 14. release HTTP 客户端不能完成真实 GitHub asset 下载，且 timeout 不是总时长上限

位置：`crates/service/src/release_http.rs:71-108,154-175`

Hyper client 没有跟随 3xx redirect；GitHub release assets 通常重定向到对象存储，因此正式下载会把 302 当成失败。30 秒 timeout 只包住 `client.request()` 到响应头，body collection 没有 deadline，慢速响应可以无限占用 helper/upgrade。

应实现受限 redirect（限制次数、scheme、host policy）并把 connect、headers 和完整 body 都包在总 deadline 内，同时保留 size bound。

### 15. release/install identity 校验只实现了一部分，版本检查还是 substring

位置：`crates/service/src/release_upgrade.rs:153-223,539-681`、`scripts/install.sh:70-87,154-220`

Rust 解析了 `gitRevision`、`workerdRelease`、`workerdLockSha256`、artifact `os/arch/filename/bytes/sha256`，但只验证 schema/version/tag、目标是否存在和两个 checksum；没有验证非空/格式、target 与 os/arch/filename 的确定映射、重复 target/filename、正式 workerd identity。installer 更弱：用 `sed/grep` 搜 JSON，只确认某处出现 target，既不核对 manifest 中该 target 的 filename/bytes/sha256，也不核对 Git revision/workerd pin。

Rust 和 shell 都用“`--version` 输出包含 version 子串”判断身份，`0.1.10` 可以通过 `0.1.1` 检查。installer 写 receipt 还直接插值可由环境控制的 path/source，包含引号或换行时会生成损坏 JSON。

应定义一个权威 release manifest validator，让 installer 调用 staged `ocd` 的只读结构化 release-identity 命令完成同一校验；版本必须解析结构化输出或精确匹配完整 token，不能 substring/grep。

### 16. `dashboard.enabled=false` 仍启用 browser session 和 control login-code 能力

位置：`crates/service/src/run.rs:650-665`、`crates/service/src/http.rs:437-515`、`crates/service/src/instance_control.rs:240-260`

`DashboardAuth` 无条件创建并挂到 `HttpState`，两个 session endpoint 也无条件注册，control socket 无条件签发 login code。即使配置禁用 Dashboard，短期 session 仍可作为 V4 Admin token 使用；`ocd dashboard` 还能拿到一个最终打不开 UI 的 URL。

应让 dashboard enablement 同时拥有静态 UI、session endpoint、control login-code 和 session-as-admin 这四个能力；禁用时全部明确不可用。

### 17. upgrade mutation 对 malformed body fail open，并缺少 P11 要求的 browser same-origin/CSRF 判定

位置：`crates/service/src/cloudflare_v4/vendor.rs:403-428`

非空 body 先解析为任意 `serde_json::Value`，任何 JSON 解析失败、非 object、缺失 `version`、错误类型或空字符串都会变成 `None`，随后按“升级到 latest”启动 destructive job。未知字段也被静默接受。

该 POST 只经过 V4 role authorization，没有在 browser-session 场景执行 `same_origin_csrf()`。P11 明确要求 Dashboard upgrade mutation 同时校验短期 session/admin 能力和 CSRF/同源。

应使用 `#[serde(deny_unknown_fields)]` 的明确 request type；空 body可以有定义好的 latest 语义，但 malformed/nonconforming body 必须 4xx。鉴权上下文应保留 credential kind，使 browser session mutation 强制同源，而普通非浏览器 admin API 有清晰合同。

### 18. Gate cleanup 会递归删除真实用户的所有 instance control runtime

位置：`crates/service/tests/p0_1_gate.rs:69-80`

新增 cleanup 无条件 `remove_dir_all($XDG_RUNTIME_DIR/open-compute)` 和 `remove_dir_all($TMPDIR/open-compute-$uid)`。若开发机同时运行任何真实 foreground/user instance，执行 P0.1 Gate 会删除它们全部 control socket/descriptor，破坏正在运行的运维面。

测试必须设置并持有自己的唯一 runtime root，只删除本 round 明确创建的路径；不能清理共享用户目录。

## 详细问题 19～30

### 19. `instances` 和 `status` 没有实现 P11 状态模型，并会把 manager 错误伪装成 stopped

位置：`crates/service/src/cli.rs:849-885`、`crates/service/src/instance_ops.rs:231-282`

`ocd instances` 对每条记录硬编码 `state=stopped`、`version=null`、`listener=null`，没有组合 service manager、control socket 和 data-dir lock。`status` 只可能输出 `ready/starting/stopped`，缺少 `degraded/failed/stale`，且 `manager.is_active(...).unwrap_or(false)` 会把 systemctl/launchctl 故障当作 inactive。

这正是 P11 §6 的核心目标，不应以文档中“control socket 落地前”的旧说明继续保留。应建立一个权威 instance inspection result，由 `instances/status/selector` 共用。

### 20. `logs` 命令只有部分命令面，没有完成平台支持

位置：`crates/service/src/service_manager.rs:233-251,366-371`

Linux `--follow` 明确返回 unsupported；macOS 所有 logs 都返回 “not wired yet”。同时生成的 launchd plist没有 `StandardOutPath/StandardErrorPath`，错误提示却让用户去看这些键。P11 已把 logs 和 `--follow` 放进目标命令面，并在完成结论中声称 lifecycle/logs adapter 已覆盖。

要么完成 systemd follow 和 launchd log ownership，要么从完成声明/命令合同中撤回；当前状态是半截实现。

### 21. 交互 setup 选择“不立即注册/启动”时仍然写 registry，并输出不可用的 Dashboard 提示

位置：`crates/service/src/setup.rs:244-260,352-389`

prompt 把选择描述为“Register, enable, and start ... now?”，但 `start_service=false` 只跳过 service 操作，`registry.register()` 仍无条件执行，最后还提示运行 `ocd dashboard`。这与用户选择及 P11 §8.1 不符。

应把 register/install/enable/start/wait 作为同一个可选 phase，未启动时输出与实际状态一致的 next step。

### 22. Dashboard logout 只清本地状态，没有撤销服务端 session

位置：`packages/dashboard/src/features/auth/AuthProvider.tsx:87-90`、`packages/dashboard/src/main.tsx:23-61`、`crates/service/src/operator_session.rs:20-103`

服务端已有 `revoke_session()`，但没有 logout endpoint，客户端 `clearAuth()` 只删 `sessionStorage`。被复制/泄露的 token 在 TTL 到期前仍有效，违反 P11 “显式退出后失效”。

应增加同源/CSRF 保护的 revoke endpoint，logout 先 best-effort revoke 当前 token，再清本地状态；服务端 mutation 必须真正删除 session。

### 23. Dashboard upgrade polling effect 会因不稳定依赖反复重建并立即 tick

位置：`packages/dashboard/src/routes/_authenticated/platform/index.tsx:21,42-88`、`packages/dashboard/src/features/toast/useMutationFeedback.ts:4-21`

`useMutationFeedback()` 每次 render 返回新对象，effect 还依赖整个 `status` query result。每次 `setUpgradeJob` 都会 render、清 interval、重建 effect，并立即 `tick()`；轮询可能退化为紧循环并产生重叠请求，而不是每 2 秒一次。

应依赖稳定 callback/refetch function，或让 feedback hook memoize；单一 polling owner 用 timeout 链保证上一次请求完成后再安排下一次。

### 24. update-check helper 既未 detached/降优先级，也继承完整环境

位置：`crates/service/src/update_check.rs:188-202`

这里只是普通 `.spawn()`：child 与父进程同 session/process group，没有 `setsid`/等价平台 detach，也没有 nice/低优先级；父终端或 service manager 的信号仍可能影响它。child 还继承所有环境变量，包括可能存在的 config/secret refs，与 P11 的最小信息 helper 不符。

应显式构造最小环境、建立平台正确的 detached lifecycle、设置低优先级，并保持绝对 executable 与 null stdio。

### 25. update cache 没有校验 owner/mode/未来时间，未来时间可长期抑制刷新

位置：`crates/service/src/update_check.rs:89-163`

`read_cache()` 只拒绝 symlink/非文件/超长/坏 JSON，不检查 owner 和 group/world permissions。`should_refresh()` 对 future `checked_at_ms/failed_at_ms` 使用 `saturating_sub`，差值为 0，于是把恶意或损坏的未来时间当作刚检查过，可能多年不刷新。

P11 §10.4 明确要求未来时间戳和宽松权限 cache 一律忽略。读取时应验证期望 owner、mode 和时间合理性；不合法 cache 直接视为 absent。

### 26. Dashboard update check 建了第二套 authority，fallback 还会把旧版本当成更新

位置：`crates/service/src/cloudflare_v4/vendor.rs:334-389`、`crates/service/src/upgrade_api.rs:190-207`

Dashboard handler 每次单独做 live network check，成功后不更新 CLI cache；失败时再走 cache/local allowance fallback。这不是 P11 所说“读取或触发同一个 cache”，而是两套语义。fallback 抹掉真实网络/metadata 错误，始终返回 200。

`check_result()` 用 `available != current` 判断更新，已有 `cmp_stable_semver()` 却未复用。升级后旧 cache 的版本低于当前版本时，UI 会错误显示 update available/upgrade allowed，直到 backend 在执行阶段拒绝 downgrade。

应由一个 update-check service 统一 live refresh、cache commit、错误和 SemVer 比较；Dashboard 与 CLI 只消费同一结果，不保留 alternate fallback。

### 27. 新增 upgrade API 没进入 OpenAPI / capability 的权威合同

位置：`crates/service/src/cloudflare_v4/vendor.rs:38-40`、`packages/cloudflare-extension/src/index.ts:28-109`、`openapi/open-compute-extension.json`、`test/conformance/p6-contract.mjs`

Rust 新增三个 `/client/v4/open-compute/upgrade*` route，TypeScript extension 手写了 payload 类型，但 OpenAPI extension 和 capability/conformance inventory 没有相应 operation/schema。生产 handler、SDK 和正式 contract 形成平行定义，类型漂移无法被现有生成/校验链发现。

Day1 应直接更新唯一 API 模型及其所有 producer/consumer；如果这些 endpoint 只属于 operator surface，就不要塞进 Cloudflare-compatible v4 extension，改为一个明确的 operator API authority。

### 28. install receipt 的可配置位置无法被安装后的 binary 重新发现

位置：`scripts/install.sh:16-34`、`crates/service/src/install_receipt.rs:87-105`、`crates/service/src/release_upgrade.rs:100-130`

installer 公开支持 `OPEN_COMPUTE_INSTALL_DEST` 和 `OPEN_COMPUTE_RECEIPT_PATH`，但 `ocd upgrade/uninstall` 只按 executable 的祖父目录推导 receipt。若安装时使用任意自定义 receipt path，运行中的 binary 没有任何持久信息能找到它，后续自升级/卸载必然报告 receipt missing。

要么把受支持布局收窄为可确定推导的 `<prefix>/bin/ocd` + `<prefix>/share/...`，要么设计单一可验证的 receipt discovery；不要公开无法闭环的 override。

### 29. P11 文档和用户文档包含互相矛盾的完成声明及历史兼容说明

位置：`docs/implemented/p11-ocd-operator-experience.md:3-42,406,515-528`、`packages/docs/ocd/cli.md:23-25`、`packages/docs/zh/ocd/cli.md:23-25`、`docs/references/runbooks/*.md`、`packages/docs/ocd/{cli,get-started,configuration,deploy}.md`

具体问题：

- 文档一面称 Implementation GO、核心路径已落地，一面明确写 final coverage/Gate 待执行，并把真实 service/setup 资格全部留待以后；
- §10.4 称 `--help/--version` 都经过 hook，但 §0 又把它们跳过列为 accepted limitation，实际 `bin/ocd.rs` 在 Clap DisplayHelp/DisplayVersion 时直接返回；
- CLI 文档仍写“control socket 落地前，instances 全部 stopped”，尽管同一分支声称 control socket 已完成；
- 多个 embedded runbook 仍用 `/etc/open-compute/platform.toml`，新文档没有直接更新它们，而是增加“旧名字也只是显式路径”的解释。这是文档层的兼容包袱，不是 Day1 收敛；
- §0 把 examples 和 generated units 称为“同一模型”，实际没有共享生成/验证关系且安全内容显著不同。

应把文档状态降为 in progress，逐项以真实实现为准；直接更新当前 runbook/example，删除历史文件名解释和不成立的验收声明。

### 30. 当前 diff 混入多组与 P11 无关的 coverage/测试维护

位置：`crates/artifacts/src/local_tests.rs`、`crates/runtime/src/process_tests.rs`、`crates/service/src/{ai_provider_tests,doctor_tests,d1_backup_tests,kv_backup_tests}.rs`、`test/coverage.sh`

这些变更覆盖 Local object authority、macOS runtime staging、AI provider、doctor、backup 以及 coverage wrapper/maxproc 策略，和 P11 operator experience 没有直接生产依赖。它们扩大 review/Gate 影响面，也让 P11 的真实回归与为恢复 coverage 数字增加的测试难以区分。

应拆到有独立根因和证据的变更中；P11 分支只保留为本功能新增/变更行为提供的测试。尤其不要把 coverage tooling 改动和功能验收混在同一冻结输入里。

## 详细问题 31～39（simplify）

### 31. 同一个 registry identity 被重复解码，绕过已有权威校验

位置：`crates/service/src/instance_ops.rs:46-58,157-166,196-205,233-249,302-350,420-433`、`crates/service/src/instance_registry.rs:59-73,421-434`

`InstanceRecord::instance_id()` 已经负责 digest hex 解码、短 ID 校验以及 canonical path digest 校验。`instance_ops` 又实现一份 `decode_digest_hex()`，并在多个命令中手工 `from_short_and_digest()`；这份副本没有校验 record path 与 digest 一致。

应直接调用 `record.instance_id()`，删除第二份 decoder 和对应重复测试，让 registry record validation 保持一个 owner。

### 32. fake/test-support 代码被编进普通生产库，并存在两份 ready-stub 实现

位置：`crates/service/src/release_http.rs:112-152`、`crates/service/src/service_manager.rs:398-590`、`crates/service/src/instance_control.rs:540-566`

`FixtureReleaseHttp`、`FakeServiceManager` 及其状态/清理逻辑没有 `#[cfg(any(test, feature = "test-support"))]`，普通生产 build 也包含。`publish_ready_stub()` 同样无 cfg，且 service_manager 又手写一份 `publish_fake_ready_stub()`，两份 descriptor 构造已经漂移。

仓库规则要求测试 hook 只在 test/test-support 可达。应 cfg 掉所有 fake，实现测试时也复用真实 control protocol 或唯一的 cfg-gated helper；生产 readiness 不能知道 fake runtime root。

### 33. `ServiceManager` trait 暴露了纯测试方法，测试需求反向污染生产流程

位置：`crates/service/src/service_manager.rs:15-39`、`crates/service/src/instance_ops.rs:119-136,181-187`、`crates/service/src/setup.rs:369-373`

`readiness_runtime_root()` 的注释明确说明生产实现总返回 `None`，只有 fake 返回 scratch root。它不是 OS service-manager 能力，却迫使 setup/start/restart 依赖一个测试专用虚方法，并进一步促成 stale descriptor fallback。

应把 runtime-root 作为测试 harness 外层输入或用 cfg-gated operation context；保持 `ServiceManager` 只表达 systemd/launchd 真正变化的边界。

### 34. config discovery 为未使用的 source metadata 建了额外数据模型

位置：`crates/service/src/config_discover.rs:13-79`

`ConfigDiscoverySource` 和 `DiscoveredConfigPath` 的 `source` 只在本模块测试中读取；生产调用马上只取 `.path`。这是一层没有消费者的未来扩展点。

应让内部 discovery 直接返回 `PathBuf`，或仅在确有用户可见诊断需要 source 时保留并消费它；当前无需把两种公共类型加入 crate API。

### 35. Dashboard auth 有死状态，鉴权判断也有两份实现

位置：`crates/service/src/dashboard_auth.rs:17-32,57-71,73-115,168-174`、`crates/service/src/http.rs:686-697`、`crates/service/src/cloudflare_v4/wire.rs:480-504`

`LoginCodeRecord.consumed` 永远是 false：成功兑换直接 remove，失败从不设 true，purge 对 consumed 的判断无效。`DashboardAuth.startup_id` 只由测试 accessor 读取，session validation 不使用它；generation isolation实际由进程内 store 重建保证。

另外，普通 admin authorize 和 V4 authentication boundary 分别重复 “admin bearer 或 dashboard session” 判断，未来很容易只更新一处。

应删除死字段/accessor，保留最小的 expiry map；把 credential 分类和校验集中到一个 auth authority，handler 只消费结构化 role/credential-kind。

### 36. 新增模块大多被不必要地公开，扩大了稳定 API 和 rustdoc 负担

位置：`crates/service/src/lib.rs:21,30,41-43,52,62-63,72-78`

对仓库搜索未发现 service crate 外部使用这些新模块；它们只在 crate 内部和 unit tests 使用，却全部声明为 `pub mod`，许多 helper/fixture 也被 `pub`/re-export。对 composition-root 私有实现没有收益。

应默认 `mod`/`pub(crate)`，只有 binary 或确实的跨 crate contract 才公开。`update_check` 也应直接从拥有者 `release_http` 导入 trait/constants，不要经 `release_upgrade` 间接 re-export。

### 37. service definition 存在两套手工维护的真实模型

位置：`crates/service/src/service_manager.rs:104-149`、`examples/systemd/open-compute.service`、`examples/launchd/dev.open-compute.ocd.plist`

generated unit/plist 与 examples 不是同一模型：examples 有非 root 用户、working directory、systemd hardening、读写路径和 launchd log path；generated 版本全部缺失。没有测试或生成器把二者关联起来。P11 文档把它们描述为“同一模型的最小子集”只是为分叉增加解释，不是 ownership 收敛。

应定义一个按 scope 参数化的 service definition model，由它同时渲染安装内容和 examples，或让 examples 由测试验证为同一 renderer 的规范输出；删除第二套手写行为。

### 38. 三个刚修改/新增的文件已经超过仓库 800 行预算

位置：`crates/service/src/cli.rs`（949 行）、`crates/service/src/instance_control.rs`（959 行）、`crates/service/src/service_manager.rs`（971 行）

`cli.rs` 本次从约 500 行增长到 949 行，command classification、pre-hook、operator dispatch、config dispatch 全挤在一个函数族。后两个文件把 300+ 行 unit tests 内嵌在生产模块末尾，尽管仓库已经普遍使用独立 `*_tests.rs`。

应按 ownership 拆分 CLI operator/release dispatch；把 instance_control/service_manager tests 移到独立测试文件。拆分应顺带删除上述 test seam 和重复 helper，而不是再加 facade/pass-through wrapper。

### 39. 若干无生产调用的 helper/接口应删除，而不是作为“以后可能用”保留

位置：`crates/core/src/instance_id.rs:78-88`、`crates/service/src/install_receipt.rs:96-105`、`crates/service/src/release_upgrade.rs:724-735`、`crates/service/src/instance_control.rs:143-157`

当前搜索显示：

- `InstanceId::is_empty()` 只在测试中断言一个构造器已经保证的不变量；
- `production_receipt_path()` 只在 receipt 测试中调用；
- `load_receipt_for_exe()` 只被一个“调用但不检查结果”的测试触达；
- `InstanceControl::update_descriptor()` 只在 unit test 中调用，生产 readiness transition 没有接线。

前三者应删除或收为真实调用方的局部逻辑。最后一个不能仅删除：它暴露的是 finding 4 的半截状态机，应该先把真实 health transition 接入，然后保留为实际需要的窄内部方法。

## 建议修复顺序

1. 先确定唯一的 instance inspection/control/readiness 模型，删除 stale-file/fake production 路径，并让 daemon 使用 registry identity。
2. 完成非 root、按 scope 的唯一 service definition 及正确 lifecycle；随后重做 setup 的 plan/commit/rollback 和冲突预检。
3. 把 upgrade 移出会被自身重启终止的 daemon job，建立 receipt/binary 可恢复事务，并统一 release manifest validator。
4. 收敛 Dashboard session/update API，补 revoke、strict request、credential-kind CSRF 和同一 update cache authority。
5. 最后做 simplify 清理：cfg fake、删重复 decoder/死字段/未用 public API、拆超长文件，并把文档状态改回与实现一致。
