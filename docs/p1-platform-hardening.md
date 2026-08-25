# P1：P0 平台加固详细设计

> 状态：详细设计，待实现
>
> 基线：P0.1 至 P0.8 以及 P0 aggregate Gate 已由用户确认在当前 checkout 跑通（2026-08-26）
>
> 直接依赖：[P0.1：Platform Foundation](./p0-1-platform-foundation.md)、
> [P0.2：Workers Runtime](./p0-2-workers-runtime.md)、
> [P0.3：Resource 与 Binding Framework](./p0-3-resource-binding-framework.md)、
> [P0.4：KV](./p0-4-kv.md)、[P0.5：R2](./p0-5-r2.md)、
> [P0.6：D1](./p0-6-d1.md)、[P0.7：Durable Objects](./p0-7-durable-objects.md)、
> [P0.8：Scheduler Kernel 与 DO Alarms](./p0-8-scheduler-do-alarms.md)
>
> 后续消费者：P2 Queue、Cron 和 Workflow 必须建立在 P1.0 至 P1.7 的稳定性 Gate 上；
> P1.8 WebSocket hibernation 是条件性兼容增强，不阻塞 scheduler workload 进入 P2。

P0 已经证明 Workers、KV、R2、D1、Durable Objects、alarms 和 basic WebSocket 可以在一个
SQLite-only、单节点、stock workerd 平台中组合运行。P1 不增加新的 Cloudflare 产品，而是把这套
“功能已跑通”的系统收敛成可以面向 SMB self-deploy 的发行版：兼容边界可查询、磁盘写入可控、整机
状态可离线快照和 fresh-host 恢复、升级可演练、恶意 Worker 不能越界、长稳和崩溃恢复有明确证据，
运维人员有一套短而完整的命令和 runbook。

P1 的核心取舍是接受单节点现实。平台整机快照与恢复使用显式停机 CLI 和短维护窗口，不实现多库、
workerd `localDisk` 与外部 S3 之间的在线一致性协议。KV/D1 已有的单资源在线 backup/restore 保持不变；
整机灾备追求简单、可验证和可恢复，而不是零停机。

## 0. P1 决策摘要

| 决策 | P1 选择 | 原因 |
| --- | --- | --- |
| 产品范围 | 不新增 Queue、Workflow、Cron 或管理面产品 | 先稳定 P0，再扩大 scheduler workload |
| 整机快照 | daemon 停止后由 offline CLI 创建 | 一条单机锁即可获得所有本地状态的一致切面 |
| 整机恢复 | daemon 停止、目标 `data_dir` 为空、先恢复到 staging | 禁止对现有数据目录原地覆盖 |
| SQLite 快照 | 项目拥有的 DB 使用 SQLite Online Backup API | 不裸拷贝 live DB、WAL 或 SHM |
| DO 快照 | workerd 停止后，按普通文件不透明复制整个 `localDisk` | 不解释、不改写 upstream 内部布局 |
| R2 数据 | 不重复复制；快照绑定原 S3 authority 和 R2 prefix | P0 R2 没有 object catalog；P1 不提供 R2 point-in-time recovery |
| immutable system object | bundle、assets、ready KV/D1 backup 不重复复制；manifest 引用会 pin object | 避免重复字节，同时保证历史 snapshot 可恢复 |
| master key | 快照只记录 fingerprint，不包含 key bytes | 密钥必须由 operator 独立备份和注入 |
| 备份保密性 | 不提供；SQLite/DO snapshot object 使用明文字节 | P1 只保证完整性、真实性和可恢复性 |
| snapshot authority | S3 中最后写入的 versioned manifest | 避免控制库中的自引用和半完成状态 |
| upgrade | offline、forward-only、可重复执行；升级前必须有已验证快照 | 多个 SQLite 文件无法做一次跨文件事务 |
| quota | 资源数量、现有 per-resource quota、host disk protection | 不伪造 Cloudflare billing 或无法精确计算的 DO/R2 聚合字节 |
| hibernation | 先过 pinned stock-workerd facet Gate；失败则保持 basic WebSocket | 不在 SQLite/gateway 中重做物理 socket runtime |

### 0.1 P1 必须守住的不变量

1. P1 不改变 P0 的 authority 分工：`control.sqlite` 管控制面，资源 SQLite 管资源数据，DO
   `localDisk` 管 object state，外部 S3 管 R2 和 immutable artifacts，`scheduler.sqlite` 只是
   alarm due projection。
2. daemon、offline command 和第二个 daemon 不能同时拥有同一个 `data_dir`。
3. 已提交的 snapshot 只能由最后写入、完整校验过的 manifest 表示；没有 manifest 的上传前缀不是备份。
4. restore 在全部 object hash、manifest MAC、schema、release、master-key 和 S3 authority preflight
   通过前，不得修改目标数据目录。
5. schema migration 只向前；migration 之后的 binary rollback 必须恢复升级前 snapshot，不能直接运行旧
   binary 读取新 schema。
6. host 磁盘低于 hard reserve 后，任何可能扩大本地状态的请求都 fail closed；delete、GC、诊断和恢复
   空间的操作仍有独立 emergency reserve。
7. fuzz、load 和 fault injection 只存在于 test harness，不在 production binary 暴露任意 crash point。
8. operator 输出、metrics、snapshot manifest 和 support bundle 不包含 master key、S3 credential、
   tenant secret、authorization header 或 Worker request body。

### 0.2 非目标

- Queue、Workflow、Cron、DLQ 或通用 task graph；
- 多节点复制、leader election、在线跨节点迁移或 zero-downtime full snapshot；
- 把平台 snapshot 恢复到另一套 S3 provider/bucket/prefix；
- 自动备份、托管、轮换或恢复 master key；
- R2 全量列举、portable export、PITR 或跨 provider copy；
- snapshot object 的 client-side encryption、provider-side encryption 检查或任何备份保密性保证；
- Cloudflare plan quota、billing、全球 KV 一致性和 Durable Object 全球 placement；
- Durable Object class rename、migration tag、object transfer 或 upstream storage format migration；
- fork workerd、运行时自动下载 workerd 或接受未锁定的系统 workerd；
- 在 platformd、SQLite 或 gateway 中模拟 WebSocket hibernation；
- 完整浏览器 Web Platform conformance；P1 只覆盖平台装配会影响的原生 API 和项目提供的 facade。

## 1. 交付架构与依赖顺序

```text
P0 verified baseline
        │
        ▼
P1.0 capability / format freeze
        │
        ▼
P1.1 quota, disk admission, offline operation gate
        │
        ▼
P1.2 platform snapshot create
        │
        ▼
P1.3 fresh-host restore
        │
        ▼
P1.4 forward-only upgrade and rollback rehearsal
        │
        ├──────────────┐
        ▼              ▼
P1.5 security      P1.6 soak/load/crash
        └──────────────┬──────────────┘
                       ▼
                 P1.7 ops release gate

P1.8 native WebSocket hibernation Gate runs last and is conditional.
```

| 阶段 | 产出 | 为什么必须在此时做 |
| --- | --- | --- |
| P1.0 | capability manifest、API conformance、格式身份 | snapshot 和 upgrade 必须先知道“什么格式、什么行为” |
| P1.1 | 统一写入准入、空间 reservation、offline data-dir lock | 快照、恢复和故障测试都依赖可阻止新写入的边界 |
| P1.2 | 可验证的整机 snapshot | forward migration 之前先有可靠 rollback anchor |
| P1.3 | fresh-host restore | 只创建过备份不等于有灾备能力 |
| P1.4 | schema/workerd/release upgrade rehearsal | 复用已经验证的 snapshot/restore 做唯一 downgrade 路径 |
| P1.5 | 恶意输入、恶意 Worker、隔离与 secret hygiene | 在行为和格式冻结后针对真实 attack surface 加固 |
| P1.6 | soak、load、crash matrix 和 capacity envelope | 在恢复路径完整后验证长期与组合故障 |
| P1.7 | health、metrics、doctor、support bundle、runbook | 汇总前面各阶段已经存在的证据，不另造一套状态系统 |
| P1.8 | hibernatable WebSocket 条件性能力 | upstream facet 行为不应绑架核心稳定性发布 |

### 1.1 发行身份

P1 中任何 conformance result、snapshot、restore 或 upgrade 都必须绑定同一组发行身份：

```text
PlatformReleaseIdentityV1
├── platformd semantic version + git revision
├── Rust MSRV
├── workerd version
├── workerd.lock.json sha256
├── packaged runtime assets digest
├── system Worker/facade capability versions
├── control.sqlite schema version
├── scheduler.sqlite schema version
├── KV/D1 resource schema version ranges
├── snapshot format version
└── supported compatibility date / flag policy digest
```

这些字段由 production constants、migration registry 和 packaged assets 生成，不能在测试代码里维护第二套
手写真相。`platformd capabilities --json` 输出 versioned JSON；测试保存预期结构和关键值，不保存可能漂移
的时间戳或绝对路径。

