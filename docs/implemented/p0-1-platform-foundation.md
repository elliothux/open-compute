# P0.1：Platform Foundation 详细设计

> 状态：已实现；按既有 [P0.1 回归记录](./p0-5-r2.md)归档，本次未重跑验收。
>
> 前置 Gate：G0 已于 2026-08-23 得到 **Conditional Go**。完整证据见
> [G0 results](./g0-results.md)，测试入口和 fixture 见 [`../poc/`](../../poc/README.md)。
>
> 本文只定义 P0.1。Worker 管理、部署、路由和请求执行由
> [P0.2：Workers Runtime](./p0-2-workers-runtime.md) 实现。

## 1. 交付目标

P0.1 把 G0 的一次性测试 harness 收敛为所有后续产品能力共用的生产宿主层。完成后，
一个全新的数据目录应当可以由一个 `platformd` 进程完成初始化，并稳定监督一个固定版本的
upstream `workerd` 子进程：

```text
operator / container / systemd / launchd
                    │
                    ▼
┌──────────────────────────────────────────────────────────┐
│ platformd                                                │
│                                                          │
│ config   data-dir   control.sqlite   S3/artifact cache   │
│ health   metrics    doctor           workerd supervisor  │
└───────────────────────────┬──────────────────────────────┘
                            │ loopback internal HTTP
                            ▼
┌──────────────────────────────────────────────────────────┐
│ pinned, unmodified upstream workerd                      │
│ static runtime host + workerLoader                       │
└──────────────────────────────────────────────────────────┘
```

P0.1 只交付“平台可以安全启动、持久化、访问 S3、监督 workerd”的能力，不对外承诺 Worker
deployment API。其直接下游是 P0.2，之后 KV、R2、D1、DO、Queue 和 Workflow 都复用这一层。

### 1.1 完成定义

P0.1 必须同时满足：

- 发布包内含经过校验的固定版本 `workerd`，启动时不从公网下载；
- 空数据目录可以首次启动，重复启动不会重复执行 migration 或覆盖已有 identity；
- 同一数据目录同一时间只能由一个 `platformd` 实例持有；
- `control.sqlite` 有 forward-only、带 checksum 的 migration 框架；
- master key 有明确的生成、加载、权限和不一致处理规则；
- S3-compatible provider 通过实际读写删除 canary 完成 preflight；
- immutable artifact store 与本地 content-addressed cache 可用；
- workerd ready、异常退出、优雅停止、强制停止和退避重启均有确定行为；
- `/health/live`、`/health/ready`、状态诊断和基础 metrics 可解释当前状态；
- 日志、错误和进程参数不泄露 S3 credential、master key 或内部认证信息；
- Linux/macOS 的前台运行通过；容器、systemd、launchd 使用相同二进制和数据目录契约。

### 1.2 非目标

P0.1 不实现：

- Worker create/deploy/promote/rollback；
- tenant bundle 解析和 `workerLoader` 动态加载；
- tenant ingress、域名和路由管理；
- KV、D1、R2、Durable Object、Queue、Workflow binding；
- 多进程共享写、多节点 failover 或远程 control plane；
- 自动升级 workerd、自动降级 schema 或在线切换两个 workerd 版本；
- Kubernetes operator、独立网关、Redis、Postgres 或消息中间件。

## 2. G0 证据与 P0.1 决策

G0 使用真实的 `workerd v1.20260826.1`，连续三轮新进程运行 hard matrix。Bootstrap、binding、
Durable Object 和 recovery gate 全部通过；Loader 只有已接受的 `D-abort` 限制。P0.1 必须继承
已经验证的路径，不把测试 fixture 当成生产实现。

| G0 证据 | P0.1 结论 |
| --- | --- |
| release URL、archive SHA-256、binary SHA-256 和 `--version` 均被验证 | 发布物必须由 lock 文件驱动，任何不一致在 spawn 前失败 |
| binary Cap'n Proto config 可编译并通过 stdin 启动 | 保留固定 config template + binary config，不把 tenant 输入拼进 Cap'n Proto |
| control fd 先报告 socket `listen`，随后 HTTP health 成功 | Supervisor readiness 必须同时依赖 control-fd ready 和 runtime probe |
| port collision、无效 config、不可写目录均 fail closed | 启动失败不能留下“看起来 ready”的 public listener 或孤儿子进程 |
| SIGTERM、SIGKILL、restart 和 harness exit 均无 child leak | `platformd` 必须拥有 process group、PID 和完整 reaping 责任 |
| workerd restart 后 dynamic loader 会 cold load | Supervisor 不承担 tenant loader cache 的持久化；cache 只是运行时优化 |
| `D-abort`：client disconnect 不会可靠 abort loaded Worker 的 signal | 不能把客户端断连当作停止 tenant 计算的 correctness primitive |
| `localDisk` 依赖 experimental/version-bound 行为 | workerd 与 config/数据格式升级必须作为同一个前向升级 Gate |

当前 G0 pin 是：

```text
release              v1.20260826.1
workerd version      1.20260826.1
runtime date         2026-08-26
compatibility date   2026-08-22
process flags        --experimental
compatibility flags  nodejs_compat, rpc, enable_ctx_exports, experimental
```

