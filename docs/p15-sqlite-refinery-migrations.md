# P15：SQLite Day 1 独立迁移与 Refinery 收敛

状态：**implemented**（2026-09-14）。P15 是
[GitHub issue #51](https://github.com/elliothux/open-compute/issues/51) 的实现记录。

## 1. 最终模型

每一种平台拥有的 authoritative SQLite database 都有独立的 Refinery lineage：

| Database type | Migration directory | History owner |
| --- | --- | --- |
| `control.sqlite` | `crates/storage/refinery-migrations/control/` | control file 自己的 `refinery_schema_history` |
| `scheduler.sqlite` | `crates/storage/refinery-migrations/scheduler/` | scheduler file 自己的 history |
| `observability.sqlite` | `crates/storage/refinery-migrations/observability/` | observability file 自己的 history |
| KV `data.sqlite` | `crates/storage/refinery-migrations/kv/` | 每个 namespace file 自己的 history |
| D1 `data.sqlite` | `crates/storage/refinery-migrations/d1/` | 每个 database file 自己的 history |
| Vectorize `data.sqlite` | `crates/storage/refinery-migrations/vectorize/` | 每个 index file 自己的 history |
| AI Search `data.sqlite` | `crates/storage/refinery-migrations/ai_search/` | 每个 instance file 自己的 history |

七个 lineage 的 Day 1 baseline 都压平为 `V1__init.sql`。后续 schema 变更只在 owning directory
追加 contiguous `V2`、`V3`……；例如 control 当前已经追加 Worker resource limits 的 V2。已经进入发布版本的
migration filename、order 和 bytes 永久不可修改。

workspace 固定使用 `refinery 0.9.2`，关闭默认 features，只启用 `rusqlite`。生产永远运行到 embedded head，
不接受 caller target、runtime migration slice、`Target::Fake`、down migration 或 grouped migration。

## 2. 唯一接管例外

P15 不提供历史版本升级兼容。唯一例外是 P15 切换前的**精确 current head**：如果旧 database 的版本、已发布
migration checksum、完整平台 schema 和 owner identity 全部与对应 Refinery V1 baseline 一致，则在一个
`EXCLUSIVE` transaction 内把它视为 V1 已执行。

接管过程固定为：

1. 验证旧 current-head marker。control 必须是完整 001–019 ledger 和 `user_version=19`；scheduler 必须是
   完整 001–005 ledger 和 `user_version=5`；observability 与 resource database 必须具有各自精确的旧 current marker；
2. 验证 immutable owner identity、required tables 和旧 ledger checksum；
3. 在同一个 transaction 中删除旧 platform schema marker；D1 tenant migration ledger 和 tenant
   `PRAGMA user_version` 不属于该 marker，保持不变；
4. 从 embedded V1 在隔离的内存 database 建立基准，对 `sqlite_master` 中的 platform tables、indexes、views 和
   triggers 做完整 canonical 比较。D1 只比较 `__open_compute_*` 私有对象，不把 tenant tables 当成 platform schema；
5. 只有比较完全一致才写入 V1 的 Refinery name/checksum history row并提交；随后由 Refinery 正常执行 V1 之后的 pending
   migrations。

任何旧版本、缺行、checksum drift、DDL drift、未知 platform object、future history、错误 identity 或不完整接管都会在
写入 history 前 fail closed。程序不会猜测“最接近”的版本，不 backfill 任意旧数据，不保留 dual reader/write，也不会自动
删除、重命名或重建 authoritative database。

这项窄接管不是持续兼容层：它只识别切换瞬间的一个完整旧 head，并立即删除旧 migration authority。接管后 database
只有 Refinery history。

## 3. Migration 执行和恢复

`crates/storage/src/schema_migrations.rs` 是唯一共享 executor。它只注册七个 embedded directories、验证 history、执行
Refinery runner，并从完整 embedded lineage 建立隔离的内存基准，验证当前 platform-owned `sqlite_master` 与 head 精确一致，
再把错误映射为稳定的 `PlatformError`。D1 比较时仍排除 tenant-owned schema。每个 database owner 负责安全打开、no-follow、
busy timeout、foreign keys、durability、identity、quota、`quick_check` 和产品 invariants。

Refinery 默认每条 migration 一个 transaction。一个 database 的某条 migration 失败时，该条 DDL 和 history row 一起
回滚；之前已经提交的 migration 保留。多个物理 database 之间没有事务、组合 target 或补偿 journal。startup 持有唯一
data-dir lock，先迁移 control，再按 current catalog 串行迁移 scheduler 和各 resource file；任何 file 失败都保持服务
not-ready，下次启动从每个 file 自己的 durable head 继续。

新 database 从空 staging file 执行自己的 V1 到 latest，之后由 owner 写入动态 identity、quota、model contract 等实例
数据并校验，再按原有安全文件合同安装。静态 migration SQL 不包含 instance-specific seed。

## 4. 删除的重复 authority

P15 删除了以下 platform schema authority：

- control 的 active 自定义 runner、`schema_migrations` 和 `PRAGMA user_version`；
- scheduler 的 active 自定义 runner、`scheduler_migrations`、`scheduler_meta.schema_version` 和
  `PRAGMA user_version`；
- observability 的 schema checksum marker 和 `PRAGMA user_version`；
- KV/D1 meta 中的 platform `schema_version`，以及 Vectorize/AI Search instance meta table 中的同类列；
- release identity、release metadata 和 authenticated snapshot manifest 中重复的 control/scheduler/product
  aggregate schema tuple和 control SQL definition registry。

保留的 version 都不是 platform migration authority：public wire/manifest format、Worker bundle/descriptor、object/backup
format、workerd capability、resource driver contract、D1 tenant `PRAGMA user_version`、D1 tenant migration ledger，以及
业务 generation/revision。`resources.driver_schema_version` 仍覆盖 R2、Durable Objects 等非 SQLite driver contract；
各 SQLite file 的迁移进度不从该字段推断，而只读该 file 自己的 Refinery history。

Snapshot 不再保存或比较 `source_schemas`。snapshot 自带 authoritative SQLite files；restore 对 control、scheduler 和每个
resource file 分别验证 embedded Refinery history、owner identity 与 current invariants。旧 manifest JSON 不保留 decoder。

旧已发布 SQL 文件继续留在原路径，并由 build-time SHA-256 wiring 作为冻结证据；它们只用于验证上述精确 current-head
接管，不再是 active runner，也不出现在 release capability 输出中。

## 5. 不在 P15 范围内的 SQLite

- D1 `__open_compute_migrations` 是 tenant migration ledger，不属于 platform Refinery lineage；
- D1 tenant `PRAGMA user_version` 继续是 tenant-owned state；
- response cache 和 AI Search parse cache 是 disposable acceleration；
- workerd Durable Object storage 仍由 pinned workerd 独占，`crates/storage` 不打开或迁移它。

## 6. 验收证据

实现包含以下回归边界：

- fresh data-dir 和 fresh KV/D1/Vectorize/AI Search file 只通过对应 embedded Refinery lineage 创建；
- verified legacy current head 的接管、旧 ledger 删除和 identity/data 保留；
- malformed、divergent 和 future Refinery history fail closed；
- 接管 transaction 在 fault boundary 前不留下半写 history，已提交 head 在 restart 后不重放；
- D1 tenant SQL export/import 不复制 platform history，tenant authorizer 保护
  `refinery_schema_history`；
- release capability和 snapshot manifest 不再包含 aggregate SQLite schema tuple。

真实 binary 验收使用全局安装的 pre-P15 `ocd 0.1.7` 创建 data-dir，并创建 KV namespace/value、D1 database/table/row、
Vectorize index/mutation 和 R2 bucket；随后直接启动开发版 `target/debug/ocd` 完成接管并达到 ready。control、scheduler、
observability、KV、D1 和 Vectorize 各自写入独立 Refinery history，旧 tracker/marker 被移除；通过开发版 HTTP API 读取的
KV value 和 D1 row 与接管前一致。开发版也从不存在的 data-dir 完成 clean initialization，再新建 KV、D1 和 Vectorize
resource，并验证所有新文件只含对应 embedded Refinery history。测试 artifact 位于忽略的 `.temp/p15-integration/`。

最终接受仍按仓库合同执行 format、Clippy、no-default-features、MSRV、metadata、dependency boundaries、coverage，最后且仅
最后执行一次 `./test/gate.py --workspace`。