### 1.2 P1 不新增一份业务数据库

P1 不创建 `backup.sqlite`、`operations.sqlite` 或 snapshot catalog 表：

- full snapshot 的 commit authority 是外部 S3 manifest；
- offline command 通过现有 data-dir exclusive lock 获得所有权；
- staging reservation 以单进程内存计数和 project-owned staging directory 为准；
- 最近一次 snapshot/restore 只写非权威、可丢失的 atomic receipt 文件，供 doctor/metrics 使用；
- forward migration 继续使用每个 DB 已有的 build-time migration checksum 和 schema history；
- crash 后 `upgrade apply` 扫描每个数据库的实际 schema，幂等地继续未完成工作。

这样不会出现“控制库说 snapshot ready，但 manifest 没写完”或“snapshot 内的控制库记录自己尚未完成”
的自引用问题。

## 2. P1.0：Capability Freeze 与 API Conformance

P0 的测试证明组合路径可运行，P1.0 要把它升级成可查询、可比较的产品合同。Cloudflare 的
compatibility date 和 flags 会随日期改变默认行为；平台不能把“使用 stock workerd”等同于“完整兼容
Cloudflare”。每个发行版必须声明自己实际支持的 surface、限制和偏差。

### 2.1 `PlatformCapabilitiesV1`

新增只读 CLI 输出：

```bash
platformd --config /absolute/platform.toml capabilities --json
```

输出至少包含：

- release identity；
- Workers fetch/RPC、streams、WebSocket、outbound fetch 支持状态；
- KV、R2、D1、DO、alarms 的 facade capability version；
- 每个 binding 支持的方法、参数形态、返回类型和已知偏差 ID；
- 支持的 compatibility date 范围和 flag allowlist/denylist；
- resource/deployment frozen limit 名称，不输出 secret value；
- basic WebSocket 与 hibernatable WebSocket 分开标记；
- P2 产品一律标记 `unsupported`，不能返回空对象暗示可用。

建议结构：

```json
{
  "schema_version": 1,
  "release": {},
  "runtime": {
    "compatibility_date_min": "...",
    "compatibility_date_max": "...",
    "allowed_flags": [],
    "workerd_lock_sha256": "..."
  },
  "products": {
    "workers": { "capability_version": 1, "status": "supported" },
    "kv": { "capability_version": 1, "status": "supported", "deviations": [] },
    "r2": { "capability_version": 1, "status": "supported", "deviations": [] },
    "d1": { "capability_version": 1, "status": "supported", "deviations": [] },
    "durable_objects": {
      "capability_version": 1,
      "basic_websocket": "supported",
      "hibernatable_websocket": "unsupported"
    },
    "queues": { "status": "unsupported" },
    "workflows": { "status": "unsupported" }
  }
}
```

### 2.2 Conformance authority

证据优先级固定为：

1. 当前 Cloudflare 官方 runtime API、product API 和 compatibility 文档；
2. pinned workerd source 与实际 stock-workerd black-box behavior；
3. 当前 `@cloudflare/workers-types`，只用于 TypeScript surface 对照；
4. Miniflare/WDL 的实现和测试，用于发现 edge case；
5. 项目自己的明确偏差。

Miniflare 或 WDL 不是规范。官方文档与 pinned workerd 不一致时，先记录差异并用黑盒 Gate 决定本发行版
能否提供该能力；不能用 adapter 静默伪造未验证语义。

### 2.3 Suite 范围

每个 facade method 都至少覆盖：

| 类别 | 必测内容 |
| --- | --- |
| shape | method 是否存在、prototype/own property、sync/async、参数默认值 |
| value | string/bytes/JSON/Date/structured clone/stream 的确切返回形态 |
| limits | 0、边界值、边界值 + 1、过深/过大/非法 UTF-8 |
| errors | JS error class、稳定 message fragment、platform error code、HTTP status |
| lifecycle | resource active/deleting/deleted、deployment A/B/rollback、restart |
| isolation | account/resource/deployment ID 交叉、stale token/generation |
| compatibility | min/max date、每个 allowlisted flag、未知/冲突 flag |
| cancellation | client abort、stream cancel、workerd exit、platform shutdown |
| security shape | getter/proxy trap、`__proto__`、constructor、symbol、header spoof |

Workers 原生 Web APIs 不做完整 WPT。只回归平台 config、loader、proxy、outbound policy 和 stream bridge
可能改变的部分，例如 `Request`/`Response` body、`Headers`、URL、AbortSignal、WebSocket 和 structured
clone。

### 2.4 Fixture 组织

```text
crates/service/tests/fixtures/p1-conformance/
├── workers.mjs
├── kv.mjs
├── r2.mjs
├── d1.mjs
├── durable-objects.mjs
├── alarms.mjs
├── websocket.mjs
└── adversarial-values.mjs

crates/service/tests/p1_conformance.rs
scripts/test-p1-conformance.sh
```

runner 必须：

- 使用 production packaging、loader、facade 和 stock workerd；
- 不需要 Cloudflare account、Wrangler 或外网；
- 每轮创建 fresh data-dir 和 mock/temporary S3；
- 把随机 ID、端口、时间戳和临时路径规范化后再比较结果；
- 同一发行版重复运行得到相同 verdict；
- 每个差异都有稳定 deviation ID 和文档，不以 snapshot update 隐藏行为变化。

### 2.5 P1.0 Exit Gate

- `platformd capabilities --json` 可由生产代码生成并通过 schema validation；
- P0 所有公开 facade method 都有 shape/value/error/lifecycle 测试；
- compatibility date/flag allowlist 有 min/max/unknown/conflict Gate；
- P0 API matrix 与 capability output 不存在无 owner 的 `partial` 或 `unknown`；
- 每个偏差都能从 capability output 链接到本地 deviation 文档；
- workerd lock、facade source 或 migration digest 改变会使 conformance cache 失效；
- `scripts/test-p1-conformance.sh` fresh-process 三轮一致通过。

## 3. P1.1：Quota、磁盘保护与 Offline Operation Gate

P0 已经有 KV/D1/R2/DO/scheduler 各自的 size、concurrency、queue 或磁盘阈值。P1.1 不把这些实现
重写成中央 quota service，而是在所有“可能扩大本地状态”的入口前加一层统一 admission snapshot，并补齐
资源数量上限和离线操作所有权。

### 3.1 Quota 边界

P1 支持：

- account 下 Workers、routes、deployments、KV namespaces、R2 buckets、D1 databases、DO namespaces
  的可配置数量上限；
- P0 已冻结在资源上的 KV/D1 logical byte quota；
- R2 单 object、stream、multipart/staging 和并发限制；
- DO dispatch、object activation、host `localDisk` 和全局磁盘保护；
- scheduler ready/in-flight/repair batch 限制；
- backup/snapshot/restore staging 的独立 reservation。

P1 不支持：

- DO 每 object 或每 namespace 的精确字节 quota；项目不能解释 upstream `localDisk` 内部 accounting；
- R2 account logical byte quota；P0 没有完整 object catalog，实际配额由外部 S3 provider 决定；
- 跨 KV/D1/DO 的 account 聚合字节账单；
- Cloudflare plan 名称和 billing semantics。

### 3.2 统一写入分类

| 请求 | soft pressure | hard pressure | emergency reserve 内允许 |
| --- | --- | --- | --- |
| deploy/create/put/D1 mutation/DO 新写入 | 按估算空间和资源 quota 准入 | 拒绝 | 否 |
| R2 upload staging | 必须先 reserve 最大本地 staging | 拒绝 | 否 |
| platform snapshot | 先计算本地 staging 与 S3 能力 | 拒绝并提示清理 | 否 |
| restore | 目标必须为空并预留完整 manifest size + margin | 拒绝 | 否 |
| delete/GC/temp cleanup | 允许 | 允许 | 是 |
| catalog/KV/R2 read | 允许 | bounded 允许 | 是 |
| D1 read | 禁止 disk temp spill，预算不足则拒绝 | bounded 或拒绝 | 仅小预算 |
| doctor/metrics/support metadata | 允许 | 允许 | 是 |

“读”不天然等于零写入：SQLite 可能创建 WAL/SHM，SQL sort 可能使用临时空间，R2 response 可能经过本地
staging。每条 read path 都必须证明不扩大磁盘，或从 emergency reserve 记账。

### 3.3 Admission snapshot

每次有界 mutation 在进入业务层前读取一次不可变 snapshot：

```text
AdmissionSnapshotV1
├── filesystem free bytes
├── configured soft/hard/emergency reserve
├── in-memory reserved bytes by operation class
├── owned staging bytes observed on disk
├── resource/account count and frozen quota
├── product queue depth / concurrency
└── current platform mode: serving | draining | offline
```

