# R4：架构简化

状态：**已完成**（2026-11-02）。每项已实施，或按用户 2026-11-02 的决定处理：R4.8 的保留项由用户决定；R4.9/R4.10 的大重构按“净删行且不慢于 15%”实测，未达标者已还原并记录数据。`containers` 与 90% coverage 需要 Linux 输入，留在发布前的完整测试中验证。

目标：Cloudflare 兼容面和产品功能矩阵不变，按“单机、小团队、单 daemon 多实例”的定位删掉多余的状态机、防御代码和自写实现。
先在本地改架构，再改测试和 CI。

## 已定的约束

- **兼容面只对齐 Cloudflare 当前行为。** 以当前文档、cf 和官方 SDK 实际发出的请求为准。已废弃的 API、只为旧 compatibility date 或 flag
  保留的行为、旧客户端（如 wrangler）专用路径，增加复杂度时不做。
- **保留单 daemon 多实例。** 一个实例对应一个 account；单机多 account 共用一个 daemon、一套对外监听和 Gateway。只简化 registry 内部实现。
- **数据库平滑升级。** 从上一个正式版本升级必须成功，不要求用户重建数据。已发布的 SQL 迁移不改，只追加。
  现有升级阻塞见 [#159](https://github.com/elliothux/open-compute/issues/159)。
- **行数上限**：Rust 生产文件 1500 行、函数 600 行；测试 2000/800。
- **覆盖率**：Rust 行覆盖率 90% 继续作为硬门槛。
- **资源限制**：Standard limits 的能力和 API 保留，原生执行器保留在 fork 中；只简化平台侧实现（R4.5）。
- **原生扩展保留 Cap'n Proto 通道。** HostExtension 补丁、fd 4 session broker 和 session Cap'n Proto client 作为长期维护的 fork 补丁。
- **验证方式**：每一步在本地跑受影响的 Gate 目标、单元测试和 clippy。完整测试只在发版前跑一次，不做每日 CI。
- **先核实再改。** 评审依据的部分源码来自旧提交 `8694cba`，每一步动手前先对照当前代码确认问题仍然存在。
  例如 #1、#45、#59、#61、#62、#110 已关闭。

## 步骤

每一步单独提交，提交后仓库净行数下降。

### R4.0 规则与配置收尾

状态：**已完成**（2026-10-31）。删除 `missing_docs`、`missing_debug_implementations` 和只为安抚 lint 的 121 个属性；`too_many_arguments`、`type_complexity` 改为 `allow`。

### R4.1 机械清理

状态：**已完成**（2026-10-31）：typed ID 的 `ToSql`/`FromSql` 与调用点、`p3_3_test_support.rs` 改名。调度器相关的字符串状态、代数和 busy 判断随 R4.3 处理。

- ID newtype 实现一次 `ToSql`/`FromSql`；代数计数改用 `i64`；字符串状态改为 enum。删掉逐行的 `to_string()`、`parse()` 和
  `map_err(|_| rusqlite::Error::InvalidQuery)`。
- SQLite busy 按 `SqliteFailure` 错误码判断，不再匹配错误信息字符串。内部日志保留原始错误，只在对外 HTTP 边界脱敏。
- 删除不可能发生的分支，例如 `checked_add` 溢出返回业务错误。
- 文件名和类型名里的阶段号换成领域名，例如 `p2_3_promotion.rs` → `promotion.rs`。Gate 目标名在 R4.10 一起改。

### R4.2 修 bug 时删除产生它的设计

状态：**已完成**（2026-10-31）：Dashboard 由 axum 直接提供（V19 清理旧 system Worker）；失败的幂等操作释放预留（V20）；测试 fake 只在测试构建中编译；#1、#110 已核实为单一来源。

- Dashboard 由 axum 直接返回嵌入的静态资源，删除 system-owned Worker 部署和对应的幂等记录。修复 #168。
- 幂等记录不再缓存失败结果。
- 测试 fake（`FakeServiceManager`、`FixtureReleaseHttp` 等）全部放进 `cfg(test)` 或 `test-support`；删除只为测试存在的 trait 方法，
  以及 readiness 读取旧 descriptor 的回退路径。
- 核实已关闭的 #1（请求体上限两处定义）和 #110（AI Search 实例名两处定义）是否已收敛到一个来源；没有的话收敛。

### R4.3 调度库并入 `control.sqlite`

状态：R4.3a **已完成**（2026-10-31）：调度表随 control 迁移 V21 并入 `control.sqlite`，启动时一次性导入旧 `scheduler.sqlite` 并改名保留。R4.3b-1 **已完成**（2026-10-31）：promotion、rollback 和 quarantine 在同一个 control 事务里替换 Queue consumer 和 Cron 调度（迁移 V22），删除 `ProductPromotionCoordinator`、projection 状态、drain 等待和 Cron activation generation。R4.3b-2 **已完成**（2026-11-01）：Alarm、Queue 和 Cron 的 claim 只按行 ID 和 `claimed` 状态识别（迁移 V23），删除 `claim_token`、持久化 lease 和过期扫描；启动时把 `claimed` 重置为 `ready`，运行超时用 `tokio::time::timeout`。Workflow 的 `run_token` 保留：它拦截 workerd 子进程重启后旧 generation 的回调。

`scheduler.sqlite`（Cron、Queue consumer、Workflow 调度，约 1.4 万行 Rust）并入 `control.sqlite`：

- promotion 改为一个事务：切换 active version，替换 consumer 和 cron 行。正在运行的 run 带着自己的 `version_id` 跑完，promotion 不等 drain。
- 删除 projection、drain 等待、多余的 generation、`claim_token` 和持久化 lease。启动时把 `claimed` 重置为 `ready`；运行超时用
  `tokio::time::timeout`。

平滑迁移：

1. control 追加下一版迁移，建好调度表。
2. 迁移后由 Rust 执行一次性导入：发现旧 `scheduler.sqlite` 时，在事务外 `ATTACH`，在一个事务里复制行（`claimed` 改为 `ready`），
   在 `platform_meta` 写完成标记，然后把旧文件改名保留。崩溃时事务回滚，重启后重新导入。
3. 备份、快照和恢复改为只处理 `control.sqlite`；恢复旧快照时走同一导入路径。
4. 用 0.3.0 生成的真实数据目录测试升级，覆盖导入中途崩溃。

验证目标：`p2-1`、`p2-2`、`p2-3`、`workflow`、`p1-snapshot`、`p1-crash`。

### R4.4 按威胁模型删除防御

状态：R4.4a **已完成**（2026-11-01）：删除启动重算摘要、发信号前的进程身份和摘要校验、registry 本地文件复检、receipt 摘要、update cache 的 owner、mode 和未来时间检查，以及数据目录的 symlink 和 owner 检查。保留构建期 pin 校验、目录锁和租户输入的路径校验。孤儿恢复按记录的 PGID 停止进程组；lease 记录开机标识，开机标识不同时丢弃 lease，不发信号。R4.4b **已完成**（2026-11-01）：ocd 与 workerd 之间的内部通道（私有 ingress、绑定后端、Workers Logs、探活、源码服务、Python 预备）改走 `<OCD_DIR>/run/<InstanceId>/` 下 0700 目录中的 Unix socket，用 stock workerd 的 `unix:` 地址，删除 generation credential 和 loopback 鉴权；租户仍不能连 Unix socket，fd 4 HostExtension broker 保留。macOS 上对端在 accept 前关闭会让 workerd 的 `peer_cred` 返回 ENOTCONN，fork R25 修补后报告未知凭据。HTTP、WebSocket 和 Workflow 分发结束时释放 Version pin，删除 Worker 不再被遗留 pin 阻塞。SQLite 关闭 memstatus，避免并行连接争用全局分配锁。真实 Cloudflare 对比（p3-cf-diff）9 个 fixture 全部一致，修正了 D1 错误信息、R2 空 range、multipart 元数据、list 上限和 Workflow 保留期下限（V24）。

依据 [威胁模型](security.md)，删除对付同机进程和本地篡改的代码：

- 每次启动重算内嵌 workerd 摘要，改为打包时校验一次。
- 向 PID 发信号前校验进程身份和二进制摘要，改为按 process group 管理子进程。
- registry 的 TOCTOU 处理、短 ID 碰撞扩展、receipt 摘要校验、update cache 的 owner、mode 和未来时间检查。
- 数据目录的 symlink 和 owner 检查。保留目录锁。
- 每个重试和等待都要有终态，缓存的失败要过期，重启后无需手工改库即可恢复。

- ocd 与 workerd 之间的 loopback 内部通道（私有 ingress、binding backend、Workers Logs 写入）改为 0700 目录下的 Unix socket，
  删除 generation credential 及其注册、指纹和轮换代码。workerd 的 external service 支持 Unix socket 地址，先核实不需要 fork 补丁。
- 原生扩展的 fd 4 session broker 保留。

### R4.5 资源限制的平台侧简化

状态：**已完成**（2026-11-01）。上传转换和 Settings PATCH 一次性校验并物化 limits；持久化读取、Loader 和 Version 克隆直接使用类型值，PATCH 仍按维度继承 active Version。snapshot 签名、探活凭证和平台侧 ceiling 计算在动手前已不存在。W2 补丁账本写明了删除条件。

能力、v4 API、Settings PATCH 克隆 Version、错误码 1101、1102、10021 和 Version response 字段形状都保留，这是 Cloudflare 的行为。
原生执行器（CPU 计时、isolate 内存、startup、subrequest、连接数）留在 fork 中。平台侧改动：

- **只在入口校验一次。** v4 decoder 和 Settings PATCH 校验取值、物化 Standard 默认值。之后持久化、runtime-source snapshot、Loader
  和 workerd 配置渲染直接使用类型，读取持久化值时不再重新校验 canonical 形式和上下限。
- **删除 snapshot 签名。** runtime-source snapshot 的 HMAC 签名防的是本地篡改，不在威胁模型内。snapshot 只保留内容摘要。
- **收窄类型传递。** `EffectiveResourceLimits` 现在出现在约 50 个文件中。只在 Version 模型、workerd 配置渲染和 Settings API 三处
  使用；其他代码随 Version 读取，不再逐层透传。
- **ceiling 只算一次。** Dynamic Worker 的 Version、WorkerCode、entrypoint 和委托 Loader 取最小值只在原生 Loader 中计算；
  平台侧如有重复计算则删除（先核实）。
- **探活去掉凭证。** supervisor 的 `/internal/live` 随 R4.4 改走 Unix socket，删除 generation credential。普通 tenant 超限不消耗
  restart budget 的行为保留。
- **补丁账本写明删除条件。** W2 执行器在上游 standalone 提供等价执行时删除。

验证目标：覆盖 limits 的 Gate 目标（先用 `./test/gate.py --list` 核对），以及 Settings API 和 Dynamic Worker 用例。

### R4.6 用成熟 crate 替换自写代码

状态：**已完成**（2026-11-01）。semver 和 croner 已替换；croner 让 `a/b` 按 Cloudflare 实测语义展开为 `a-最大值/b`。reqwest 早已在用。其余各项不做：`service-manager` 和 `self-replace` 无法覆盖现有的 launchd bootout、指定目标路径和 fsync 语义，替换反而增加代码；`persist_noclobber` 可删的代码不足 50 行，现有实现已用 `create_new` 保证独占；std 的 `UnixStream::peer_cred()` 尚未稳定，现有调用已经是成熟库接口。

每行一次提交：

| 自写代码                                 | 替换为                                       |
| ---------------------------------------- | -------------------------------------------- |
| 发布下载的手写 hyper 客户端              | `reqwest`                                    |
| 自升级的二进制替换                       | `self-replace`                               |
| systemd unit 和 launchd plist 渲染与启停 | `service-manager`                            |
| control socket 对端 UID 检查             | `UnixStream::peer_cred()`                    |
| check-then-copy 模拟的独占创建           | `tempfile::NamedTempFile::persist_noclobber` |
| 子串匹配和自写版本比较                   | `semver`                                     |
| Cron 表达式解析（先核实是否自写）        | `croner`                                     |

### R4.7 删除兼容负担

状态：**已完成**（2026-11-01）。已删除未协商使用的逐对象 Assets 上传端点，以及 Service/DO binding 已拒绝的 `environment` 字段接线；废弃上传字段仍由固定 cf 发出，因此保留，`ocd` 本身没有旧 compatibility date 行为分支可删。兼容面注解维护在 [`openapi/p6-capability-source.json`](../openapi/p6-capability-source.json)，文档链接、cf 使用情况和由 SDK surface 派生的使用情况随现有生成器进入 [`openapi/p6-capability.json`](../openapi/p6-capability.json)。

- 按 cf 和官方 SDK 的实际请求盘点 `/client/v4`，删除只有旧客户端使用的端点、字段和上传格式。
- 删除 wrangler 专用路径，例如 `target_http.rs` 的 remote Wrangler target 探测。
- 删除 Cloudflare 已废弃的字段，例如 Service binding 的 `environment`（先对照 CF 文档确认）。
- 删除只为旧 compatibility date 或 flag 保留的行为分支。
- 兼容面整理成一份机器可读清单：每项写 CF 文档链接、cf 或 SDK 是否使用、支持状态。

### R4.8 fork 瘦身

状态：**已完成**（2026-11-02）。补丁账本已完成（2026-11-01）；其余三项由用户在 2026-11-02 决定：私有 dynamic binding forwarding 与 Caddy 保留，HostExtension 收窄随下次上游 rebase 进行，见 [workerd 路线](workerd/README.md#补丁账本)。HostExtension 接线收窄延期到下次上游 rebase，预计仅减少 10–40 行，且需要 fork 发行。私有 dynamic binding forwarding 保留，因为 stock Worker Loader 不能转发 KV/R2/Queue，删除会改变产品矩阵；Caddy 替换条件未满足，operator Caddyfile imports 与 `ocd caddy` 已超出 TLS/ACME/Host 路由，Container sidecar 仍需要 Go。

与 [W4](workerd/w4-thin-fork.md) 一起执行：

- 在 [workerd 路线](workerd/README.md) 补全补丁账本：每个补丁写用途、上游 issue 或 PR、删除条件。
- 原生扩展的 Cap'n Proto 通道保留。HostExtension 补丁随上游升级时只收窄接线代码，不改变 `HOST.call`、`HOST.stream` 和 Provider 协议。
- 删除私有 dynamic binding forwarding grants，只用上游 Worker Loader API。
- Gateway 只做 TLS 终止、ACME 和按 Host 路由时，用 `rustls` 加 ACME crate 替换 Caddy fork，删掉 Go 工具链。

### R4.9 结构简化

crate 合并实测（2026-11-02，第二次计时）：只减少 66 行清单胶水，计入导入改动后净增 142 行；改 storage 后重编译 7.3 s→8.8 s（+21%），改 artifacts 后 7.2 s→10.1 s（+39%），storage 测试编译 2.3 s→4.8 s（+105%）。超过 15% 的回退线，已还原。
KV 产品模块加 `kv` feature 试点：改 KV 后重编译 7.8 s→6.1 s，关闭默认 feature 后 8.3 s→7.5 s，但去掉纯移动后净增 173 行（59 行 `cfg`、105 行导入/模块/导出），未满足净删除，已还原；按产品布局和每产品 feature 不做。

状态：**三项小清理已完成，其余按调研结论收窄**（2026-11-01）：已移除 search→core 的未用依赖和 `check-boundaries.sh` 的未用 `members` 计算，并将 DO schema 最终定义移回原对象位置，删除后续覆盖赋值。crate 合并预计仅删除约 50–150 行胶水代码，却涉及 821 个导入文件、扩大重编译单元并牵涉 artifacts 的 `opt-level = 3` 覆盖，待构建计时证明收益后再做；按产品布局延期，纯移动没有净删除，`promotion::replace_products` 等共享事务必须保持原子性，`check-boundaries.sh` 保留，因为 Rust 模块不能表达其依赖与 Bun 策略。每产品 feature 暂不做，预计增加 400–1,200 行且组合未经测试；vendor `/open-compute/**` HTTP schema 已改由 Rust 模型经 `schemars` 生成，并继续使用现有 OpenAPI-to-TypeScript 生成器；Cloudflare 形状仍以上游为准，保留 capability/type/config 双射检查。

- `core`、`storage`、`artifacts` 合成一个底层库。
- 代码按产品组织：每个产品一个模块，自带表、逻辑、HTTP handler 和测试；`service` 只做组装。删除 `check-boundaries.sh`。
- 每个产品一个 cargo feature：发行版全量编译，开发只编译受影响的产品。
- 从 Rust 模型生成 OpenAPI 和 TS 类型（`schemars` 或 `ts-rs`），删除手写类型和 capability/type/config 双射检查。

### R4.10 测试与 CI

nextest 替换实测（2026-11-02，nextest 0.9.150）：`runtime daemon kv r2 d1 contract` 第二轮 247.0 s→264.3 s（+7.0%），保留输入校验、金丝雀、隔离、登记、互斥与 90% 覆盖率的前提下净增 140 行，未满足净删除，已还原。

共享 daemon 实测（每个测试一个实例）：隔离、清理与金丝雀检查均通过，但 `python-services` 59.5 s→81.9 s（+37.6%），三目标合计 +10.1%，净增 231 行，已还原。

状态：**已完成**（2026-11-02）。已删除 compile IPC/source-digest 握手和 final 冻结/重复运行策略；Python 矩阵从 2,465 s 降至 1,365 s；R2 上传缩为 3 个分片；摘要只覆盖 Git 跟踪的源码输入。PR 按受影响产品选择 Gate，发布只跑一轮完整测试并保留 90% coverage；Rust 工具直接用 cargo；新增录制合同 Gate，Gate 目标改为领域名。
完整 nextest 替换延期：仍需迁移 typed runner、用例登记和覆盖率证据，不能只替换进程调度。共享 daemon 延期：现有崩溃/重启用例依赖独立状态和进程生命周期，需先拆分隔离边界；跨 account 共用一个 workerd 跳过，因为这会改变一个实例对应一个 account 的生产架构。

原定范围：

- `cargo nextest` 替换 `gate.py` 和 `gate_cases.py`；删除 `--final` 冻结、重复 final 拒绝和源码摘要握手；去掉 `mbx` 硬依赖。
- 测试共享一个 daemon 和 workerd，每个测试用自己的 account；重启和崩溃场景收敛到一个小套件。
- 缩小测试数据（241 MB 上传用例只需 3 个分片）；Python 矩阵只测 Python 与 JS 绑定之间的一层。
- 契约测试改为对录制的 Cloudflare 响应比对，发版前用真实 Cloudflare 刷新。
- 把规则文件和参考文档移出 `sourceDigest` 输入。
- CI：PR 只跑静态检查和受影响产品的测试；完整测试、打包和安装升级测试只在发版前跑一次。阶段号 Gate 目标改为领域名。

## 不在 R4 内的遗留问题

CR.md（提交 `94e5ef7`）中 R4 未覆盖的延期项已转为 issue：
[#189](https://github.com/elliothux/open-compute/issues/189) logs follow 与 macOS 日志、
[#190](https://github.com/elliothux/open-compute/issues/190) Dashboard 退出不撤销 session、
[#191](https://github.com/elliothux/open-compute/issues/191) Dashboard 关闭时仍签发 login code、
[#192](https://github.com/elliothux/open-compute/issues/192) 交互式 setup 不支持 S3。
其余延期项由 R4.1、R4.2、R4.4、R4.6、R4.9 覆盖，Gate 清理共享目录的问题已不存在。