这些值是首个 P0 实现的验证基线，不是“永远默认给 tenant 的兼容配置”。P0.2 会分别管理 host
Worker 的 flags 和 tenant deployment 的 compatibility date/flags。

## 3. 进程与信任边界

### 3.1 进程职责

`platformd` 是唯一的产品进程入口，负责：

- 解析和验证 operator config；
- 独占数据目录；
- migration 和 control state；
- master key 生命周期；
- S3 client、内部 artifact store 和本地 cache；
- public/control listener；
- workerd config 生成、启动、监督、停止与状态采集；
- health、metrics、doctor 和 tenant-safe error mapping。

`workerd` 只负责：

- 执行固定的 platform runtime Worker；
- 在 P0.2 之后通过 native `workerLoader` 执行 tenant Worker；
- 提供 workerd runtime API、isolate/resource limits 和 native DO/facet 能力；
- 访问 config 明确授予的 service、network、disk 和 loader capability。

tenant code 永远不能直接获得：

- `control.sqlite` 或任意资源 SQLite 文件路径；
- 数据目录的通用 disk service；
- S3 credential、master key 或内部 control token；
- 任意内部 Fetcher、loader callback 或 host admin endpoint；
- `platformd` 的 process/control capability。

### 3.2 内部 transport

P0.1 选择 G0 已验证的 **loopback HTTP** 作为跨平台基线：

- `workerd` 只绑定 `127.0.0.1:<ephemeral-port>`；
- port 由 OS 分配并由 control-fd 返回，不能写死；
- 外部只监听 `platformd` 的 public/control address；
- `platformd` 生成每次子进程启动独立的 256-bit internal token；
- token 通过生成的 binary config 进入固定 host Worker，不出现在 argv、环境 dump 或日志；
- 所有内部请求由 `platformd` 覆盖认证 header，tenant 提供的同名 header 在 public ingress
  边界先删除；
- host Worker 对 token、method、path、content type 和 request size 全部 fail closed。

Unix domain socket 可以在 P1 作为 hardening 增强，但不能成为 P0.1 Linux/macOS 行为不一致的
来源。即使后续使用 Unix socket，capability token 和请求边界仍然保留，不能只依赖“端口没有
公开”。

### 3.3 public/control listener

P0.1 允许同一个 listener 承载 health 和 control API，但必须支持以下部署配置：

```text
server.public_bind       default 127.0.0.1:8787
server.admin_bind        optional, default same listener
server.trusted_proxies   empty by default
```

如果 admin listener 暴露到非 loopback，启动必须要求显式 admin auth 配置。P0.1 只有 health、
metrics 和 doctor 所需的最小端点；P0.2 才增加 Worker management API。

## 4. 发布物与 workerd 供应链

### 4.1 发布目录

推荐发布布局：

```text
open-compute/
├── bin/
│   ├── platformd
│   └── workerd
├── runtime/
│   ├── workerd.lock.json
│   ├── config.capnp
│   └── system-workers/
├── licenses/
└── share/
    └── default-config.toml
```

容器镜像只是同一目录的包装；非容器安装不走另一套 runtime 逻辑。

### 4.2 `workerd.lock.json`

lock 文件是 release artifact 的一部分，至少包含：

```json
{
  "schemaVersion": 1,
  "release": "v1.20260826.1",
  "expectedVersionOutput": "workerd 2026-08-26",
  "hostCompatibilityDate": "2026-08-22",
  "processFlags": ["--experimental"],
  "hostCompatibilityFlags": [
    "nodejs_compat",
    "rpc",
    "enable_ctx_exports",
    "experimental"
  ],
  "targets": {
    "darwin-arm64": {
      "archiveSha256": "<hex>",
      "binarySha256": "<hex>"
    }
  }
}
```

完整值由 [`../poc/workerd.lock`](../../poc/workerd.lock) 迁入正式格式。构建/release job 下载官方
artifact、验证 archive 和 binary hash 后再打包。生产启动 **不得** 自动联网下载或静默选择
另一个 workerd。

每次启动在 spawn 前执行：

1. 校验当前 OS/arch 在 lock 中；
2. 校验 binary 是普通文件且不经过不受信任 symlink；
3. 校验 binary SHA-256；
4. 执行一次有 deadline 的 `workerd --version` 并精确比对；
5. 校验 config/template digest 和 platform release metadata；
6. 任一失败即把 runtime 状态标为 `invalid`，不启动 child。

hash 可以在同一 inode、size、mtime 未变化时缓存于当前进程内，但每次新 `platformd` 进程至少
完整计算一次，不能仅信任扩展属性或文件名。

### 4.3 升级规则

workerd、固定 host Worker、Cap'n Proto config 和需要持久化的数据格式构成一个 runtime
compatibility unit：