同一请求只使用这一份 decision，不能在 KV、S3 adapter 和 response bridge 各自用不同磁盘值重新判断。
实际写入仍由产品级 hard limit 二次约束。

Reservation 规则：

1. 在创建 staging file 或 SQLite transaction 前原子增加内存 reservation；
2. reservation 上限使用输入 content length、配置 hard limit 或保守最大值，不信任 tenant header；
3. stream 每写入固定 chunk 后校正实际值，超过 reservation/limit 立即 cancel；
4. success、error、abort 和 panic path 都幂等释放；
5. crash 后内存 reservation 自然消失，startup 只清理 project-owned、格式可验证、超过 grace period 的
   staging directory；
6. 不把 reservation 写入 SQLite，因为磁盘耗尽时不能依赖一次新的 DB write 才能释放空间。

### 3.4 稳定失败语义

- product quota 超限：稳定 `quota_exceeded`；
- bounded queue 饱和：稳定 `admission_busy`，HTTP path 映射 429；
- host hard reserve：稳定 `storage_pressure`，HTTP path 映射 507；
- daemon 正在关闭或 data-dir 被 offline command 占用：稳定 `platform_unavailable`；
- S3 provider quota/limit：保留 provider-independent public code，原始 provider message 只进入 redacted log。

失败必须发生在 tenant mutation 提交前。不能在 SQLite 已 commit 后因 response serialization 失败而回报
“未写入”；需要沿用各 P0 产品已有的 commit/response 语义。

### 3.5 Offline data-dir ownership

从 `DataDir::acquire()` 中抽出可复用的跨进程 exclusive-lock primitive；offline command 复用锁协议，
但不能复用“首次启动时生成 master key、打开并 migrate DB”的副作用：

```bash
platformd --config /absolute/platform.toml backup create --name nightly-20260826
platformd --config /absolute/platform.toml backup restore --snapshot <id>
platformd --config /absolute/platform.toml upgrade check --release /absolute/release
platformd --config /absolute/platform.toml upgrade apply --release /absolute/release
```

V1 规则：

- daemon 运行时，full snapshot、restore、backup delete/retention 和 schema upgrade 一律快速失败并返回
  owner information；`backup list/inspect` 是无 data-dir mutation 的例外；
- offline command 不通过 admin HTTP 要求 live daemon 自我暂停；
- operator 必须先停止 service，执行命令，再启动 service；
- per-resource KV/D1 online backup 不受此限制；
- snapshot/create/check/list/inspect 以 inspect-existing 模式加载 source，不生成 key、不应用 migration；
- 只有 `upgrade apply` 可以执行 migration；restore 只向新的 sibling staging 写入；
- command 获得 data-dir lock 后仍要检查 orphan workerd/lease identity，不能只按 PID 猜测；
- command 收到 SIGINT/SIGTERM 时停止创建新对象、清理自己拥有的 staging，保留可诊断的失败 receipt；
- `platformd run` 也必须持有同一把锁直至所有 workerd 子进程已终止。

这是一条刻意的单机运维边界。未来若需要 online full snapshot，应作为新的设计，不在 P1 暗中加入可逆
maintenance state machine。

### 3.6 P1.1 Exit Gate

- 每个 P0 storage-growing entrypoint 都经过同一 admission layer；
- soft/hard/emergency 三段阈值有 bytes-on-disk 黑盒测试；
- 并发 stream reservation 不超卖，abort/restart 后 reservation 与 staging 可回收；
- hard pressure 下 create/write 被拒绝，delete/GC/doctor 仍能释放和诊断空间；
- account resource count 与 per-resource frozen quota 在并发 create/update 下不越界；
- daemon、snapshot、restore、upgrade 两两竞争同一 data-dir 时只有一个 owner；
- 不通过篡改 PID 文件、symlink 或 stale socket 获取 ownership。

## 4. P1.2：Platform Snapshot Create

P1.2 定义“platform snapshot”：它包含恢复整个**本地 authority** 所需的全部本地持久状态，并引用同一个
外部 S3 中已经存在的 R2 和 immutable artifact。它不是把外部 S3 再完整复制一遍的 portable archive，
也不是 R2 的 point-in-time backup。恢复结果是 snapshot 时刻的本地状态加 restore 时仍然存在的外部
S3 authority；这是必须对 operator 明示的能力边界，不能简称为“所有产品的 PITR”。

### 4.1 CLI 与权限

```bash
platformd --config /absolute/platform.toml \
  backup create --name before-upgrade-20260826 --json

platformd --config /absolute/platform.toml \
  backup list --json

platformd --config /absolute/platform.toml \
  backup inspect --snapshot <snapshot-id> --verify --json

platformd --config /absolute/platform.toml \
  backup delete --snapshot <snapshot-id>
```

- `create` 必须 offline 并持有 data-dir lock；
- `list`/`inspect` 可只读访问 S3，但仍需加载并验证 config/master-key；
- `delete` 是 offline、显式破坏性命令，必须只删除 manifest 精确列出的 object keys，再删除 manifest；
- CLI 不接受任意 S3 prefix、任意 local source path 或 shell glob；
- snapshot name 只是 operator label，不参与 object key 路径；真正 key 使用随机 snapshot ID。

### 4.2 Include / exclude matrix

| 状态 | Snapshot 行为 | Restore 行为 |
| --- | --- | --- |
| `control.sqlite` | SQLite Backup API 生成独立文件 | 校验后安装 |
| `scheduler.sqlite` | SQLite Backup API 生成独立文件 | 校验后安装；expired lease 由 scheduler 恢复 |
| 每个 KV SQLite | 逐资源 Backup API，记录 resource ID/schema | 恢复原资源路径和 mode |
| 每个 D1 SQLite | 逐资源 Backup API，记录 resource ID/schema | 恢复原资源路径和 mode |
| DO `localDisk` | workerd 已停止后，不透明枚举 regular files | 不解释内容，恢复完整相对路径树 |
| alarm | DO authority 与 scheduler projection 随上面两类一起进入 | token/generation fence 保持不变 |
| R2 object bytes | 不复制；记录每个 bucket marker 与 S3/R2 authority fingerprint | 使用 restore 时的当前 object 状态，不回滚到 snapshot 时刻 |
| Worker bundle/assets、ready KV/D1 backup | 不复制；枚举、`HEAD` 校验并由 committed manifest pin | 使用同一 system prefix；snapshot 删除后才可 unpin |
| packaged workerd/system assets | 只记录 release digest，不复制 binary | 由 exact source release package 提供 |
| redacted effective config | 记录影响语义的 policy digest/非 secret 值 | operator 提供新路径下等价 config |
| master key | **不包含**，只记录 fingerprint | operator 独立提供同一 key |
| S3 credentials/tenant secrets | **不包含** | 由 config/env/key file 和 control secret ciphertext 恢复 |
| logs/metrics/diagnostics | 默认不包含 | 不恢复 |
| PID/socket/lock/lease files | 不包含 | 重新创建 |
| cache/temp/staging | 不包含 | 重新生成 |

### 4.3 Snapshot S3 layout

```text
<system-prefix>/snapshots/v1/<platform-id>/<snapshot-id>/
├── objects/
│   ├── 000001.bin
│   ├── 000002.bin
│   └── ...
└── manifest.json       # 最后写入；唯一 commit point
```

每个 object 属于一个 snapshot，不做跨 snapshot dedupe。这会多占少量控制状态备份空间，但 GC 不需要
全局 reference count，也不会因为删除一个 snapshot 破坏另一个 snapshot。R2 和 bundle 本来就在 S3，
不进入这些 objects。

`system-prefix` 与 tenant R2 prefix 必须继续使用 P0.1/P0.5 已验证的不相交规则。snapshot code 只接受
平台生成的 canonical key，不接受用户拼接 path。

### 4.4 `PlatformSnapshotManifestV1`

manifest 使用 canonical JSON，至少包含：

```json
{
  "schema_version": 1,
  "snapshot_id": "...",
  "platform_id": "...",
  "label": "before-upgrade-20260826",
  "created_at": "...",
  "source_release": {},
  "source_schemas": {},
  "master_key_fingerprint": "...",
  "s3_authority_fingerprint": "...",
  "r2_prefix_fingerprint": "...",
  "immutable_references": [
    { "role": "worker_bundle", "sha256": "...", "object_key": "...", "size": 123 }
  ],
  "files": [
    {
      "role": "control_sqlite",
      "logical_id": "control",
      "restore_path": "control.sqlite",
      "object_key": ".../objects/000001.bin",
      "size": 123,
      "sha256": "...",
      "mode": 384
    }
  ],
  "totals": { "files": 1, "bytes": 123 },
  "manifest_mac": "..."
}
```

约束：

