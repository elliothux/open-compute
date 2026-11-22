# I200–I201　D1 应用迁移、workerd 内存、#148、#157 与 #199

I200 已在本地修复 D1 改表授权，并通过公开 API 和服务重启验证。I201 已确认短测中同一 Worker 实例得到复用，仍需在原生 Linux x86_64 上查明长期内存增长的原因。

## I200　让应用能改表，并在发布前验证数据仍然可读

在全新安装的 v0.4.0 上，通过 D1 API 给表改名会返回 HTTP 500 和错误码 `9100008`。原发布 CI 用 KV 和 Worker 检查安装状态。对于留有旧版 Worker 的实例，发布流程当前预期升级被拒绝，并检查旧服务和数据仍可用。此前的 D1 迁移用例只给已有表加了一列。这两项都可以通过，而应用仍然无法改表名、改列名或删列。[问题报告](https://github.com/elliothux/open-compute/issues/200) · [v0.4.0 的 D1 迁移用例](https://github.com/elliothux/open-compute/blob/v0.4.0/crates/storage/src/d1/tests/migration_ledger_is_atomic_idempotent_and_detects_drift.rs)

**修复。** SQLite 在编译这三种改表语句时会检查临时 schema 目录。D1 先确认改表目标是主数据库中的租户表，再允许 SQLite 在当前语句内读取临时目录、更新必要的目录字段。编译下一条语句时重置授权，平台元数据表和迁移历史表继续受保护。本地 Cargo 测试通过 40 项 D1 存储用例，包括外键延迟到提交时检查的成功和回滚路径。[D1 授权器](../crates/storage/src/d1/hardening.rs)

**补进现有 D1 产品 Gate。** 通过管理 API 创建数据库，对同一个 `POST /client/v4/accounts/{account}/d1/database/{database}/query` 入口执行以下操作：

1. 建表并写入两行不同的数据，依次执行 `RENAME COLUMN`、`DROP COLUMN` 和 `RENAME TO`。检查每次 API 返回成功，并查询实际列名和行值。
2. 在一个请求里执行 Drizzle 常用的重建流程：建新表、复制行、删除旧表、把新表改为原名。重启服务后，通过同一数据库 ID 核对主键、行值和新表结构。
3. 让同一批次的第二条语句失败，检查第一条改名也已回滚。随后在同一数据库执行有效 DDL 并读回原行；同时验证普通 SQL 仍无法修改平台元数据或临时目录。

上述场景已加入现有 D1 产品 Gate，本地两项均通过。安装升级脚本已增加成功路径：即使旧 Worker 会触发预期拒绝，也先用一个仅含 D1 和 KV 的旧实例验证成功升级。另一组实例继续检查旧 Worker 的拒绝原因和原服务的数据。Linux arm64 实测中，v0.4.0 正式包先写入两行并正常关停，候选包按原 UUID 接管同一份数据，改列名和表名，重启后仍能读回两行。本地验证覆盖两个真实版本的数据交接；包含系统服务管理器的完整安装升级流程留给发布 CI。[公开 API 回归](../crates/service/tests/p0_6_d1_gate/http_migrations.rs) · [安装与升级测试](../test/install-lifecycle/run.py)

存储用例也验证了报告附带的外键 PRAGMA：读出 `foreign_keys` 的状态，并覆盖 `defer_foreign_keys` 在事务提交时检查外键的行为。[Gate 与测试分层](references/testing.md)

## I201　找出间隔请求后仍留在 workerd 中的内存

报告者在 v0.4.0 上观察到，流量稀疏时一次请求后匿名内存常增加约 5–7 MiB，小内存实例在 8–16 小时内触及服务上限。本地 Linux arm64 对照中，正式包和 R26 候选包各自都连续复用了同一模块实例。连续 10 次请求后再发 4 次间隔一分钟的请求，workerd 子进程的私有脏页（`Private_Dirty`）分别净变动 −1.67 MiB 和 +0.71 MiB；原生 x86_64 的长期表现仍待验证。[问题报告及原始测量](https://github.com/elliothux/open-compute/issues/201)

**先验证快照复用。** v0.4.0 每次普通请求都重新读取并解析包含模块内容的部署快照。主线已合入的 #165 会复用同一版本、同一路由代次的快照，这能省掉重复解析；它对长期内存曲线的影响还需要实测。[快照缓存改动](https://github.com/elliothux/open-compute/pull/165)

| 检查位置             | 要验证的行为                                                                                                                                                                                                                              |
| -------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| PR 的 Loader 测试    | 同一版本和路由代次的普通请求只读取一次快照。路由代次变化后重新读取，旧条目按现有缓存预算淘汰。复用已有断言。[请求分发](../packages/runtime/tests/loader/dispatch.test.mjs) · [缓存预算](../packages/runtime/tests/loader/shared.test.mjs) |
| Linux 长时间运行测试 | 在同一台原生 x86_64 主机上，对正式包和候选包使用相同的 Worker、绑定和内存上限。优先采用报告者的脱敏应用，分别记录空闲、连续请求、间隔请求三个阶段，覆盖报告中的 8–16 小时窗口。接入[现有发布压力测试](../test/stress/README.md)。         |

本地已通过 10 项 Loader 快照与缓存测试。Linux 采样脚本可按固定时长记录 PID 的启动时间、RSS、私有脏页和 cgroup 匿名内存，读数缺失或 PID 被复用时会留下标记。当前开发机和本机 Docker 均为 arm64，暂无可用的原生 Linux x86_64 隔离主机，长测待执行。[采样命令和输出说明](../test/stress/README.md)

**判定与后续修复。** 对齐请求数、workerd PID、模块实例标识、子进程私有脏页和服务 cgroup 匿名内存。先用空闲对照确定正常波动；若正式包重现持续增长，候选包需在相同窗口内达到稳定范围，期间子进程持续服务且没有重启。若候选包仍增长，在相同请求数处对比 V8 堆与原生分配，定位请求完成后仍持有内存的对象，修复其释放路径并重跑同一对照。若正式包也未重现，保留问题开放并记录应用和环境差异。

## #148　外部反向代理的浏览器验收与主线合入

[P24 实现记录](implemented/p24-external-proxy-worker-origins.md)对应 [#148](https://github.com/elliothux/open-compute/issues/148)。原实现于 2026-11-08 提交。2026-11-22，GitHub `main` 已从 `bca30aa3c` 快进到[实现提交 `d151f89ea`](https://github.com/elliothux/open-compute/commit/d151f89ea2b3b1b4edce00d9a15d299262940cc4)。已从该提交重建单二进制，Caddy HTTPS 回源测试和 Firefox 浏览器验收均通过。浏览器回归已在[提交 `07e01491e`](https://github.com/elliothux/open-compute/commit/07e01491e3ae8eb30a5be1691dd88c8a7319fc9c)合入 GitHub 主线。[回源测试](../crates/service/tests/single_binary/external_ingress.rs)

1. **准备浏览器用例。** 在现有 [Dashboard Playwright 测试](../apps/dashboard/tests/e2e/lifecycle.spec.ts)中部署一个能回显自身标识和请求 URL 的 Worker。关闭受管 Gateway，让测试域名在浏览器端解析到外部 Caddy，信任测试 CA，并通过 Dashboard 绑定一个带非默认 HTTPS 端口的外部 origin。
2. **从页面访问。** 核对 Endpoints 和 Domains and routes 展示的地址包含登记的协议、域名和端口。点击 **Visit**，等待新标签页加载，断言地址栏为登记的 origin 加 `/`、响应成功，且正文中的 Worker 标识和回显 URL 均符合预期。撤销绑定后刷新页面，确认外部地址消失，再点击 **Visit** 应打开该 Worker 的本地地址。
3. **交付浏览器回归。** 验收记录为 Firefox 1/1、Caddy 回源 1/1。绑定后 HTTPS 返回 200；解绑并刷新 Dashboard 后，原外部页返回 404，本地 Visit 返回 200 并回显同一 Worker 的标识。[本地运行命令](references/testing.md)

## #157　让没有 alarm 的空闲 Durable Object 保持休眠

**已定位周期性唤醒的原因。** v0.3.0 和修复前的主线都从对象目录中分页选出 `creating`、`ready` 的对象。调度器到达修复时间后逐个调用 workerd，读到空页就重置游标；下一轮又会遇到同一批对象。workerd 为读取对象内的 alarm 记录会先取得租户对象实例，已休眠的对象也会因此被重新激活。[旧版候选查询](https://github.com/elliothux/open-compute/blob/d151f89ea2b3b1b4edce00d9a15d299262940cc4/crates/storage/src/durable_objects/objects.rs) · [旧版修复循环](https://github.com/elliothux/open-compute/blob/d151f89ea2b3b1b4edce00d9a15d299262940cc4/crates/service/src/scheduler/alarm.rs) · [workerd 调用路径](../packages/runtime/src/durable-objects/host.ts)

报告者在 Linux arm64、无业务流量时观察到 workerd RSS 随唤醒次数上升。代码可以解释唤醒来源；仅凭 RSS 还无法确定哪些分配长期留在内存中。现有测试通过构造次数验证过对象休眠后会被 repair 再次激活。[原始测量](https://github.com/elliothux/open-compute/issues/157) · [已有休眠测试记录](implemented/p0-7-durable-objects.md)

**修复方案。** 对象内已提交的 alarm 行决定是否触发以及何时触发，调度记录是它在调度器中的副本。对象事务可能已经提交，而副本更新失败；只查询已有调度任务会漏掉最需要恢复的 alarm。[事务提交后的同步逻辑](../packages/runtime/src/durable-objects/alarm-shim.ts)

1. 在调度器一侧按 namespace、对象 ID 和对象代次登记可能有 alarm 的对象。首次变更在对象本地能够提交前，须先持久化带操作版本的待核对记录；正常的无 alarm 访问不登记。登记失败时让变更失败，保证每个已提交的 alarm 都有可供恢复的线索。
2. 常规修复只处理待核对记录，以及已登记有 alarm、但调度副本缺失或不一致的对象。同步成功后按操作版本确认；删除、投递完成或重建时，只有确认该代对象已无 alarm 且没有更新操作在途，才能移除登记。修复与未提交事务应串行核对，过期回包不得清掉新标记。进程重启后继续处理未确认项。
3. 升级时旧对象的 alarm 状态未知。追加迁移并安排一次可续跑的分批核对，将进度和未成功的条目持久化；核对完成后退出旧对象扫描。新建且从未设置 alarm 的对象从一开始就跳过周期 repair。

本地实现已追加 V27 数据库迁移。升级时会给已有的活动对象登记一次核对任务，新建对象在首次变更 alarm 时才登记。从未设置 alarm 的对象调用 `storage.deleteAll()` 清空普通数据也不会登记。已通过 24 项调度器存储测试、10 项对象仓库测试及 496 项运行时测试。[V27 迁移](../crates/storage/refinery-migrations/control/V27__do_alarm_repair.sql) · [核对与恢复用例](../crates/storage/src/durable_object_alarm_repair_tests.rs)

**本地验收。** 已扩展并跑通[现有真实调度器 Gate](../crates/service/tests/p0_8_scheduler_do_alarms_gate/p0_8_real_scheduler_alarm_matrix.rs)。让从未设置 alarm、已删除 alarm、仍有待触发 alarm 的对象共存，推进多个修复周期，断言前两组在状态收敛后 repair RPC 和构造次数均无增量，第三组按时触发。另测对象提交后副本写入失败、修复与重新设定交错、事务回滚、删除重建，以及升级核对中途重启；无需新的业务请求也能找回已提交的 alarm。

**Linux arm64 实测。** 在同一台 Apple Silicon 主机的 Linux 虚拟机内分别运行 v0.4.0 正式包和候选包（SHA-256 前缀 `e555ec86a63f`）。每次创建 3 个从未设置 alarm 的对象，随后空闲 100 秒，期间没有租户请求，也没有 alarm 投递。构造次数从对象数据库读取，读取本身不会激活对象。

| 空闲窗口内的变化             |     v0.4.0 |      候选包 |
| ---------------------------- | ---------: | ----------: |
| 周期核对调用                 |          6 |           0 |
| 额外构造次数（三个对象合计） |          6 |           0 |
| workerd RSS                  | +3,836 KiB | −21,532 KiB |
| workerd 私有脏页             | +6,868 KiB |    +156 KiB |

这次对照确认了多余唤醒消失。内存数值只覆盖这 100 秒，长期趋势仍需更长的观察窗口。

## #199　按剩余字节数决定能否创建 Durable Object

本地实现现按数据目录所在文件系统的可用字节数判断磁盘压力。旧版默认值在磁盘用到 85% 时报告健康降级，用到 95% 时拒绝创建新对象。报告中的 926 GiB 磁盘还有 36 GiB 可用，Python Durable Objects Gate 却收到 `DO_STORAGE_LIMIT`。[问题报告](https://github.com/elliothux/open-compute/issues/199)

**配置与判断。** 在 `[durable_objects]` 下增加 `disk_stop_writes_free_bytes`（默认 2 GiB）和 `disk_degraded_free_bytes`（默认 10 GiB）。`config check` 校验两者均为正数且前者小于后者；实例启动时再核对阈值不超过数据目录所在文件系统的总容量。显式使用旧字段 `disk_stop_writes_percent` 或 `disk_high_watermark_percent` 时，配置检查应指出对应的新字段及字节单位。[现有配置](../crates/core/src/config/product_limits.rs)

v0.4.0 的默认安装也会把两个旧字段写入实例配置。使用默认值的实例可在升级前移除这两行：同一份配置已分别在 v0.4.0 正式包和候选版本通过配置检查。使用自定义水位的实例需要选定新的字节值。保留旧字段时，候选版本的升级预检返回 `CONFIG_PARSE_FAILED`。[升级步骤](../apps/website/src/content/docs/zh/docs/ocd/configuration.md)

| 进程可用空间               | 健康和指标状态 | 创建新对象              |
| -------------------------- | -------------- | ----------------------- |
| 达到或高于降级阈值         | 健康，`normal` | 允许                    |
| 达到停止阈值、低于降级阈值 | 降级，`high`   | 允许                    |
| 低于停止阈值               | 降级，`stop`   | 返回 `DO_STORAGE_LIMIT` |

读取数据目录的 `statvfs`，用 `f_bavail × f_frsize` 得到进程可用字节数。复用通用存储准入已有的取值算法，让健康检查和创建入口共用同一份阈值判断。已有对象继续按现行规则解析，其他存储操作仍遵守各自的配额和空间预留。[存储取值](../crates/storage/src/disk_admission.rs) · [对象登记](../crates/storage/src/durable_objects/helpers.rs)

**对外更新。** 同步默认配置、`capabilities --json` 的 limits、运行时配置摘要和中英文配置文档。监控沿用 `oc_do_storage_watermark` 的三个状态标签，说明它们对应的剩余字节区间。[默认配置](../share/default-config.toml) · [运行时摘要](../crates/runtime/src/digest.rs) · [水位指标](../crates/service/src/metrics_do.rs)

**本地验收。** 已用固定的文件系统读数分别测试两个阈值的前一字节、等值、后一字节，以及释放空间后从 `stop` 恢复到 `normal`。断言健康状态、指标和创建结果一致，并覆盖停止状态下解析已有对象。用报告中的 926 GiB 总量、36 GiB 可用作为回归样例；同样的可用空间换到另一总容量，判定应相同。已覆盖旧配置报错和阈值超出磁盘总容量的启动错误。报告中出错的 `python-durable-objects` Gate 已在本机跑通，覆盖 Python 对象的 fetch、RPC、存储、alarm、重启和回滚。