- 不自动跟随 `latest`；
- 升级只允许到 release manifest 明确列出的版本；
- 先在备份/复制的数据目录运行 G0 和 P0 regression matrix；
- 遇到未知的更新 schema，旧版本 `platformd` 必须拒绝启动；
- `localDisk`/DO 数据只做 forward-only 升级，不承诺 downgrade；
- 发布说明必须列出 workerd pin、host compatibility flags 和数据 migration；
- 升级失败不能用旧 binary 打开已经执行新 migration 的数据目录。

## 5. 配置契约

### 5.1 来源和优先级

配置只允许来自：

1. 显式 `--config <absolute-path>`；
2. 文件内声明的 `env:` 引用；
3. 少量有文档的 `OPEN_COMPUTE_*` bootstrap 变量。

不隐式读取 `$HOME`、当前目录 `.env` 或 tenant bundle。优先级为 CLI bootstrap path > config
file > documented defaults。包含 secret 的值必须支持 `file:` 或环境变量引用，解析后的 config
不得被完整打印。

### 5.2 最小配置面

```toml
[server]
public_bind = "127.0.0.1:8787"

[storage]
data_dir = "/var/lib/open-compute"
master_key_file = "/var/lib/open-compute/keys/master.key"

[s3]
endpoint = "https://s3.example.com"
region = "auto"
bucket = "open-compute"
force_path_style = true
access_key_id_env = "S3_ACCESS_KEY_ID"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"

[runtime]
startup_timeout_ms = 20000
shutdown_grace_ms = 10000

[cache]
max_bytes = 10737418240
low_watermark_ratio = 0.80
```

数值只是配置示例，不是 Cloudflare plan limit。实现必须为所有 timeout、body size、cache size、
restart rate 和并发量提供 operator 可见的本平台默认值，不复制 Cloudflare 套餐数字。

配置校验在获取数据目录锁之前完成静态检查，在启动 listener 之前完成 dependency check。未知字段
默认报错，避免拼写错误被静默忽略。`platformd config check` 只做只读/无 child 校验；会写 S3
canary 或迁移 DB 的检查只能由 `doctor --full` 或正式 startup 显式执行。

## 6. 数据目录

### 6.1 物理布局

```text
<data-dir>/
├── platform.lock
├── control.sqlite
├── control.sqlite-wal
├── control.sqlite-shm
├── keys/
│   └── master.key
├── runtime/
│   ├── config.<sha256>.bin
│   └── previous/
├── cache/
│   └── artifacts/sha256/<first-2>/<remaining-hex>
├── do/                       # P0.7 起由 workerd localDisk 管理
├── kv/                       # P0.4 起
├── d1/                       # P0.6 起
├── backup-staging/
└── diagnostics/
    └── failed-starts/
```

`scheduler.sqlite` 在 P0.8 首次引入，不由 P0.1 提前创建空壳。

### 6.2 目录规则

- `data_dir` 必须是显式绝对路径；
- 首次创建目录使用 `0700`，普通文件默认 `0600`；
- 启动拒绝 group/world-writable 的 data root、key file 和 SQLite authority file；
- 所有子路径先 canonicalize 并验证仍位于 data root，不能接受 `..`、不受信任 symlink 或
  device/FIFO/socket 作为普通文件；
- 临时文件必须在最终文件同一 filesystem 创建，之后 `fsync(file) -> rename -> fsync(dir)`；
- cache 可以删除重建，`control.sqlite`、key 和未来资源文件不能由 GC 删除；
- diagnostics 只保留有限数量和字节，写入前做 secret redaction；
- 启动记录可用空间；达到 soft threshold 时进入 degraded，达到 hard threshold 时拒绝新的
  mutation，但 health、读取和受控 shutdown 仍可用。

### 6.3 单实例锁

`platform.lock` 使用 OS advisory exclusive lock，锁持有期等于 `platformd` 生命周期：

- lock 文件可包含 instance ID、PID、start time 和 release version 供诊断，但文件内容不是锁；
- 获取失败返回明确的 `DATA_DIR_IN_USE`，不能尝试删除 lock 文件；
- PID 不存在也不能据此跳过 OS lock；
- child `workerd` 不单独取得此锁，由 `platformd` 保证同一数据目录只有一个 owner；
- NFS/网络文件系统不在 P0 支持范围，doctor 必须提示无法保证锁和 SQLite durability。

## 7. `control.sqlite` bootstrap

### 7.1 连接策略

P0.1 创建独立的 control DB connection manager。推荐初始化：

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = <configured-ms>;
PRAGMA trusted_schema = OFF;
```

每次借出 connection 都校验 `foreign_keys=ON`；migration 使用一个独占 writer，不与 runtime
mutation 并行。`control.sqlite` 只存平台 metadata，tenant SQL 永远不在这个 connection 上执行。

### 7.2 基础 schema

P0.1 只落地 migration 和 platform identity 所需的最小表。P0.2 通过后续 migration 增加 Worker
表。

```sql
CREATE TABLE schema_migrations (
  version          INTEGER PRIMARY KEY,
  name             TEXT NOT NULL,
  checksum_sha256  BLOB NOT NULL CHECK(length(checksum_sha256) = 32),
  applied_at_ms    INTEGER NOT NULL,
  app_version      TEXT NOT NULL
) STRICT;