- `restore_path` 只能是 data-dir 下 project-owned allowlisted root 的规范相对路径；
- 拒绝 absolute path、`..`、空 segment、NUL、symlink、hardlink、device 和 socket；
- object count、单文件大小、总字节和 manifest 大小都有 operator-configured hard cap；
- 每个 object 使用 SHA-256 校验，不能使用 multipart ETag 作为内容 hash；
- `manifest_mac` 使用 master key 派生出的 domain-separated HMAC-SHA-256 key；
- key derivation 复用项目既有 HKDF-SHA-256，使用 platform ID 作为 salt、
  `open-compute/platform-snapshot-manifest/v1` 作为唯一 info；MAC 输入是移除
  `manifest_mac` 字段后的 canonical JSON bytes；
- 派生只用于 snapshot manifest authentication，不改变已有 secret encryption key；
- manifest 只包含 endpoint/bucket/prefix 的 canonical fingerprint，不包含 credential；
- snapshot object 不做 client-side encryption，也不要求、配置或验证 provider-side encryption；
  control/KV/D1 SQLite 和 DO file 按原始字节上传，P1 backup 不提供保密性；
- control DB 中的 Worker secret 因 P0.1 原有数据模型仍是 AEAD ciphertext，但这不是 P1 backup
  提供的加密能力；KV/D1 tenant data 与 DO storage 可在 snapshot object 中以明文出现；
- mode 只保留 owner 所需的普通文件 permission bits，清除 setuid/setgid/sticky；
- timestamp 只用于审计和 retention，不参与 correctness/fencing。

HMAC 不能防止同时掌握 master key 和 S3 write permission 的攻击者，但能区分传输/存储损坏、错误 key
和只有 S3 write permission 的 manifest 篡改。

### 4.5 Create protocol

1. 以 inspect-existing 模式加载绝对 config，验证 master key fingerprint、S3 capability 和 platform
   identity；不得生成 key 或执行 migration；
2. 获得 data-dir exclusive lock，确认 daemon/workerd 不在运行；
3. 扫描 owned staging，按格式和 grace period 清理旧的未提交 snapshot 上传；
4. 计算资源数量、预估本地 staging/S3 objects，检查 hard limit 和本地 headroom；
5. 以只读 control authority 枚举 active/deleting resources、deployment artifacts 和期望文件；
6. 对 control、scheduler、每个 KV/D1 运行 SQLite Online Backup API，输出到本地 snapshot staging；
7. 对项目拥有的 SQLite backup 运行 `quick_check`、schema checksum 和 platform-owned metadata checks；
   D1 tenant schema 不增加超出 P0.6 的阻断性检查；
8. 不透明枚举已 quiesce 的 DO `localDisk`；只接受 regular file，逐文件计算 size/hash；
9. 对 control 引用的每个 Worker bundle/assets 和 ready KV/D1 backup object 执行 bounded `HEAD`，
   验证 key/hash/size，并写入 `immutable_references`；
10. 验证当前 S3 authority、R2 prefix fingerprint 和每个 active bucket marker；记录 marker hash，
    不尝试列出或复制全部 R2 object；
11. 上传所有 snapshot object；上传后以 provider-independent read/head 验证 size 和 checksum；
12. 生成 canonical manifest、计算 MAC，**最后** put `manifest.json`；
13. 再读回 manifest，验证 MAC 和所有 object metadata；
14. atomic 写入 `data/operations/last-snapshot.json` 非权威 receipt，清理本地 staging，释放 lock。

只要第 12 步没有完成，snapshot 就不能被 `list`、`restore` 或 retention 当作 ready。失败上传在 grace period
后由 exact-layout GC 清理；不能对 `<system-prefix>/snapshots/` 做未界定的递归删除。

### 4.6 一致性说明

daemon 停止后：

- control/KV/D1/scheduler 不再接受新 transaction；
- workerd 已停止，不再写 DO `localDisk`；
- clean shutdown 尚未完成的 scheduler claim 要么已 drain，要么带 lease 留在 scheduler DB，恢复后按
  P0.8 token/expiry 规则继续；
- immutable artifact 已在外部 S3 原子提交，control 只引用已经 durable 的 object；
- R2 logical key 会被后续 put/delete 原地改变；platform snapshot 不冻结这部分状态。restore 只验证
  snapshot control 中每个 bucket 的 identity marker 仍存在且匹配，再使用 provider 当前 object state；
- snapshot create 自己不修改 product authority。

因此各本地文件的备份时间虽不是同一个 SQLite transaction，却处在同一个无写入维护窗口内。P1 不声称
它是在线 global transaction。

### 4.7 Retention 与 GC

- retention 只考虑有合法 MAC 的 committed manifest；
- artifact/backup GC 在删除任何 immutable system object 前，必须把所有 committed snapshot manifest 的
  `immutable_references` 合并为 pin set；list/download/MAC 任一步失败时 GC fail closed；
- deployment cleanup 和单资源 backup delete 只移除 live reference/row，物理 object 删除统一经过上述
  pin-aware GC，不能绕过 snapshot pin 直接 delete；
- 支持按 `keep_last`、`max_age` 和 operator label policy 生成 dry-run plan；
- delete 必须逐个匹配 manifest object key、platform ID 和 snapshot ID；
- manifest 最后删除；中途失败可幂等继续；
- manifest 损坏时不自动删除其 objects，先 quarantine/report；
- incomplete prefix 只有同时满足合法 layout、无 manifest、超过 grace period才可回收；
- 每次 GC 输出 count/bytes，不输出 tenant object key 或 secrets。

### 4.8 P1.2 Exit Gate

- 组合 P0 fixture 的 control、KV、D1、DO、alarm 状态可进入同一个 snapshot；
- snapshot 时 daemon/workerd 竞争 lock 必定失败；
- 每个 SQLite backup 可打开并通过 project-owned integrity/schema checks；
- DO tree 不被解释或改写，symlink/device/path traversal fail closed；
- missing artifact、S3 short read、wrong size/hash、upload timeout 不会产生 committed manifest；
- snapshot 之后删除 deployment 或 ready KV/D1 backup 不会删除被 committed manifest pin 的 immutable
  object；snapshot 删除后，只有不再被 live control/其他 snapshot 引用的 object 才可 GC；
- snapshot/restore 都验证 active R2 bucket marker；marker 缺失或 identity 不匹配时 fail closed；
- manifest last-put crash matrix 的每个点都只产生“完整 ready”或“不可见 incomplete”；
- wrong master key、MAC 篡改、object 篡改可被 `backup inspect --verify` 检出；
- retention/delete 只删除目标 snapshot 的精确 objects。

## 5. P1.3：Fresh-host Restore 与灾备演练

备份只有在全新目录、全新进程中真正恢复并跑过 P0 smoke 才算可用。P1.3 不允许“把备份下载回来算
验证完成”，也不允许在原数据目录上覆盖几个文件后继续启动。

### 5.1 Restore compatibility

P1 V1 固定：

- restore 使用 snapshot 的 exact `source_release`，或该发行版显式列出的 restore-compatible patch release；
- 恢复完成后，如需新版本，再走 P1.4 正常 forward upgrade；
- 不直接把旧 snapshot 解包成任意最新 schema；
- 需要相同 master key、S3 endpoint/region/bucket/system prefix/R2 prefix authority；R2 使用 restore
  时 provider 中的当前状态，不提供 snapshot-time rollback；
- 主机绝对路径、端口和 TLS certificate 可以变化，只要 redacted policy 和 authority preflight 通过；
- snapshot 不能恢复 S3 provider 本身，也不能恢复丢失的 master key；
- fresh-host restore 时 master key 必须通过环境变量或 data-dir 外的绝对 recovery key file 提供。
  如果原部署使用默认 `<data-dir>/keys/master.key`，operator 必须事先把它独立备份，并在 recovery config
  中先指向 data-dir 外的副本；restore 不把 key 写进 snapshot 或目标 staging。

### 5.2 Restore protocol

```bash
platformd --config /absolute/platform.toml \
  backup restore --snapshot <snapshot-id> --json
```

1. 加载 source-compatible release 和 absolute config；
2. 要求 configured `data_dir` 不存在或为空，拒绝 `--force` 原地覆盖；
3. 获得目标父目录下的 restore ownership lock，验证 target path 不经过 symlink；
4. 下载 manifest，验证 schema、platform ID、release identity、MAC、master-key 和 S3 authority；
5. 对 file count/size/path 做 hard-cap preflight，计算 staging + installed copy 所需 headroom；
6. 创建与目标同一 filesystem 的随机 sibling staging directory；
7. 逐 object 下载、流式 hash、size check、fsync，恢复到 allowlisted relative path；
8. 设置 restrictive owner permission，不采用 manifest 中更宽的 group/world bits；
9. 对 control/scheduler/KV/D1 跑 migration checksum、`quick_check` 和 platform-owned metadata checks；
10. 交叉验证 control catalog 中每个 resource 的 restore file、artifact 引用和 snapshot manifest entry；
11. 验证 DO tree 只有 regular file/directory；不直接打开或改写 upstream SQLite；
12. 验证 control 中每个 active R2 bucket 的 provider marker 仍存在且 identity hash 匹配；不比较或
    回滚 bucket 内 object 内容；
13. fsync staging files/directories，原子 rename 为最终 data-dir；若平台不支持可靠 atomic rename则 fail closed；
14. 写 `data/operations/last-restore.json`，包含 snapshot/release/hash，不含 secret；
15. 由 operator 启动 daemon，先运行 `doctor --full`，再运行 explicit P0 smoke。

任何失败都保留原目标为空。失败 staging 默认保留一份 bounded diagnostic metadata；object bytes 可由显式
cleanup 删除，不在错误 path 上自动递归删除未知目录。

### 5.3 Post-restore smoke

restore Gate 不能只查 DB：

- `platformd doctor --full --json` 验证 runtime pin、S3 canary、SQLite/schema、master key 和 workerd start/stop；
- 启动 source release 的 daemon 和 stock workerd；
- 读取 snapshot 前写入的 KV/D1/DO sentinel；R2 只断言当前 provider marker/object 可读，不断言 object
  内容回到 snapshot 时间点；
- 验证 DO alarm authority/projection 能继续或被 token-exact repair；
- basic WebSocket 可以重新连接；不承诺恢复 snapshot 前的物理 socket；
- deploy B/rollback A authority、resource binding 和 route 仍一致；
- 新 mutation 成功后再次 restart 并读取；
- smoke 使用 reserved test account/resource，清理时只删除自己创建的精确 ID。

### 5.4 灾备能力边界

| 故障 | P1 snapshot 能否恢复 |
| --- | --- |
| 本地主机/data-dir 丢失 | 能；使用 fresh host + 同一 master key/S3 |
| control/KV/D1/DO 本地文件损坏 | 能；恢复整个 platform snapshot |
| platformd/workerd binary 丢失 | 能；重新安装 exact source release package |
| S3 credential 轮换 | 能；operator 提供对同一 authority 有权限的新 credential |
| master key 丢失 | 不能 |
| R2/bundle 所在 S3 bucket 丢失 | 不能；P1 没有复制这部分 authority |
| 想迁移到新 S3 provider | 不能；另做 export/migration 方案 |
| snapshot 之后的本地 KV/D1/DO/control 写入 | 不能；RPO 是 snapshot commit 时间 |
| snapshot 之后的 R2 put/delete | 不回滚；restore 看到 external S3 的当前状态 |
| 未断开的 WebSocket session | 不能；client 必须 reconnect |

### 5.5 P1.3 Exit Gate

- 在新的 temporary root、fresh config path、fresh process 中完成 restore；
- 原主机 data-dir 在 restore 测试期间不可见，防止误用原文件；
- recovery master key 通过 data-dir 外文件或 env 提供，目标目录在 install 前仍为空；
- 组合 P0 fixture 的所有 sentinel、binding、deployment 和 alarm 状态保持；
- wrong key、wrong S3 prefix、wrong release、missing object、hash mismatch、path traversal 全部在 install 前失败；
- restore 每个 crash point 都只留下空 target 或完整原子安装 target；
- restore 后 `doctor --full`、P0 aggregate smoke、restart mutation Gate 通过；
- 记录实际 snapshot bytes、restore duration、RPO timestamp 和启动后 ready duration。

## 6. P1.4：Forward-only Upgrade 与 Rollback Rehearsal

P1 upgrade 覆盖 platformd schema、每资源 schema、facade capability、snapshot format、S3 object format 和
workerd pin。它不是只执行 `ALTER TABLE`。

### 6.1 Release compatibility metadata

每个 release package 增加 machine-readable metadata：

```text
release.json
├── release identity
├── supported upgrade-from release range
├── restore-compatible release range
├── target schema tuple
├── required migration IDs/checksums
├── readable immutable S3 object format versions
├── workerd localDisk compatibility Gate result ID
└── required capability/conformance result ID
```

package 构建继续使用 P0.1 的 pinned official workerd archive 和 hash verification。`release.json` 必须由
production registries 生成并进入 package checksum，不能由 operator 手工编辑。

### 6.2 Offline upgrade workflow

```bash
# old release; daemon stopped
old/platformd --config /absolute/platform.toml \
  backup create --name before-upgrade

# new release; still offline
new/platformd --config /absolute/platform.toml \
  upgrade check --from-snapshot <snapshot-id> --json
new/platformd --config /absolute/platform.toml \
  upgrade apply --from-snapshot <snapshot-id> --json

# only after apply succeeds
new/platformd --config /absolute/platform.toml doctor --full --json
new/platformd --config /absolute/platform.toml run
```

`platformd run` 在发现需要 migration、schema mixed state、未知 future schema 或 release identity 不匹配时
拒绝启动，不能静默执行不可逆升级。

### 6.3 Multi-database migration

升级时 daemon 已停止，resource catalog 不再变化。`upgrade apply` 按以下顺序迭代：

1. 验证 pre-upgrade snapshot committed 且通过 manifest inspect；
2. control DB preflight，但暂不让新 binary 对外服务；
3. scheduler DB；
4. KV DB，按 resource ID canonical order；
5. D1 DB，按 resource ID canonical order；
6. project-owned metadata/receipt format；
7. immutable S3 format 只写新 version，不原地改写旧 object；
8. DO `localDisk` 只由通过 Gate 的 pinned workerd 读取，不由 platformd migration 打开；
9. 全量 schema scan，只有全部到 target tuple 才写 success receipt。

每个 SQLite migration 在自己的 transaction 中执行并记录既有 checksum。跨 DB crash 可能留下 mixed schema，
但 daemon 保持 offline；重跑 `upgrade apply` 从实际 schema 幂等继续。migration 必须支持“已经完成”与“未开始”，
不能依赖一次全局 operation row 才判断状态。

### 6.4 Rollback 规则

| 时点 | 允许的 rollback |
| --- | --- |
| `upgrade check` 前/后，未写任何 DB | 直接继续运行 old binary |
| migration 已开始或完成 | **只允许**恢复 pre-upgrade snapshot，再运行 old binary |
| new binary 已服务并产生新写入 | 恢复 pre-upgrade snapshot 会丢失升级后的写入，必须显式确认 RPO |
| 只有 workerd 启动失败、schema 未迁移 | 可回到 old package；仍需检查 receipt/schema tuple |

不提供 down migration，也不让 old binary“试着读”future schema。rollback runbook 必须把数据回退和 binary
回退视为同一个步骤。

### 6.5 workerd upgrade Gate

更换 workerd pin 是协调升级，不是普通依赖 bump：

- 更新 `runtime/workerd.lock.json` 并重新 package 官方 archive；
- 重跑 G0 全套以及 P0.2 至 P0.8 stock-workerd Gate；
- 重跑 P1.0 conformance；
- 用旧 workerd 写入 DO SQLite/KV/alarm/WebSocket fixture，clean stop 后由新 workerd 读取和继续写；
- 旧 localDisk 的 opaque snapshot 经 restore 后必须由新 workerd 成功激活；
- loader abort allowlist 只能维持 G0 已记录的精确条件，不可扩大；
- native facet/hibernation 行为改变只能进入新 capability version；
- No-Go 时保持旧 pin，不通过 platformd shim 猜测 upstream internal migration。

### 6.6 Crash matrix

至少在以下边界 SIGKILL offline command：

- 每个 DB migration transaction 前、commit 后；
- control 完成、scheduler 未开始；
- 第 N 个 KV/D1 完成；
- S3 new-format object 已写、control reference 未 commit；
- success receipt temp file 写入、rename 前后；
- doctor 验证前；
- new daemon 第一次 ready 前。

每次重启后只允许：继续 upgrade，或恢复 pre-upgrade snapshot。不能进入 mixed-schema serving。

### 6.7 P1.4 Exit Gate

- 支持的 N -> N+1 release fixture 可从 snapshot、check、apply、doctor、serve 完整跑通；
- 每个 migration checksum、schema tuple 和 resource file count 都被 release metadata 验证；
- crash matrix 重跑 apply 幂等，无重复/漏 migration；
- future schema、篡改 migration、unsupported upgrade-from 全部 fail closed；
- migration 后 old binary 拒绝运行；恢复 snapshot 后 old binary 可正常运行；
- workerd pin 升级重跑 G0/P0/P1.0 并通过旧 DO localDisk compatibility Gate；
- upgrade 后组合 P0 fixture 和 restart Gate 通过。

## 7. P1.5：Security Fuzzing 与恶意 Worker Fixtures

P1.5 不追求“随机 fuzz 所有 Rust 函数”。优先覆盖 tenant-controlled bytes 穿过 trust boundary 后会影响
authority、文件路径、SQLite、S3 key、workerd config 或内部 dispatch 的解析器和状态机。

### 7.1 Fuzz targets