CREATE TABLE platform_meta (
  key              TEXT PRIMARY KEY,
  value            BLOB NOT NULL,
  updated_at_ms    INTEGER NOT NULL
) STRICT;

CREATE TABLE accounts (
  id               TEXT PRIMARY KEY,
  name             TEXT NOT NULL,
  created_at_ms    INTEGER NOT NULL,
  deleted_at_ms    INTEGER
) STRICT;

CREATE UNIQUE INDEX accounts_live_name
ON accounts(name) WHERE deleted_at_ms IS NULL;
```

`platform_meta` 至少保存：

```text
platform_id             首次初始化生成，之后不变
created_at_ms
last_started_version
master_key_id           key 的非秘密 fingerprint，不保存 key
artifact_schema_version
```

单机默认账户在首次初始化事务中创建，但内部仍保留 `account_id`，避免后续所有表都需要破坏性
改造。resource/worker ID 统一使用 canonical lowercase UUIDv7；名称只是 display/lookup key，
不能用于物理路径或 capability identity。

### 7.3 migration 协议

每个 migration 包含固定 version、名称、SQL/代码和 build-time SHA-256：

1. 读取 `user_version` 和 `schema_migrations`；
2. 已应用 migration 的 checksum 必须精确匹配当前 binary；
3. 数据库版本高于 binary 支持版本时以 `SCHEMA_TOO_NEW` 拒绝启动；
4. 在 `BEGIN EXCLUSIVE` 中执行一个 migration；
5. 做 migration-specific invariant check；
6. 插入 migration row 并更新 `user_version`；
7. commit 后再开始下一个 migration；
8. 任一步失败 rollback，保留诊断，但不继续启动 workerd。

禁止启动时自动 down migration。涉及不可逆数据变化的 release 必须先要求可验证 backup。测试中
必须支持故障注入到“执行前、DDL 中、写 migration row 前、commit 后”四个边界。

## 8. Master key

### 8.1 生成与加载

P0 支持两种模式：

- operator 提供：环境变量或权限受限的绝对 key file；
- single-click 首次启动：CSPRNG 生成 32 bytes，原子写入
  `<data-dir>/keys/master.key`，权限 `0600`。

key 文件使用带版本的文本封装，例如 `ocmk1:<base64url>`。如果同时配置环境变量和 key file，
两者解码后的 key 必须相同，否则以 `MASTER_KEY_MISMATCH` 失败。已有数据库记录 `master_key_id`
后，新的 key fingerprint 不匹配必须拒绝启动，不能把旧 ciphertext 当作损坏数据覆盖。

### 8.2 加密契约

P0.2 的 Worker secrets 使用成熟库提供的 AEAD：

- 每条 ciphertext 使用新的随机 nonce；
- associated data 至少包含 schema version、account ID、worker ID、deployment ID 和 secret name；
- 数据库保存 `key_id + algorithm + nonce + ciphertext`；
- 明文只在创建 runtime env 的最短路径存在，禁止写日志、metrics、S3 manifest 或 diagnostics；
- 错误只报告 secret name/hash，不回显 secret value；
- 内存 zeroization 是 best effort，不能代替进程和权限隔离。

在线 key rotation 不属于 P0，但 schema 必须保留 `key_id`，不能假定平台永远只有一个 key。

## 9. S3 preflight

### 9.1 配置

需要显式支持：

- endpoint、region、bucket；
- virtual-host 或 path-style；
- TLS verification，P0 默认不能关闭；
- static credential env/file，或实现明确支持的 provider chain；
- connect/request timeout、有限 retry/backoff；
- 平台内部 prefix，默认 `system/`，必须与 tenant R2 prefix 隔离。

credential 只属于 `platformd`。它不能进入 workerd config、tenant env 或 argv。

### 9.2 实际能力检查

不能只用 `HeadBucket` 判断可用性，因为很多最小权限 policy 会禁止 bucket-level 操作。完整
preflight 在内部 prefix 创建唯一 canary：

```text
system/preflight/<platform-id>/<startup-id>/<random>
```

顺序为：

1. PUT 随机小 payload 和 checksum metadata；
2. HEAD 并验证 size/metadata；
3. GET 并验证 payload SHA-256；
4. DELETE；
5. 再次 HEAD 确认不可见，或接受 provider 文档化的 delete 状态；
6. 无论中间哪步失败都 best-effort DELETE，并记录不含 credential 的 reason code。

preflight key 不包含账户名、bucket logical name 或 secret。启动时可配置有限 retry，但认证、
签名、TLS 和 permission error 不做无限重试。

### 9.3 健康语义

- 首次启动未通过 preflight：不进入 ready；
- 运行中 S3 短暂失败：`live=200`，`ready=503` 且状态为 `degraded_s3`；
- 已在本地 cache 的 immutable artifact 仍可被读取；
- 不允许新的 artifact mutation，不能把未持久化数据报告为成功；
- health 状态不能直接驱动“重启 platformd”；orchestrator 只应以 liveness 决定 crash restart。

## 10. Immutable Artifact Store

### 10.1 边界

P0.1 提供通用的内部 `ArtifactStore`，P0.2 再定义 Worker bundle 的 canonical manifest。接口至少
包括：

```text
put_verified(stream, expected_sha256, expected_size) -> ArtifactRef
head(ArtifactRef) -> metadata
open(ArtifactRef) -> verified stream
delete_unreferenced(ArtifactRef)
```

物理 key：

```text
system/artifacts/v1/sha256/<first-2-hex>/<remaining-hex>
```

`ArtifactRef` 只包含 version、SHA-256 和 size，不包含 credential、endpoint 或可由 tenant
选择的完整 S3 key。

### 10.2 写入协议

1. 在接收流时计算 SHA-256 和 size，超过 configured limit 立即中断；
2. expected digest/size 不匹配则拒绝；
3. 以 digest 对应的 final content-addressed key 上传；
4. provider 支持条件写时使用 create-if-absent；不支持时允许同 digest 的幂等覆盖；
5. HEAD/GET 验证远端 size 和内容 digest；
6. 验证成功后才返回可写入 `control.sqlite` 的 ref；
7. 数据库提交前崩溃产生的 artifact 是 orphan，由 grace-period GC 清理；
8. 数据库 ref 已提交但上传未验证的状态不允许出现。

同一 digest 的并发 put 必须收敛到一个物理对象。删除只接受内部 referrer/GC 决策，tenant 删除
Worker 不能直接删除共享 artifact。

### 10.3 读取与完整性

每次从 S3 下载到 cache 都在流式写临时文件时计算 SHA-256，只有 digest/size 都匹配才原子
rename 到 cache final path。S3 返回内容与 key digest 不匹配时：

- 删除临时文件；
- 报告 `ARTIFACT_INTEGRITY_ERROR`；
- 不进入 loader；
- readiness 进入 degraded；
- 日志包含 digest、provider request ID 和 stage，不包含 signed URL/credential。

## 11. Local Artifact Cache

cache 是可删除优化，不是 authority：

```text
cache/artifacts/sha256/<first-2>/<remaining-hex>
```

行为要求：

- cache hit 仍校验 path、普通文件、size；进程首次使用该 entry 时完整校验 SHA-256；
- miss 通过 per-digest singleflight 下载，避免 cold-start stampede；
- 写入 `.partial.<startup-id>.<random>`，验证后同文件系统原子 rename；
- 启动清理超过 grace period 的 partial file；
- 发现损坏 entry 后 quarantine/delete，再从 S3 拉取一次；再次失败不无限循环；
- in-use artifact 有内存 pin，evictor 不能删除正在读取的文件；
- LRU metadata 可在内存重建，P0.1 不为 cache 再引入 authority SQLite；
- 达到 high watermark 后清理到 low watermark；只删除无 pin 的最旧 entry；
- S3 不可用时允许使用已经通过完整校验的 cache entry。

P0.2 必须通过 `ArtifactStore` 使用 cache，不能自己拼 S3 key 或读取任意本地 path。

## 12. Static workerd config

### 12.1 固定拓扑

P0.1 的 config 只包含平台拥有的 static service：

```text
platform ingress host
loader host
runtime source bridge       # P0.2 接到 platformd internal endpoint
outbound gateway            # 只导出 fetch，P0.2 用作 globalOutbound
public network              # allow = ["public"]，只绑定给 outbound gateway
workerLoader
DO supervisor/localDisk     # P0.7 启用，不提前对 tenant 暴露
```

tenant module、binding name、route 或 deployment metadata 不进入 Cap'n Proto config；这些从 P0.2
开始通过 `workerLoader` callback 动态构造 `WorkerCode`。

workerd 的 network service 使用 `allow = ["public"]`，由地址解析后的 CIDR policy 阻止 private/
local 目标；它只绑定给平台拥有的 outbound gateway。Dynamic Worker 的 `globalOutbound` 指向
gateway stub，gateway 只实现 HTTP(S) `fetch`，不导出 raw `connect`。内部 platformd service 是
单独的显式 capability，不属于 tenant `globalOutbound`。

### 12.2 编译与缓存

发布包保留可审计的 `config.capnp` 和 system Worker 源码。启动时：

1. 对 config、system Worker、lock 和 platform release metadata 生成一个 input digest；
2. 若 `<data>/runtime/config.<digest>.bin` 已存在，验证 digest/普通文件/权限；
3. 否则用 pinned workerd 执行有 deadline 的 `workerd compile ... --config-only`；
4. 输出写临时文件，编译成功后原子 rename；
5. `workerd serve --binary -` 从 stdin 接收已验证的 binary config；
6. 编译 stderr 经 redaction 后进入 bounded diagnostics。

任意 tenant 输入都不得成为 compile 文件路径或 Cap'n Proto expression。旧 binary config 只用于
诊断和显式 release rollback，不能在当前 input digest 不匹配时自动启用。

## 13. Supervisor

### 13.1 状态机

```text
                 config/dependency fixed
                          ┌──────────────┐
                          ▼              │