| Target | 主要不变量 |
| --- | --- |
| canonical bundle/descriptor parser | size/depth bounded、无 path/ID confusion、结果确定 |
| binding descriptor/config JSON | unknown field policy一致、无 prototype/number confusion |
| request metadata/header bridge | internal header 不能伪造、hop-by-hop header 正确剥离 |
| resource/deployment/cursor ID codec | canonical round trip、跨 account 不碰撞、错误不泄露存在性 |
| facade RPC frame/structured value | depth/bytes bounded、cycle/error/stream cancel 有界 |
| KV cursor and metadata | MAC/scope/expiry/limit 正确，任意 bytes 不 panic |
| D1 SQL authorizer and result encoder | 禁止 pragma/attach/extension/hidden table，result 不失真 |
| R2/S3 object key builder | 不能 prefix escape、unicode/percent/slash 不混淆 |
| snapshot manifest/path parser | MAC/size/path/type 全部先验证，不能 archive traversal |
| migration/release metadata parser | checksum/schema/future version fail closed |
| scheduler/DO internal envelope | token/generation/type 不能由 tenant payload 伪造 |

短 deterministic property tests 进入普通 Rust suite。长 fuzz target 放在独立 `fuzz/`，固定 toolchain、seed、
corpus 和最大内存；release rehearsal 在本地运行固定时长。任何 crash/hang/oom input 先缩减，再进入
`tests/regressions/`，不能只留在某台机器的 fuzz corpus。

### 7.2 恶意 Worker fixture

stock-workerd fixture 至少覆盖：

- global scope、constructor 和 class field 中修改 facade/prototype；
- getter、Proxy trap、symbol、`__proto__`、`constructor.prototype`；
- 极深、极大、循环 structured value 和异常 `toJSON`；
- never-ending stream、cancel 时继续 enqueue、错误 content-length、slowloris；
- subrequest redirect、DNS rebinding、loopback/link-local/private address 和 credential URL；
- tenant 自建同名 internal header、binding、service name、alarm/DO dispatch route；
- SQL comment/pragma/attach/temp trigger/virtual table/extension 和 reserved table name；
- KV/R2/D1/DO resource ID 跨 account 替换、stale generation、deleted resource reuse；
- WebSocket frame flood、oversize message、close race、alarm + socket并发；
- log/error/stack 中尝试打印 secret、credential、internal URL 和 host filesystem path。

fixture 必须走 production loader/facade，不能直接调用 Rust backend 绕过 workerd boundary。

### 7.3 Isolation matrix

至少两个 account、每类两个 resource、两个 deployment generation：

```text
account A / deployment A1,A2 / resource A
account B / deployment B1,B2 / resource B
```

对每个 API 交换一个 scope component，断言：

- 返回统一 not-found/forbidden surface，不形成 resource existence oracle；
- backend 没有打开另一资源 DB/S3 key/DO object；
- metrics/log label 不出现 tenant ID 的无界基数；
- stale A1 token 不能在 A2 或 rollback A1 的新 generation commit；
- account A 的 delete/restore/GC 不影响 B。

### 7.4 Snapshot 与运维安全

- manifest/HMAC key 使用 domain separation；
- snapshot/list/inspect 输出不显示 S3 credential、master-key fingerprint 全值或 tenant secret；
- support bundle 做 allowlist serialization，不对任意 config/debug struct 直接 `Serialize`；
- restore 所有 path 在打开文件前做 lexical + filesystem containment；
- 不跟随 symlink，不恢复 device/socket/setuid bit；
- error chain 在 operator log 可关联 request/operation ID，但 public response 不含内部路径/SQL/S3 error；
- fuzz/crash harness 的 test-only key、endpoint 和 marker 不进入 release package。

### 7.5 P1.5 Exit Gate

- 所有 listed authority parser/state machine 都有 property/fuzz target owner；
- 固定 release fuzz budget 内无 crash、hang、unbounded allocation 或 invariant violation；
- 所有历史 corpus regression 在普通 test suite 可复现；
- 恶意 Worker fixture 不能获取 raw SQLite path、S3 credential、generic internal Fetcher 或其他 account data；
- header/ID/token/generation 交叉 matrix 全部 fail closed；
- snapshot/restore path、MAC 和 limit corpus 通过；
- secret canary 扫描 stdout/stderr/log/metrics/doctor/support bundle 为零命中；
- production release artifact 不包含 fault injection route、fuzz corpus secret 或 debug bypass。

## 8. P1.6：Soak、Load 与 Crash-point Matrix

P1.6 的目标不是发布一个“每秒请求数”营销数字，而是得到参考硬件、参考 config 下的可重复 capacity
envelope，并证明达到饱和时平台通过 backpressure 降级，而不是内存、文件描述符、WAL 或 scheduler backlog
无限增长。

### 8.1 本地 runner

```text
scripts/test-p1.sh                 # deterministic quick aggregate
scripts/test-p1-crash.sh           # bounded crash matrix
scripts/test-p1-upgrade.sh         # N -> N+1 + restore rollback
scripts/soak-p1.sh --duration 1h   # developer rehearsal
scripts/soak-p1.sh --duration 24h  # release Gate
scripts/load-p1.sh --profile mixed
```

这些脚本只在本地运行，不引入 CI、Codecov、上传或远程 threshold gate。默认使用 fresh temp data-dir、
MockS3/temporary platform storage 和 production stock workerd；manual-dev S3 是单独的 provider rehearsal，
不能替代 deterministic suite。

### 8.2 Workload profiles

| Profile | 组合 |
| --- | --- |
| HTTP mixed | fetch/RPC、small/stream response、outbound fetch、deploy A/B |
| storage mixed | KV get/put/list、R2 put/get/range、D1 prepared/batch、DO KV/SQL |
| DO realtime | 多 object、basic WebSocket、alarm overwrite/delete/retry |
| lifecycle | create/bind/deploy/promote/rollback/delete/backup/restore |
| snapshot | offline create/inspect/fresh restore、S3 slow/error/short read |
| upgrade | old release write、snapshot、N+1 migrate、new write、snapshot rollback |
| pressure | disk soft/hard、FD/process/concurrency/queue/staging saturation |

每个 profile 都使用可复现 seed，记录 request mix、object/resource count、payload distribution、duration 和 host
fingerprint。结果比较使用 envelope，而不是一次峰值。

### 8.3 Crash / fault matrix

| Fault | 注入点 | 必查恢复不变量 |
| --- | --- | --- |
| SIGKILL platformd | request、deploy、resource delete、shutdown | committed authority 不丢，workerd/orphan/socket 可回收 |
| SIGKILL workerd | cold start、stream、DO fetch、WebSocket、alarm | supervisor bounded restart，stale dispatch 不 commit |
| SQLite busy/I/O/full | control/KV/D1/scheduler transaction | fail closed、无 cross-resource corruption、emergency cleanup 可用 |
| S3 timeout/5xx/short read | artifact、R2、snapshot object/manifest | 不产生假 commit，retry/backoff 有界 |
| disk soft/hard/read-only | staging、WAL、snapshot、restore | admission 生效，delete/doctor 仍可用或明确失败 |
| clock forward/backward | TTL、alarm、lease、snapshot retention | monotonic/floor 规则生效，不重复旧 token commit |
| client abort | upload/download/RPC/WebSocket | stream cancel，reservation/permit/file 释放 |
| migration crash | 每 DB/receipt 边界 | offline mixed schema 可续跑，不能 serve |

fault injection 位于 test harness 的 wrapper/mock/process controller。production binary 不提供“第 N 次写入
crash”的 operator API。

### 8.4 Restart invariants

每个 fault 后自动运行同一份 invariant checker：

1. 所有 SQLite migration checksum、`quick_check` 和 foreign-key invariant；
2. control authority 引用的 resource file/artifact 存在；
3. 不存在另一 account 的资源打开或错误绑定；
4. deployment active/pending/failed generation 与 runtime projection 一致；
5. scheduler claim/token/lease 可完成、过期恢复或被安全丢弃；
6. DO object generation、alarm authority/projection fence 一致；
7. 没有无 owner 的 workerd、Unix socket、lock、temporary file 或 reservation；
8. health 最终 ready，或以稳定 degraded/unready reason 结束；
9. 内存、FD、thread、SQLite connection、WAL 和 queue depth 回到 bounded steady state；
10. operator/public output 没有 secret canary。

### 8.5 Capacity envelope 与 release budget

首次实现不在设计文档写死通用 QPS。`load-p1.sh` 在指定参考硬件和默认 config 上输出：

- steady/saturation request rate 与 p50/p95/p99；
- workerd cold/warm activation latency；
- KV/D1/DO transaction latency；
- R2 first-byte/stream throughput；
- scheduler due lag/retry/recovery time；
- RSS、CPU、FD、SQLite connections、WAL bytes、staging bytes；
- platform/workerd crash 到 ready 的 recovery time；
- snapshot/restore duration 与 bytes；
- admission reject rate 和 queue wait。

P1 release budget 由第一次稳定 baseline 冻结到 machine-readable result；后续 release 若超出允许回归幅度必须
解释或修复。不同硬件结果不横向承诺 Cloudflare SLA。

### 8.6 Soak 分层

- 10 分钟：每次实现迭代的泄漏/重启 smoke；
- 1 小时：P1 aggregate 本地 Gate；
- 24 小时：release candidate Gate；
- restart/fault 不是 24 小时结束后再做，而是在 workload 中按 deterministic schedule 注入；
- 每个阶段都保存 bounded JSON summary 和最后 N 条 redacted event，避免无限日志本身耗尽磁盘。

### 8.7 P1.6 Exit Gate

- quick、crash、upgrade、1h soak 脚本在 fresh process 连续三轮通过；
- release candidate 24h mixed soak 无 invariant violation、unbounded growth 或 secret leak；
- platformd/workerd/S3/SQLite/disk/clock/client-abort fault matrix 全部收敛到可服务或明确不可服务状态；
- saturation 通过 bounded wait/reject 降级，不出现 OOM、FD exhaustion 或 WAL 无限增长；
- fresh-host restore 和 upgrade rollback 被包含在组合 workload，而非独立 happy path；
- capacity result 记录 release identity、host/config fingerprint 和 seed，可重复比较。

## 9. P1.7：Production Ops Contract 与发布门槛

P0 已有 health、metrics、doctor、scheduler inspect/pause/resume/repair 和 supervisor 状态。P1.7 扩展这些
既有接口，不引入第二套 agent、Redis 或外部 control plane。

### 9.1 Health

| 端点/状态 | P1 语义 |
| --- | --- |
| liveness | platformd event loop 可响应；不代表 storage/runtime ready |
| readiness | control、runtime、required S3、disk admission、schema tuple 可服务 |
| degraded | 可服务但 snapshot 过旧、scheduler repair/backlog、S3/R2 部分能力异常 |
| draining | terminal shutdown，只停止新工作；不能复用为可逆 snapshot maintenance |

readiness reason 使用稳定低基数 enum。tenant/resource/snapshot ID 只进入 bounded operator detail，不进入
metric label。

### 9.2 Metrics 增量

至少补充：

- admission accepted/rejected，按 product/reason；
- disk free/reserved/staging/emergency headroom；
- account/resource count 与 quota reject；
- snapshot last success age、bytes、duration、inspect failure；
- restore receipt age、last verified smoke result、duration；
- schema tuple、migration required/failed resource count；
- conformance/release identity info metric；
- workerd crash/restart/cold activation；
- SQLite busy/WAL/check failure；
- S3 operation error/latency，按 operation class，不按 bucket/key；
- scheduler backlog/due lag/expired lease/repair；
- active WebSocket、close reason class，不按 object ID。

snapshot/restore 在 daemon offline 时产生的 receipt 由下次 daemon startup 读取并暴露。receipt 不是 authority，
损坏只导致 metric/doctor warning，不阻止产品数据启动。

### 9.3 Doctor 分层

```bash
platformd --config /absolute/platform.toml doctor --json
platformd --config /absolute/platform.toml doctor --full --json
```

Basic 保持只读、快速：

- config/path/permission；
- release/workerd lock/assets digest；
- data-dir owner/schema/migration required；
- control/scheduler/资源文件存在性与 lightweight check；
- disk thresholds/reservation/staging age；
- master-key fingerprint 与 ciphertext decrypt canary；
- last snapshot/restore/upgrade receipt。

Full 是 operator 显式授权的 bounded side effect：

- S3 system/R2 prefix canary create/read/delete；
- temporary workerd compile/start/ready/stop；
- immutable artifact sample/head；
- snapshot manifest list + latest manifest MAC/object sample verification；
- scheduler repair dry-run；
- optional reserved smoke resource，不触碰 tenant resource。

所有 JSON 都有 `schema_version`、check ID、status、stable error code、redacted detail 和 remediation key。

### 9.4 Support bundle

新增显式本地命令：

```bash
platformd --config /absolute/platform.toml support-bundle \
  --output /absolute/open-compute-support.tar
```

内容使用 allowlist：release identity、redacted config policy、doctor JSON、metric snapshot、migration schema tuple、
最近 bounded operator events、snapshot/restore receipt 和文件/权限清单。默认不包含 SQLite DB、DO localDisk、
Worker bundle、request/response body、R2 object、master key、credential、tenant secret 或完整 tenant identifier。

生成前后运行 secret canary scanner；输出路径必须绝对、目标不存在、不跟随 symlink。support bundle 只是本地
文件，不自动上传。

### 9.5 Runbook 集合

P1 Exit Gate 前必须完成：

```text
docs/runbooks/
├── install-and-first-start.md
├── backup-and-retention.md
├── fresh-host-restore.md
├── upgrade-and-rollback.md
├── disk-pressure.md
├── sqlite-corruption.md
├── s3-outage.md
├── workerd-crash-loop.md
├── master-key-loss-and-recovery.md
├── scheduler-recovery.md
└── collect-support-bundle.md
```

每份 runbook 都必须写清：触发信号、影响面、只读诊断、允许的 mutation、预期输出、停止条件、回滚和验证。
命令使用绝对 config/path，不使用 `$HOME`、`~`、未解析 env var、glob 或 broad recursive delete。

### 9.6 One-click self-deploy contract

P1 发布物仍只有一个 service unit：

```text
release/
├── bin/platformd
├── bin/workerd
├── runtime/workerd.lock.json
├── runtime/assets/...
├── share/default-config.toml
├── share/release.json
├── licenses/...
└── docs/runbooks/...
```

支持：

- 一个 Docker/Compose service，挂载一个 local data-dir、一个 config、一个 master-key secret，连接外部 S3；
- 容器外使用 systemd、launchd 或前台运行；
- install 后 `config check -> doctor --full -> run`；
- upgrade orchestration 执行 `stop -> snapshot -> upgrade check/apply -> doctor -> start -> smoke`；
- restore orchestration 执行 `install exact release -> inject key/config -> restore -> doctor -> start -> smoke`。

不增加 Nginx/API gateway、Redis、Postgres、Kafka 或 sidecar。TLS termination 可以由 operator 已有 reverse proxy
完成，但平台安全边界不能信任未配置的 forwarded headers。

### 9.7 P1.7 Exit Gate

- health/readiness/degraded/draining 语义和稳定 reason code 有黑盒测试；
- metrics 低基数、bounded、无 tenant secret/ID explosion；
- doctor basic 无副作用，doctor full 的每个 side effect 精确创建/删除；
- support bundle allowlist 和 secret canary Gate 通过；
- 所有 runbook 在 fresh temp host/container 至少演练一次；
- Docker 与容器外 service 都能执行 first start、snapshot、restore、upgrade、rollback；
- 停机/升级/恢复失败时输出下一步可执行 remediation，不要求人工编辑 SQLite/S3 object；
- P1 aggregate 本地 Gate 和 release checklist 可由单条脚本编排，但每个子 Gate 可独立运行。

## 10. P1.8：Advanced WebSocket Hibernation 条件性增强

Cloudflare Durable Objects 的 hibernation API 允许 runtime 在保留客户端 socket 的同时回收 object，之后在
message/close/error 时重新构造 object；应用需要用 attachment 恢复 per-socket metadata。当前平台的 public
socket 经 platformd bridge 再进入动态 facet，不能假定普通 native DO 的 hibernation 行为会自然成立。

因此 P1.8 先做 Hard Gate。P1.0 至 P1.7 全部通过即可宣布核心 P1 完成；P1.8 可以得到 `Go`、
`Conditional Go` 或 `No-Go`，但不能用自制 socket replay 延迟 P2。

### 10.1 Native facet Hard Gate

在 pinned stock workerd、production config、production public socket bridge 下验证：

| Gate | 断言 |
| --- | --- |
| WH-01 | dynamic facet class 可调用 `ctx.acceptWebSocket()` |
| WH-02 | `ctx.getWebSockets()` 返回当前 object 的 accepted sockets |
| WH-03 | tags 可写入、过滤和在 constructor rerun 后读取 |
| WH-04 | `serializeAttachment()` / `deserializeAttachment()` 跨 eviction 保持 |
| WH-05 | `webSocketMessage/Close/Error` dispatch 到正确 class/object/generation |
| WH-06 | eviction 后 constructor 重跑，但 socket 不经过新的 HTTP upgrade |
| WH-07 | alarm、RPC、fetch 与 socket event 保持 `blockConcurrencyWhile`/object ordering |
| WH-08 | deployment promotion/rollback 执行 P0.7 restart policy并以 1012 关闭旧 socket |
| WH-09 | platformd/workerd clean shutdown 不泄漏 FD/permit；client 收到 close 或断线 |
| WH-10 | platform/workerd process crash 后不声称恢复物理 socket，client reconnect 可重建 session |
| WH-11 | message/send/close 与 eviction race 无丢失 commit、double callback 或 runtime crash |
| WH-12 | 连接数、tag 数/长度、attachment bytes、message bytes 使用本地可配置 hard cap |