STOPPED ── start ──> STARTING ── ready ──> RUNNING
                         │                   │
                         │ error/exit        │ exit/unhealthy
                         ▼                   ▼
                    BACKING_OFF <─────── FAILED
                         │
                         └── retry budget permits ──> STARTING

RUNNING ── shutdown ──> DRAINING ── term ──> STOPPING ──> STOPPED
                                              └─ deadline -> kill
```

状态与 `last_transition_at`、`attempt`、`last_exit`、`next_retry_at` 暴露给 health/status，但不能
把 raw stderr 直接返回给外部。

### 13.2 Spawn 协议

沿用 G0 已验证的关键点：

- argv 由固定字段构造，不经 shell；
- 使用独立 process group/session；
- stdin 只传 binary config；
- 单独 control fd 接收结构化 `listen` event；
- stdout/stderr 使用 bounded line reader，处理超长行和非 UTF-8；
- 记录实际 PID、process group、binary/config digest 和 startup ID；
- readiness 需要期望 socket 的 control-fd `listen` 与内部 HTTP probe 都成功；
- child 在 ready 前退出时保留 exit code、signal 和 bounded redacted tail；
- startup timeout 到期后先 TERM、再按 deadline KILL，并完整 wait/reap。

`platformd` 必须在 child spawn 后、等待 ready 前持久化无密钥 child lease。lease 至少绑定 PID、
PGID、OS start identity、已验证 binary digest；macOS 还绑定实际 staging executable 的 canonical
路径。下次启动只有在 live PID、process-group leader、start identity 与实时 executable hash 全部匹配
时才能 KILL/reap orphan，任何身份缺失或不一致都 fail closed。

macOS 从已打开 vnode 物化 `oc-exec-*/workerd` 前，必须先在 `<data>/runtime/child.staging`
原子记录 staging journal。若 SIGKILL 落在复制中，下一次启动只清理 journal 指向、无人使用、且哈希
尚不完整的精确文件和空目录；已完整复制但尚无 child lease 的文件不得自动删除。正常退出、verified
orphan recovery 与失败清理都必须同时清除 staging executable、空目录和 journal。

### 13.3 Restart policy

- 只有 unexpected child exit/明确 runtime failure 触发自动 restart；
- config invalid、binary hash mismatch、schema/key mismatch、权限错误不重试；
- transient exit 使用 exponential backoff + jitter；
- 在 rolling window 内超过 restart budget 后进入 `failed_runtime`，等待 operator intervention 或
  配置的较长 retry；
- 每次 restart 生成新的 internal token、startup ID 和 ephemeral port；
- restart 期间 public listener 保持 live，但新的 Worker 请求由 P0.2 返回 `503 RUNTIME_UNAVAILABLE`；
- 非幂等请求不能由 supervisor 自动 replay；child 在 commit/response 边界退出时结果可能未知。

### 13.4 Shutdown

1. health 立即进入 draining/not-ready；
2. 停止接受新的 control mutation 和 tenant dispatch；
3. 等待 platformd 已接收请求到 configured drain deadline；
4. 向 workerd process group 发送 SIGTERM；
5. deadline 后 SIGKILL；
6. `wait` 回收 child；
7. checkpoint/关闭 SQLite connection；
8. 释放 data-dir lock 并退出。

P0.1 不以“child 收到 SIGTERM”推断其一定结束；必须总有 kill deadline。反过来，正常 stop 不应被
计入 crash restart budget。

### 13.5 client disconnect 限制

G0 已证明当前 pin 下，断开 loader-host 请求不保证 tenant `request.signal` 进入 aborted。正式实现：

- platformd 在 client disconnect 后停止读取/写入并关闭自己的 upstream；
- 不等待 tenant 自愿退出作为资源回收前提；
- tenant isolate 仍受 CPU、memory、subrequest 和平台 wall deadline 限制；
- 不因为 disconnect 自动 restart 整个 workerd；
- metrics 记录 `client_disconnected` 与 `runtime_completed_after_disconnect`；
- 只有未来新 pin 通过相同黑盒测试，才能改变这一已知限制。

## 14. Health、metrics 与 doctor

### 14.1 Health endpoints

| Endpoint | 成功条件 | 失败是否应触发进程重启 |
| --- | --- | --- |
| `GET /health/live` | `platformd` event loop 可响应，未进入 unrecoverable panic | 是，仅此端点用于 liveness |
| `GET /health/ready` | data lock、schema、key、control DB、S3 preflight、workerd 均可用且未 draining | 否，用于接流/部署判断 |
| `GET /health/status` | 始终返回经过鉴权/脱敏的 component 状态 | 否 |

`/health/live` 不同步调用 S3、SQLite integrity check 或 workerd；否则依赖抖动会制造重启风暴。
`/health/ready` 返回稳定 reason code，例如：

```text
STARTING
MIGRATION_FAILED
MASTER_KEY_MISMATCH
S3_UNAVAILABLE
RUNTIME_STARTING
RUNTIME_RESTART_BACKOFF
RUNTIME_INVALID
DRAINING
READY
```

### 14.2 Metrics

P0.1 至少暴露：

```text
platform_info{version,workerd_version}
platform_ready{component}
platform_start_total{result,stage}
workerd_process_up
workerd_restart_total{reason}
workerd_start_duration_seconds
sqlite_operation_duration_seconds{database="control",operation}
s3_request_total{operation,result}
s3_request_duration_seconds{operation}
artifact_cache_bytes
artifact_cache_entries
artifact_cache_hit_total
artifact_integrity_error_total
```

禁止把 account ID、worker ID、deployment ID、object key、URL 或 request ID 用作无界 metrics
label。高基数字段只进入 sampled structured logs/traces。

### 14.3 Doctor

提供两个模式：

```text
platformd doctor             # 只读，不改 DB/S3，不启动长期 child
platformd doctor --full      # 显式执行 canary、temporary workerd start/stop
```

检查项包括 binary/hash/version、config compile、data path/permission/lock、SQLite quick_check、
migration checksum、master-key fingerprint、磁盘空间、S3 canary、cache sample integrity、workerd
control-fd/health。输出同时支持 human text 和 stable JSON；JSON 值不得包含 secret。

## 15. 启动顺序

```text
1. parse CLI + config
2. validate static config and redactable representation
3. open/create data directory and acquire exclusive lock
4. validate permissions, filesystem and free space
5. load/generate master key
6. open control.sqlite and run migrations
7. initialize/read stable platform identity
8. verify packaged workerd + config inputs
9. initialize S3 client and run preflight
10. initialize artifact cache
11. compile/reuse verified binary workerd config
12. start health/control listener in STARTING state
13. spawn workerd and wait control-fd + runtime probe
14. transition READY; enable later P0 data-plane admission
```

失败时按逆序释放已取得资源。第 12 步之前的失败由 CLI/stderr 返回；第 12 步之后也必须反映在
health/status。任何失败路径都要回收 temporary child、partial file、SQLite connection 和 data
lock。

## 16. 错误与故障语义

| 故障 | 外部状态 | 自动动作 | 数据承诺 |
| --- | --- | --- | --- |
| workerd hash/version 不符 | not ready / `RUNTIME_INVALID` | 不 spawn、不重试 | 不修改 DB |
| config compile 失败 | not ready | 保存脱敏诊断，不启用旧 digest config | 不修改 authority |
| migration 中失败 | not ready / `MIGRATION_FAILED` | transaction rollback | 已提交旧 migration 保留 |
| schema 比 binary 新 | not ready / `SCHEMA_TOO_NEW` | 拒绝启动 | 不尝试 downgrade |
| master key 不匹配 | not ready | 拒绝启动 | 不覆盖 ciphertext/key file |
| S3 preflight 失败 | live, not ready | 有限 retry | 不报告 artifact mutation 成功 |
| cache entry 损坏 | degraded only if refetch fails | 删除/隔离并 refetch 一次 | authority 仍在 S3 |
| workerd ready 前退出 | live, not ready | backoff restart | control DB 不受影响 |
| workerd 处理中退出 | live, not ready | 不 replay，backoff restart | 请求结果可能未知 |
| platformd 被 SIGKILL | process down | 下次 WAL recovery + orphan cleanup | 只承诺已 commit/fsync 的状态 |
| 磁盘达到 hard limit | live, degraded | 拒绝 mutation，允许诊断/清理 | 不继续放大 WAL/cache |

## 17. 实现工作包

### P0.1.0：Rust service skeleton

- CLI、typed config、redaction；
- component registry 和 startup/shutdown coordinator；
- structured logging、request/startup ID；
- `/health/live`、`/health/status` skeleton；
- deterministic clock、fault-injection interface 只在 test build 启用。

### P0.1.1：Runtime supply chain

- 正式 `workerd.lock.json`；
- release fetch/verify/package job；
- startup hash/version/platform verification；
- static config input digest 和 binary config compiler/cache。

### P0.1.2：Data directory 与 control DB

- exclusive lock、permission/path/filesystem checks；
- atomic file helper；
- SQLite pool/PRAGMA；
- migration/checksum framework；
- `platform_meta`、default account 和 ID generator。

### P0.1.3：Master key

- env/file/first-run generate；
- fingerprint 与 DB identity 校验；
- AEAD facade 和 secret redaction test helper；
- mismatch/permission/corrupt key failure path。

### P0.1.4：S3、ArtifactStore 与 cache

- S3 config/client/preflight；
- content-addressed put/open/head/delete；
- streaming hash、partial/atomic rename；
- singleflight、LRU high/low watermark 和 orphan cleanup。

### P0.1.5：workerd supervisor

- spawn argv/stdin/control-fd/process group；
- readiness handshake；
- timeout、TERM/KILL、reaping；
- restart backoff/budget；
- platform drain 和 internal token lifecycle。

### P0.1.6：Observability 与 operations

- ready/status reason codes；
- bounded metrics；
- doctor/doctor-full；
- diagnostics retention 和 log redaction；
- systemd/launchd/container examples。

### P0.1.7：Integration Gate

- 使用真实 packaged workerd；
- Linux 与 macOS CI；
- fresh/restart/crash/corruption/S3 matrix；
- 连续三轮 fresh process；
- 确认没有 child、port、lock、partial file 泄漏。

工作包按编号顺序合入。P0.1.4 与 P0.1.5 可以在 P0.1.0–P0.1.3 稳定后并行开发，但最终 Gate
必须在同一个 `platformd` 进程内验证完整启动顺序。

## 18. 测试矩阵

### 18.1 继承 G0 bootstrap cases

以下 G0 行为必须进入正式 regression suite，而不是只保留 POC：

- lock/version/archive/binary checksum；
- mismatch 在 spawn 前失败；
- config compile success/invalid config non-zero；
- port collision fail closed；
- unwritable directory；
- control-fd ready 后 health 才成功；
- handler exception 不终止 runtime host；
- SIGTERM、SIGKILL、restart new PID；
- parent exit 回收 child；
- 测试结束无进程和端口泄漏。

### 18.2 P0.1 新增 cases

| 领域 | 必测场景 |
| --- | --- |
| First run | 空目录、重复启动、并发双启动、初始化中 crash、已有 partially-created key/DB |
| Path/permission | relative path、symlink escape、world-writable、FIFO/device、只读目录、网络 FS 告警 |
| Migration | 全新、重复、checksum mismatch、future schema、每个 crash point、rollback 后重启 |
| Key | auto-generate、operator file、env/file mismatch、corrupt base64、权限过宽、DB fingerprint mismatch |
| S3 | bad DNS/TLS/auth/region/bucket/policy、timeout、5xx、PUT 后 crash、DELETE 失败、canary cleanup |
| Artifact | digest/size mismatch、同 digest 并发、S3 内容损坏、orphan、shared ref、stream cancel |
| Cache | cold/hit、partial startup cleanup、corrupt cache/refetch、S3 down cached hit、LRU pin、disk full |
| Supervisor | ready 前 exit、ready 后 exit、rapid crash loop、control-fd 无事件、probe hang、TERM hang/KILL |
| Shutdown | active health request、spawn 中 shutdown、backoff 中 shutdown、SIGINT/SIGTERM、double signal |
| Redaction | credential/key/token 出现在 config/error/stderr/header 时均不可进入 log/status/metrics |

所有 crash case 使用外部 test supervisor 杀死真实 `platformd`/`workerd`，不能只 mock Rust method。
时间、retry 和 backoff 在测试中使用 deterministic clock；真实进程 deadline 另做少量 wall-clock
integration test。

## 19. P0.1 Exit Gate

P0.1 只有在以下 checklist 全绿后才允许 P0.2 依赖它：

- [ ] release bundle 不联网即可启动，workerd pin 与 G0 一致；
- [ ] clean data-dir 首启、正常重启和 crash recovery 通过；
- [ ] 双实例竞争同一 data-dir 时只有一个成功；
- [ ] migration checksum/future-schema/fault matrix 通过；
- [ ] master key 不会进入 DB plaintext、S3、argv、log 或 metrics；
- [ ] S3 canary 与 immutable ArtifactStore 通过兼容 provider matrix；
- [ ] cache corruption 和 S3 outage 有确定降级行为；
- [ ] workerd control-fd + HTTP readiness、restart backoff 和 shutdown reaping 通过；
- [ ] `/health/live` 与 `/health/ready` 不混淆 crash 和 dependency degradation；
- [ ] doctor 能在不泄露 secret 的情况下定位 binary/config/DB/S3/runtime 问题；
- [ ] Linux/macOS 连续三轮 fresh-process integration suite 无 leaked child/file/lock/port。

## 20. 提供给 P0.2 的稳定接口

P0.2 只能依赖以下明确接口，不能绕过到实现细节：

```text
ControlDb
  transaction(mode)
  migration(versioned migration)
  stable IDs / platform identity