本地 limit 可以低于 Cloudflare 当前 limit，但必须由 capability output 暴露；不能把 Cloudflare plan limit
硬编码成单机默认值。

### 10.2 Go 路径

只有 WH-01 至 WH-12 在 exact workerd pin 上通过，才增加 facade：

- `ctx.acceptWebSocket(ws, tags?)`；
- `ctx.getWebSockets(tag?)`；
- `ws.serializeAttachment(value)`；
- `ws.deserializeAttachment()`；
- `webSocketMessage`、`webSocketClose`、`webSocketError` handler；
- native 支持且通过 Gate 时再考虑 auto-response API。

实现必须使用 native hibernatable socket/facet primitives。platformd 只负责 public transport、bounded
backpressure、lifecycle 和 close policy，不把 frame/session 存入 SQLite，不在 object eviction 后 replay client
socket。

### 10.3 Conditional Go / No-Go

以下任何一项不稳定就维持 P0 basic WebSocket：

- facet eviction 时 send/close/in-flight event 行为不可 fence；
- public bridge 无法把 socket ownership 可靠交给 native facet；
- constructor rerun、attachment 或 tags 在 pinned build 不可靠；
- deployment restart 时旧 socket 无法 bounded close；
- 需要解释 workerd internal storage 或 fork upstream；
- 只能通过 platformd 保存 frame/session 并伪装 hibernation。

capability output 明确标记 `basic_websocket=supported`、`hibernatable_websocket=unsupported` 或
`conditional`。WDL/Miniflare 的实现可以帮助构造 fixture，但不能替代 production path Gate。

### 10.4 P1.8 Exit Gate

- 保存 WH-01 至 WH-12 的 exact workerd release、config、fixture 和 verdict；
- Go 时 API/error/limit 进入 P1.0 conformance 与 24h DO realtime soak；
- eviction、alarm、promotion、shutdown、process crash 和 client reconnect matrix 通过；
- No-Go 时没有半暴露 method、flag 或 facade，basic WebSocket regression 全部通过；
- 不论 verdict，P2 scheduler 只能依赖 P1.0 至 P1.7，不能依赖 hibernation。

## 11. 代码组织建议

尽量复用现有 crate，不建立 `backup-service` 或 `quota-service` 微服务：

```text
crates/core/src/
├── capability.rs
├── admission.rs
├── snapshot_manifest.rs
└── release_identity.rs

crates/storage/src/
├── platform_snapshot.rs
├── platform_restore.rs
├── disk_admission.rs
└── upgrade.rs

crates/runtime/src/
├── capability.rs
└── hibernation_gate_support.rs      # only after Hard Gate

crates/service/src/
├── capabilities.rs
├── backup_cli.rs
├── upgrade_cli.rs
└── support_bundle.rs

crates/service/tests/
├── p1_conformance.rs
├── p1_snapshot_restore.rs
├── p1_upgrade.rs
├── p1_security.rs
├── p1_reliability.rs
└── fixtures/...

scripts/
├── test-p1.sh
├── test-p1-conformance.sh
├── test-p1-crash.sh
├── test-p1-upgrade.sh
├── soak-p1.sh
└── load-p1.sh
```

模块边界：

- `core` 只放稳定 value types、validation 和 format contract；
- `storage` 拥有 data-dir、SQLite backup/restore、path containment、snapshot S3 object orchestration；
- `runtime` 只声明与 workerd pin、facade 和 native behavior 相关能力；
- `service` 组装 CLI、doctor、metrics、operator output 和 process tests；
- full snapshot 不复用任意 tenant R2 binding；只使用 P0.1 platform artifact/S3 authority；
- hibernation Gate support 不进入 production facade，直到 Gate 为 Go。

## 12. 实施工作包

### WP1：Capability freeze

- 实现 release identity/capability registry 与 JSON schema；
- 补 P0 API matrix、deviation IDs 和 conformance fixtures；
- 添加 exact compatibility date/flags regression。

### WP2：Admission 与 offline ownership

- 收口每个 storage-growing entrypoint；
- 增加 reservation、staging reconciliation、resource count limits；
- 复用 data-dir lock 给 backup/restore/upgrade。

### WP3：Snapshot format/create

- canonical manifest、hash/MAC、S3 layout；
- SQLite backup、DO opaque walk、artifact reference verification；
- list/inspect/delete/retention 与 crash matrix。

### WP4：Fresh restore

- empty-target staging restore、path/mode/integrity validation；
- exact source release verification；
- fresh-host aggregate smoke 与 failure matrix。

### WP5：Upgrade

- release metadata、check/apply/run refusal；
- multi-DB resumable forward migration；
- snapshot rollback 和 workerd coordinated upgrade Gate。

### WP6：Security hardening

- property/fuzz target、regression corpus；
- malicious Worker、isolation、secret canary；
- snapshot/support-bundle parser and path audit。

### WP7：Reliability

- local quick/crash/upgrade/soak/load runners；
- deterministic fault matrix、invariant checker；
- reference capacity result 和 24h release run。

### WP8：Ops release

- health/metrics/doctor 增量；
- support bundle 和 runbooks；
- Docker/container-outside install/backup/restore/upgrade rehearsal。

### WP9：Hibernation conditional Gate

- stock-workerd facet fixture；
- Go 才实现 facade 和 production path；
- Conditional/No-Go 时固化 capability deviation 和 basic WebSocket regression。

## 13. P1 总 Exit Gate

P1.0 至 P1.7 同时满足以下条件，才允许进入 P2：

1. `PlatformCapabilitiesV1`、release identity、API conformance 和 deviation matrix 已冻结；
2. 所有 P0 storage-growing path 受 quota/admission/disk reserve 约束；
3. platform snapshot 在 manifest/object/MAC/crash matrix 下只产生完整 commit 或不可见 incomplete，并
   明确只恢复本地 authority、不是 R2 point-in-time backup；
4. fresh-host restore 使用同一 master key/S3 和 exact source release 恢复组合 P0 fixture；
5. N -> N+1 upgrade、crash resume、snapshot rollback、old-binary refusal 全部通过；
6. security fuzz/property、恶意 Worker、跨 account/resource/deployment 隔离和 secret canary 通过；
7. 24 小时 mixed soak、load saturation 和完整 crash/fault matrix 无 invariant violation 或 unbounded growth；
8. health、metrics、doctor、support bundle、runbook 与 one-click deploy/upgrade/restore rehearsal 完成；
9. `cargo fmt --all -- --check`、workspace Clippy/tests/no-default-features/MSRV/dependency-boundary 和既有
   local coverage Gate 通过；
10. G0、P0.2 至 P0.8 stock-workerd Gate 和 P0 aggregate Gate 在最终 release identity 上回归通过；
11. `git diff --check` 通过，release artifact 不包含 test-only fault/fuzz bypass；
12. P1 results 文档记录 exact commit、workerd pin、config/host fingerprint、命令、duration 和 verdict。

P1.8 单独记录 verdict：

- `Go`：hibernatable WebSocket 进入 capability manifest 和 P1 release；
- `Conditional Go`：只有精确列出的非安全条件，默认 capability 仍关闭；
- `No-Go`：继续提供 P0 basic WebSocket，不阻塞 P2。

## 14. 官方与实现参考

- Cloudflare Workers runtime APIs：<https://developers.cloudflare.com/workers/runtime-apis/>
- Cloudflare Workers Web standards：<https://developers.cloudflare.com/workers/runtime-apis/web-standards/>
- Cloudflare compatibility dates：<https://developers.cloudflare.com/workers/configuration/compatibility-dates/>
- Cloudflare compatibility flags：<https://developers.cloudflare.com/workers/configuration/compatibility-flags/>
- Cloudflare Durable Objects state API：<https://developers.cloudflare.com/durable-objects/api/state/>
- Cloudflare Durable Objects WebSocket best practices：
  <https://developers.cloudflare.com/durable-objects/best-practices/websockets/>
- Cloudflare Durable Objects limits：<https://developers.cloudflare.com/durable-objects/platform/limits/>
- SQLite Online Backup API：<https://www.sqlite.org/backup.html>
- SQLite WAL：<https://www.sqlite.org/wal.html>
- SQLite corruption / unsafe file-copy guidance：<https://www.sqlite.org/howtocorrupt.html>
- 本地 WDL 兼容性说明：`references/wdl/docs/compatibility.zh.md`
- 本地 workerd actor/WebSocket 实现：`references/workerd/src/workerd/api/actor-state.c++`、
  `references/workerd/src/workerd/api/hibernatable-web-socket.c++`、
  `references/workerd/src/workerd/api/web-socket.c++`