SecretCrypto
  encrypt(context, plaintext)
  decrypt(context, ciphertext)

ArtifactStore
  put_verified / head / open / delete_unreferenced

ArtifactCache
  acquire_verified(ref) -> pinned readable artifact

RuntimeSupervisor
  state / ready endpoint / dispatch transport / restart generation

HealthRegistry + Metrics
  component state / bounded observation
```

下列内容不构成稳定接口：workerd ephemeral port、internal token、cache path、S3 physical key、binary
config path、SQLite connection、child PID 和 raw workerd error。P0.2 若需要这些信息，应扩展 typed
interface，而不是读取文件或解析日志。

## 21. 参考资料

- [G0 results](./g0-results.md)
- [G0 POC README](../../poc/README.md)
- [G0 workerd lock](../../poc/workerd.lock)
- [总体方案](../open-compute-workerd-platform.md)
- [workerd configuration schema](https://github.com/cloudflare/workerd/blob/main/src/workerd/server/workerd.capnp)
- [workerd repository](https://github.com/cloudflare/workerd)
- [Cloudflare Dynamic Workers API](https://developers.cloudflare.com/dynamic-workers/api-reference/)
- [Cloudflare Dynamic Workers egress control](https://developers.cloudflare.com/dynamic-workers/usage/egress-control/)
- [SQLite WAL](https://www.sqlite.org/wal.html)
- [SQLite atomic commit](https://www.sqlite.org/atomiccommit.html)
