# P8：Local / S3 对象后端设计

状态：2026-09-05 Implementation GO；Day 1 实现与本地验收完成。

本文把当前强制 S3 的对象存储路径改造成互斥的 Local / S3 后端。目标是让单机部署可以直接使用本地目录，
不再要求额外启动 `rclone serve s3`，同时保留真实 S3-compatible provider。本文只改变平台内部对象字节的持有方式；
SQLite、master key、workerd local disk、runtime cache 和普通临时文件仍由各自现有 authority 管理。

## 0. 完成结论与实际证据

P8 已进入唯一生产路径，不保留旧配置、旧 S3-only composition、rclone 开发 sidecar、双读写、自动迁移或
Local/S3 fallback。实际完成范围包括：

- `[data]` 与 tagged `[storage]` Local/S3 配置、统一的 config-relative 路径解析，以及 SQLite/marker/snapshot
  三方 object-authority 绑定；
- backend-neutral `ObjectBackend`，封闭的 secure Local 实现与 AWS SDK/SigV4 S3 adapter；
- artifact、R2、cache body、snapshot、KV/D1 backup、AI Search source 和相关 doctor/health/metrics/support
  bundle 全部使用同一个已选 backend；
- Local fd-relative no-follow 访问、权限与 hardlink/special-file 拒绝、原子 envelope、fsync、容量约束、bounded
  recovery、multipart intent/reconcile 及 SSE-C authenticated chunked AEAD；
- checked-in `scripts/config/dev.toml`、`dev-test.toml`、`dev.env`，以及不再启动 rclone/object-server 的开发脚本。

最终 Gate 的 `source_sha256` 为 `160d420c634f0bbd3df689f0ca50e391422e6a47ce547ce03c4c54b95895a625`，
conformance baseline `openComputeRevision` 为 `2c475e635a0f3d85a4f6f4038a24d9b73f807962f478cac6941ca7b78ec7c550`。
实际验收：

| 检查 | 结果 |
| --- | --- |
| `bun run build` | PASS；runtime、toolchain、dashboard、extension、examples、scripts 与 conformance TypeScript 均通过 |
| `cargo fmt --all --check` | PASS |
| canonical Clippy `--workspace --all-targets --all-features --keep-going -- -D warnings` | PASS |
| `cargo check --workspace --no-default-features` + `RUSTFLAGS='-D warnings'` | PASS |
| `cargo +1.98.0 check --workspace --all-targets` | PASS |
| `cargo metadata --no-deps --format-version 1` | PASS |
| `./test/check-boundaries.sh` | PASS |
| `./test/coverage.sh` | 49/49 targets、1,129/1,129 cases PASS；109,286/121,412 lines，**90.0125%**；673.89 秒；报告 `.temp/gate-run/20260905T044454-8e5a9cb6/report.json` |
| 最终 `./test/gate.py --workspace` | 单轮 49/49 targets、1,129/1,129 cases PASS；763.89 秒；报告 `.temp/gate-run/20260905T045653-4e056756/report.json` |

`cf-compatibility-check` 以 formal pin `workerd v1.20260830.1`、revision
`e9dda5963aba7ee4323960db795690ec78fec118`、effective compatibility date `2026-08-30` 和 Workers types
`5.20260830.1` 校验。对照官方
[R2 Worker API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/)、
[upload semantics](https://developers.cloudflare.com/r2/objects/upload-objects/)、
[consistency](https://developers.cloudflare.com/r2/reference/consistency/) 与
[durability](https://developers.cloudflare.com/r2/reference/durability/) 后，无剩余 in-scope Worker API finding：single/part/
multipart ETag、lowercase-hex `ssecKeyMd5`、storage class、conditions、checksum、SSE-C 与 multipart surface 均由
Local/S3 共用产品合同覆盖。`OC-R2-001` 只保留准确的部署拓扑差异：open-compute 是单机 Local 或单 endpoint
S3 authority，不宣称 Cloudflare 全球 placement、replication 或 durability；public S3 endpoint 不在支持范围。
本次没有执行 Cloudflare 账号部署，也没有修改任何已有 Cloudflare 服务。

实现文件长度的例外是有意且局部的：`local.rs` 将一个 versioned envelope 的 fd ownership、AEAD、multipart commit
和 recovery 状态机保持在同一 crate-private 安全边界，拆开会增加跨模块复开路径或绕过校验的入口；`client.rs` 保持
单一 S3 protocol adapter；`local_tests.rs` 保留同一持久化格式的攻击、fault 与 restart matrix。它们不建立公共通用
filesystem framework或扩展点，后续只有在不复制校验、fd 或 format authority 的前提下才按内部所有权拆分。

验收期间覆盖率构建耗尽磁盘后，按用户授权执行一次 `cargo clean`，仅清除了约 163.3 GiB 可重建的 `target/`
Rust build cache；未删除 `.data/`、失败证据或正式 pinned workerd 输入。未执行发行打包、发布或部署。

## 1. 结论

1. `ocd` 在一次启动中只选择一个对象后端：`local` 或 `s3`。配置通过带 `backend` 判别字段的枚举解析，
   从类型结构上拒绝两套字段同时出现，不提供“优先 local”“S3 失败回退 local”或双写模式。
2. `crates/artifacts` 提供一个后端中立的 `ObjectBackend` facade。内部只有两个封闭实现：
   `LocalObjectBackend` 直接执行安全文件操作，`S3ObjectBackend` 使用现有 AWS Rust SDK 发出 SigV4 请求。
3. Local 路径不启动 HTTP server，不实现 S3 API，也不嵌入、调用或监督 rclone。领域层调用的是 object
   operation，不是 S3 request builder。
4. 当前所有由 `S3ArtifactClient` 持有的对象统一迁移到该 facade：Worker/Assets immutable artifacts、Workers
   Cache body、KV/D1 backup、platform snapshot、AI Search source object，以及 tenant R2 object/multipart。
5. Local 后端必须维持仓库已有的 no-follow、path-containment、权限、原子发布、fsync、完整性和 crash recovery
   边界。普通 `root.join(key)` 加 `std::fs::File::open/create` 不合格。
6. S3 和 Local 是同一产品语义的两种持久化实现。选择后端不能改变 R2、backup、snapshot、artifact GC 或
   Worker deployment 的公开行为。
7. P8 不支持在线迁移、镜像、tiering、双写或运行中切换后端。已初始化平台更换 backend 或 authority 时
   fail closed；未来若需要迁移，另行设计显式、可校验的 export/import 工作流。

## 2. 对象存储所有权

Artifact、R2、snapshot、backup 与 AI Search domain store 统一调用 `ObjectBackend`；
它只负责后端操作、并发条件与错误分类，领域 key、metadata、完整性和生命周期仍由各 store 拥有。
Local 与 S3 是启动时互斥选择，不能在单个 account／bucket 中切换；SQLite 与 workerd storage 保留原有 ownership。

## 3. 配置合同

### 3.1 Local

以下示例假设配置文件位于 `/srv/open-compute/open-compute.toml`；相对路径以该文件所在目录为基准：

```toml
[data]
path = ".data/platform"
master_key_file = ".data/platform/keys/master.key"
sqlite_busy_timeout_ms = 5000
free_space_soft_bytes = 1073741824
free_space_hard_bytes = 268435456

[storage]
backend = "local"
path = ".data/platform/objects"
prefix = "system/"
r2_prefix = "tenant/r2/"
free_space_soft_bytes = 1073741824
free_space_hard_bytes = 268435456
```

### 3.2 S3

`[data]` 与 credential file 同样允许使用配置文件相对路径；S3 endpoint、bucket 和 object key prefix 不是主机文件路径，
不参与 path resolution：

```toml
[data]
path = ".data/platform"
master_key_file = ".data/platform/keys/master.key"
sqlite_busy_timeout_ms = 5000
free_space_soft_bytes = 1073741824
free_space_hard_bytes = 268435456

[storage]
backend = "s3"
endpoint = "https://s3.example.com"
region = "auto"
bucket = "open-compute"
force_path_style = true
verify_tls = true
access_key_id_env = "S3_ACCESS_KEY_ID"
secret_access_key_env = "S3_SECRET_ACCESS_KEY"
prefix = "system/"
r2_prefix = "tenant/r2/"
max_retries = 3
retry_backoff_ms = 200
connect_timeout_ms = 5000
request_timeout_ms = 30000
```

`[data]` 是本机平台状态根：SQLite、master key、D1、KV、Durable Objects、runtime、cache、staging 和
diagnostics 都在这棵受 `ocd` 排他拥有的目录树中。`[storage]` 只选择 object-byte authority，承载 immutable
artifacts、tenant R2、snapshot、backup、AI Search source 和 Workers Cache body。两者都是真实数据，区别是
“本机平台状态”与“可由 Local 或 S3 持有的对象正文”，而不是 authority 与非 authority 的区别。

wire 使用 `[data]` 与 `[storage]`，Rust 内部仍以 `object_storage` 明确后者的职责，避免与负责 data-dir/SQLite
的 `storage` crate 混淆。配置模型使用一个带内部 tag 的枚举，而不是两个 `Option`；以下省略其余字段，仅展示
wire shape：

```rust
#[serde(deny_unknown_fields)]
struct PlatformConfig {
    data: DataConfig,
    #[serde(rename = "storage")]
    object_storage: ObjectStorageConfig,
}

#[serde(deny_unknown_fields)]
struct DataConfig {
    path: PathBuf, // Resolved absolute path.
    master_key_file: PathBuf, // Resolved absolute path.
    // SQLite and local data-root policies.
}

#[serde(tag = "backend", rename_all = "snake_case", deny_unknown_fields)]
enum ObjectStorageConfig {
    Local {
        path: PathBuf, // Resolved absolute path.
        // Local-only fields.
    },
    S3 {
        endpoint: String,
        // S3-only fields.
    },
}
```

准确 wire shape 由 internally tagged enum 加 `deny_unknown_fields` 表达；每个 variant 都拒绝未知字段。
最终规则：

- `[data].path`、`[storage]` 和 `storage.backend` 必填，不从环境、已有目录或旧配置推断；
- `data.path`、`data.master_key_file`、Local `storage.path`、S3 credential file、`SecretReference.file` 及以后新增的
  host filesystem path 统一接受绝对或配置文件相对路径，并在配置 authority boundary 一次解析成绝对路径；
- `backend = "local"` 时，出现 `endpoint`、`bucket`、credential 或 retry 字段立即 `CONFIG_INVALID`；
- `backend = "s3"` 时，出现 `path` 或 local free-space 字段立即 `CONFIG_INVALID`；
- 旧 `[storage].data_dir`、顶层 `[s3]` 和 `[object_storage]` 都成为未知配置并直接失败，不保留 alias 或兼容解析；
- local 启动不读取、要求或探测任何 S3 credential 环境变量；
- S3 继续要求 HTTPS；只有现有明确允许的 loopback test/development endpoint 可以使用 HTTP，
  `verify_tls = false` 仍拒绝；
- `prefix` 与 `r2_prefix` 都必须是 canonical、非空、以 `/` 结尾且互不重叠；两种 backend 采用同一校验；
- `[data]` 与 Local `[storage]` 的 `free_space_hard_bytes <= free_space_soft_bytes`，两者非零；前者测量
  `data.path` 所在 filesystem，后者单独测量 `storage.path`，不能假设两棵 root 位于同一 filesystem。

`share/default-config.toml` 选择显式 `[data]` 加 local `[storage]` 作为单机默认示例；references 同时给出完整 S3
示例。代码中的 `PlatformConfig::default()` 不能偷偷选择生产 backend，测试应通过明确的 local/S3 fixture
constructor 创建配置。

### 3.3 路径解析合同

路径基准采用与 Node.js `package.json` 相同的“入口相对 CWD、内容相对配置文件”模型：

1. 进程进入 CLI 后立即捕获一次 startup CWD；后续 child、线程或库代码不得重新读取 CWD参与路径解释；
2. `--config` 接受绝对路径或相对路径；相对值只相对 startup CWD，不搜索 CWD、parent、`$HOME` 或默认目录；
3. 对 `--config` 执行 lexical resolve，消去 `.` 和 `..`，得到绝对候选；安全打开实际 parent directory，最终
   config leaf 使用 `openat` + `O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK`，并验证 regular file、size 和 UTF-8；
4. config leaf 是 symlink、FIFO、socket、device、目录或过大文件时失败；若参数中的 parent 路径包含 symlink，
   `config_base` 取实际打开文件所在 parent 的 canonical absolute path，而不是调用者写下的 alias；
5. TOML 中每个 host filesystem path：绝对值保持绝对语义；相对值按
   `resolve(config_base, configured_value)` 做 lexical normalization。原始相对值可以包含 `.` 和 `..`；解析结果可以位于
   config directory 之外，因为可信 operator 本来就可以写等价绝对路径；
6. `~`、`$HOME`、`${NAME}`、shell glob 和 URI 不展开，按普通路径字符处理；需要环境选择时只能使用字段明确声明的
   env reference，不能在 path string 中插值；
7. 所有 resolved path 必须是非空绝对路径且不再含 `.`/`..`，随后继续执行字段自己的 no-follow、owner/mode、regular
   file/directory、local-filesystem、root-overlap 和 containment 校验；路径解析不能替代这些安全检查；
8. `load_platform_config` 在 parse 后、static validation 前完成全部解析。`PlatformConfig`、`DataConfig` 和
   `ObjectStorageConfig` 不向下游暴露 unresolved path，service/storage/artifacts/runtime 也不得再次相对 CWD解析；
9. `config check`、run、doctor、backup、restore、support bundle 使用同一 resolver。运行中修改 CWD 或配置文件不改变
   已加载路径；重启后按新的 invocation/config location重新解析；
10. `config init` 输出到 stdout，无法知道最终配置文件会保存在哪里，因此把相对 `--data-dir` 按 startup CWD解析后输出
    绝对路径；手写或保存后的配置仍可改用 config-relative path。

例如：

```text
startup CWD: /srv/lynx
--config:    config/open-compute.toml
config_base: /srv/lynx/config
data.path:   ../.data/open-compute/platform
resolved:    /srv/lynx/.data/open-compute/platform
```

移动配置文件会有意改变其内部相对路径的目标，行为与移动 `package.json` 一致。systemd、launchd、container 和正式运维
文档仍优先使用绝对 `--config`，避免服务管理器 WorkingDirectory变化选择另一份配置；配置内部是否使用相对路径由
operator决定。

### 3.4 Local `storage.path` 与 `data.path` 的关系

以下规则针对已经解析的绝对路径。允许两种布局：

- 精确的保留子目录 `<data.path>/objects`；
- 与 `data.path` 完全不重叠的独立目录。

拒绝以下配置：

- `storage.path` 等于 `data.path`；
- `storage.path` 是 `data.path` 的祖先；
- `storage.path` 是 data root 内除保留 `objects/` 之外的任意后代；
- object root 与 master key、SQLite、runtime、cache、staging、DO storage 或 diagnostics 路径重叠；
- resolved root 为 `/`、symlink、非目录、group/world writable，或位于不满足现有本地锁/持久性前提的
  network filesystem。

若选择保留子目录，platform snapshot 的 include/exclude matrix 必须明确排除 `objects/`。对象引用由 snapshot
manifest 单独验证，不能把正在写入 snapshot 的 object authority 递归打包进自身。

## 4. `ObjectBackend` 边界

### 4.1 使用封闭 facade

P8 不为未知第三方 backend 建立插件系统。对消费者公开的是可 clone 的单一 facade，分派 enum 保持 crate-private：

```rust
#[derive(Clone)]
pub struct ObjectBackend {
    inner: Arc<ObjectBackendImpl>,
}

enum ObjectBackendImpl {
    Local(LocalObjectBackend),
    S3(S3ObjectBackend),
}
```

`ObjectBackend` 提供后端中立的 inherent async methods，并在内部 match 两个实现；上层只能读取低基数
`ObjectBackendKind` 用于 health/metrics，不能按 variant 分叉业务逻辑。这避免：

- 把所有上层 store 泛型化；
- 为 async trait object 新增无必要的 boxing/macro 依赖；
- 暴露 AWS SDK 类型；
- 为尚不存在的 backend 留扩展点。

测试 fake 只能位于 `cfg(test)` / `test-support`；生产 enum 只有 Local 和 S3。

### 4.2 统一值类型

backend API 使用以下概念，不接收 bucket、tenant、R2 logical key 或裸 filesystem path：

| 类型 | 合同 |
| --- | --- |
| `ObjectKey` | 已验证的 backend physical key；只能由 domain store 从固定 prefix 和结构化 ID 构造 |
| `ObjectSource` | bounded bytes，或已 no-follow 打开并验证的 regular-file fd + exact length；不传待重新打开的 path |
| `ObjectMetadata` | size、opaque ETag、last-modified、bounded user metadata、HTTP content fields、backend-neutral physical storage class |
| `PutMode` | `CreateOnly`、`Replace`、`IfMatch(ETag)` |
| `HeadOptions` | optional customer encryption key；用于验证 SSE-C object 而不读取 body |
| `GetOptions` | optional exact byte range、optional `IfMatch`、optional customer encryption key |
| `ObjectBody` | backend-neutral bounded async reader/stream；不暴露 AWS `ByteStream` |
| `ListPage` | canonical objects、truncation 和 backend-owned opaque cursor |
| `MultipartUploadId` | opaque、bounded、不可由 caller 解释的 upload identity |
| `BackendError` | `NotFound`、`PreconditionFailed`、`InvalidRange`、`Corrupt`、`Unavailable`、`Capacity` |

`ObjectKey` 是安全边界，不只是 `String` alias：

- 非空，不以 `/` 开头或结尾；
- 不允许空 segment、`.`、`..`、反斜杠、NUL、控制字符或平台分隔符别名；
- 固定最大总长度和 segment 长度；
- 只接受当前 host-generated ASCII physical layout；
- tenant R2 key 继续先 SHA-256 映射，不能直接成为 filesystem component 或 S3 key suffix。

### 4.3 操作集合

| 操作 | 必须提供的语义 |
| --- | --- |
| `probe` | 对选定 authority 执行 write/head/get/delete/absence canary，不返回 key/path |
| `put` | bounded streaming、metadata、customer encryption、create/replace/if-match 原子条件 |
| `head` | missing 与 corrupt 分离；返回 committed object 的完整 metadata |
| `get` | full/range streaming 和可选 ETag fence；body 与返回 metadata 来自同一个已打开的 committed object inode |
| `delete` | missing 也成功；不得删除 caller key 以外的对象 |
| `delete_many` | 逐对象确定结果；S3 batch 不可用时可在 adapter 内 bounded fallback |
| `list` | prefix、稳定 lexical order、limit、opaque continuation；只列 committed object |
| `create_multipart` | 持久化 upload intent 和 metadata，返回 opaque ID |
| `upload_part` | exact part number/length，原子替换同一 part，返回 deterministic ETag |
| `list_multipart` | 只返回 exact physical key 的未完成 upload IDs，供 restart reconciliation |
| `complete_multipart` | 验证有序 parts 后原子发布一个 object；成功后不暴露部分结果 |
| `abort_multipart` | 幂等清理 exact upload，不能影响 committed object |

不增加当前没有消费者的 copy、rename、presign、ACL、versioning 或 watch API。Domain store 负责把 backend error
映射为稳定的 artifact、snapshot、backup 和 R2 error，adapter 不直接生成 Cloudflare response。

## 5. S3 adapter

`S3ObjectBackend` 保留现有固定 AWS SDK 和 SigV4 实现，但把 provider 细节收口在 adapter 内：

- bucket、endpoint、region、path style、credentials、retry 和 timeout 只存在于 S3 config/adapter；
- `PutMode` 映射为 `If-None-Match: *` 或 `If-Match`；
- `ObjectMetadata` 映射 S3 metadata/content headers/ETag/last-modified；
- range、list pagination、batch delete 和 multipart 映射现有 S3 operations；
- provider status/SDK errors 只在 adapter 内归一化为 `BackendError`；
- SSE-C customer key 只在 request builder 内短暂映射为 header，不进入日志、错误或持久化状态；
- preflight 继续验证所依赖的 read-after-write、conditional put、metadata、range、list、delete 和 multipart 能力。

S3 object key layout原则上保持当前 canonical `system/` 与 `tenant/r2/` 结构，但 P8 不承诺读取历史开发 bucket。
配置 authority marker 和持久化 fingerprint 必须与当前 platform identity 一致；同 bucket 下不同实例必须使用不同、
不重叠的 prefixes。

## 6. Local 持久化格式

### 6.1 Root layout

Local backend 使用独立、带版本的内部格式：

```text
<storage.path>/
├── format.json
├── backend.lock
├── objects/
│   └── <ObjectKey segments>/object.ocobj
└── multipart/
    └── <upload-id>/manifest.json + parts/
```

`object.ocobj` 是单文件、带版本的内部 envelope：固定 magic/version 和 bounded header 后紧跟 payload。header 至少包含：

- format/schema version；
- ObjectKey SHA-256，用于防止路径与内容错配；
- plaintext size、ETag、last-modified；
- bounded canonical metadata/content fields；
- encryption mode、chunk size、nonce material 和 key verifier（如适用）；
- header checksum、payload checksum/length。

最终 `object.ocobj` 文件是对象可见性的唯一 commit point。PUT/multipart complete 先在最终 parent directory 中创建唯一
`.partial` envelope，写完并 `fsync` 后原子发布；partial 不属于可见 object。header 损坏、文件类型错误、声明长度与
`fstat` 不一致、payload 校验失败都属于 corruption，读取必须失败，不能当作 missing 或自动修复。

ObjectKey 每个 segment 映射为一级受控目录，末端固定使用 `object.ocobj`，因此 `a` 与 `a/b` 可以同时存在；不能用
追加 `.meta` 的方式制造 key collision。`.partial` 名只使用 host 生成、严格解析的 UUID/hex，不包含 caller 输入。
Local object tree 是平台内部格式，不承诺让 operator 直接编辑或把 object 映射成可读文件名；“direct filesystem”只表示
`ocd` 直接对这些文件执行 I/O，不经过 S3 HTTP server、FUSE 或 rclone。

### 6.2 Root 初始化与绑定

首次初始化遵循：

1. 从可信 parent 创建 mode `0700` root，拒绝 symlink 和已有不明非空目录；
2. no-follow 创建/打开 mode `0600` `backend.lock` 并取得排他锁；
3. 确认除 lock 外为空，再用原子写创建 mode `0600` `format.json`，记录 schema version、platform ID、canonical
   prefixes 和随机 root ID；
4. 逐个创建/验证固定子目录，所有目录 mode `0700`；
5. 计算 authority fingerprint 并与 SQLite platform authority 比较；不一致则在任何 object mutation 前失败。

以后启动必须先安全打开 root并取得 object root lock，再验证 marker和固定布局。锁顺序固定为 platform data-dir lock，
然后 object root lock；offline backup/restore/doctor 使用同一顺序。即使 object root位于
`<data.path>/objects`，也保留独立 marker和lock，避免以后独立挂载或误配时改变所有权语义。

### 6.3 安全文件访问

所有 Local 操作从启动时 no-follow 打开的 root directory fd 出发：

- 每级目录使用 `openat` + `O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC`；
- 文件使用 `O_NOFOLLOW | O_CLOEXEC`，创建使用 `O_CREAT | O_EXCL` 和 mode `0600`；
- 打开后用 `fstat` 验证 regular file、owner/mode、长度和允许的 link count；
- symlink、FIFO、socket、device、异常 hard link、group/world writable entry 一律 fail closed；
- 删除和 rename 也相对于已验证 parent dirfd，不能重新拼 absolute path；
- 不依赖“先 canonicalize 再普通 open”，避免 check/open TOCTOU；
- `list` 不跟随目录项，不跨 filesystem/root，不递归未知布局；
- public errors、metrics、health 和 tenant response 不包含 local root 或 object physical path。

Local 实现可复用 `crates/artifacts` 现有 cache 的 rustix no-follow/openat 经验，并在 crate 内收敛成一个
`secure_fs` 私有模块；不让 `artifacts` 依赖 sibling `storage`，也不复制一套宽泛公共 filesystem framework。

不直接采用 `object_store::local::LocalFileSystem`：其公开合同允许跟随 symlink，包含解析到配置 root 外的
symlink，这不满足上述边界。S3 adapter 继续使用 AWS SDK，P8 不因接口统一而引入另一个通用 object-store crate。

### 6.4 原子 PUT / overwrite / delete

Local PUT 顺序固定为：

1. 在最终 object parent directory 内以 `O_EXCL | O_NOFOLLOW` 创建唯一 `.partial` envelope；
2. 从 bytes 或已验证 fd 流式写入，执行 exact-length、checksum 和 capacity accounting；
3. 完成 header/payload checksum 与长度，`fsync` 整个 partial envelope；
4. 在 per-key 有界/striped lock 下 no-follow 打开当前 object header 并执行 `CreateOnly` / `IfMatch` 条件；
5. 以 no-replace 或 replace 原子 rename 发布为 `object.ocobj`，再 `fsync` parent directory；
6. rename 前失败只留下合法命名的 partial；rename 后旧 reader 继续持有旧 inode，新 reader 只看到完整新文件。

条件判断和 commit 必须位于同一 key lock；root 排他锁排除第二个进程。`CreateOnly` 的首次发布使用平台支持的
no-replace primitive，不能靠不受保护的 `exists()`。如果目标平台缺少可证明的 no-replace primitive，则用固定 lock
加 create-new commit protocol，并以 real-process race Gate 证明只有一个 writer 成功。

DELETE 在 per-key lock 下 unlink 最终 `object.ocobj` 并 `fsync` parent。已经打开的 fd 可以继续完成读；新 reader
只能看到 missing 或随后发布的新对象，不能看到半写内容。未知文件和 corrupt envelope 不因 GC 被静默删除。

### 6.5 Multipart

Local multipart 不模拟 HTTP，而是实现同一 operation contract：

- upload ID 使用随机 UUID，manifest 绑定 exact ObjectKey、metadata、customer encryption fingerprint 和创建时间；
- part 文件只写入 `multipart/<upload-id>/parts/`，part number、size、checksum 和 ETag 写入原子 side record；
- 重传同一 part 原子替换，不允许同 ID 改 key 或 encryption key；
- complete 在有序校验后把 parts 流式写入最终目录中的新 partial envelope，执行总长度/part limits/checksum，再走普通 commit；
- complete 成功后清理 upload directory；commit 后 crash 留下的 multipart state由 restart reconciliation 精确识别；
- abort 缺失 upload 也成功；陌生、损坏或不符合固定布局的目录保留为错误证据，不做宽泛递归删除；
- ETag 在通用 backend 合同中是非空、对同一 committed object 稳定的 opaque value；HTTP domain 负责加引号。
  R2 domain 另按官方 Worker API 收紧：single-part/part ETag 是 lowercase MD5，completed multipart ETag 是 ordered
  binary part-MD5 的 MD5 加 `-partCount`。Local 直接生成该格式；S3 preflight拒绝不满足该 R2合同的 provider。

### 6.6 SSE-C

当前 R2 已声明 SSE-C supported，Local backend 不能只比较 `ssecKeyMd5` 后把 plaintext 写盘。统一 operation contract
携带短生命周期的 customer key：

- S3 adapter 映射为 provider SSE-C；
- Local adapter 使用经过评审的分块 AEAD 格式加密 payload，key、plaintext、nonce 不进入日志或 SQLite；
- 每次 object write 使用随机 nonce domain，chunk index、ObjectKey digest、object version、plaintext length 和 format
  version进入 associated data；
- envelope header 只保存公开的 lowercase-hex key MD5、nonce/format 和一个 authenticated key verifier，不保存 plaintext key；
- HEAD 必须验证 key verifier，不能仅依赖可碰撞的 MD5；
- range GET 只解密覆盖范围的 chunks，再裁剪到请求区间；任一 tag/length 失败返回稳定 SSE-C/integrity error；
- multipart parts 在 staging 中同样不得以 plaintext 持久化，complete 不产生 plaintext中间文件。

优先复用 workspace 已固定的 AEAD dependency，但必须为 object format 建立独立 key/AAD domain，不能调用 storage secret
envelope 的业务方法或让 `artifacts -> storage`。加密格式是新的 Day 1 authority；其测试必须覆盖 nonce uniqueness、
wrong/missing key、tamper、range、multipart 和 crash remnants。

## 7. Domain store 改造

### 7.1 `ArtifactStore` 与 cache

- `ArtifactStore` 持有 `ObjectBackend`，content-addressed key、SHA-256、size、immutable create-only 和 GC fence 仍由它管理；
- `ArtifactCache` 继续作为统一、已验证的 execution-facing materialization path，两种 backend 都通过同一 download/hash
  流程进入 cache；P8 不增加 Local 专属旁路；
- 因此 Local 首版可能在 object root 和 artifact cache 各保存一份 hot artifact。这是明确接受的简化，只有测量证明
  容量收益后才另行设计 fd pin/direct materialization；
- cache 仍是 disposable acceleration，object backend 才是 authority。

### 7.2 Tenant R2

- `R2ObjectStore` 不再持有 `S3ArtifactClient`，只从 validated locator 和 hashed user key构造 `ObjectKey`；
- metadata codec 改为 backend-neutral类型，不能依赖 AWS `DateTime`、`ByteStream` 或 fluent builders；
- condition、range、checksum、storage class、SSE-C、multipart 和 restart intent保持现有公开合同；
- `storage class` 在 Local 仅作为 R2 metadata round-trip，不暗示真实磁盘 tier；该现有限制继续记录为 deviation；
- S3 provider capability差异在 S3 preflight/adapter 内处理，不进入 Local 分支。

### 7.3 Snapshot、backup 与 AI Search

- `SnapshotObjectStore`、KV/D1 backup 和 `AiSearchObjectStore` 只调用统一 object operations；
- manifest 字段从 S3 authority fingerprint 改为 object-backend authority fingerprint，并包含 backend kind；
- snapshot restore 要求当前 authority与 manifest一致。P8 不做 local↔S3 restore migration；
- Local snapshot只提供一致性快照，不等于 off-host backup。运维文档必须明确要求复制整个 object root 或选用 S3
  才能覆盖磁盘/主机丢失；
- snapshot cleanup/list只遍历 snapshot domain prefix，不扫描整个 local root。

## 8. Authority、启动与切换规则

backend authority descriptor 使用 canonical、带长度前缀的字段编码后计算 SHA-256：

- Local：format version、root ID、system prefix、R2 prefix；canonical path属于部署配置，不进入持久化authority identity，
  因而完整object root可在停机后搬到新的绝对路径；
- S3：endpoint、region、bucket、path-style、system prefix、R2 prefix；不包含 credentials；
- 两者都绑定 platform ID 的 authority marker。

SQLite 当前 `provider_config_sha256`、snapshot 中 `s3_authority_fingerprint` 及相关命名在 P8 直接改成 backend-neutral
authority，不保留 dual columns、旧 schema reader 或 fallback。现有开发数据库/历史 snapshot 不构成兼容义务。

启动顺序：

1. 解析并静态验证 `[data]` 和唯一 `[storage]` variant；
2. 取得 platform data-dir lock、验证 schema/master key/platform identity；
3. Local：安全打开/初始化 root并取得 backend lock；S3：只在此分支解析 credentials并创建 client；
4. 创建或验证 platform authority marker和 fingerprint；
5. 运行统一 object canary，再运行 R2 capability preflight；
6. 构造一个 `ObjectBackend`，clone 给各 domain store；
7. 打开 artifact cache，继续 runtime/service启动。

任一步失败都在 listener admission 前终止。不得在 Local 初始化失败时尝试 S3，也不得在 S3 暂时不可用时切换 Local。

运行中修改配置文件不切换 backend；需要重启。若平台已初始化而 kind/root ID/endpoint/bucket/prefix fingerprint变化，启动
返回稳定 authority mismatch。更换 S3 credential但 authority不变允许正常轮换；credential本身不进入 fingerprint。

## 9. Health、错误与 observability

跨层命名改为 backend-neutral：

- startup stage/component：`object_storage`；
- readiness：`object_storage_ready`；
- stable internal errors：`OBJECT_STORAGE_UNAVAILABLE`、`OBJECT_STORAGE_INTEGRITY_ERROR`、
  `OBJECT_STORAGE_AUTHORITY_MISMATCH`、`OBJECT_STORAGE_CAPACITY`；
- metrics 使用固定低基数 `backend="local"|"s3"` 和 operation/result，不记录 endpoint、bucket、root、key、upload ID；
- S3-only doctor detail保留 `s3_connectivity`、TLS 和 provider capability；Local 使用 `local_root`、`local_fsync`、
  `local_free_space`、`local_format`；
- support bundle输出 backend kind、format version、authority fingerprint和容量摘要；不包含 credentials、customer key、
  object key、local absolute object path或 payload sample。

Cloudflare R2 对外错误仍由 R2 domain mapping决定，不能把 POSIX errno、AWS SDK exception、filesystem path或 provider body
直接返回 tenant。

## 10. Crash recovery 与 GC

Local backend启动恢复只处理自己能严格证明所有权的状态：

- 删除超过配置 grace，且名称、类型、owner 和 parent layout 均合法的 `.partial` envelope；
- 清理已 committed 或已 aborted 的合法 multipart残留；
- 所有扫描都有 entry count、总 bytes、单 record size和时间预算；超限使 readiness失败或降级，不无限启动；
- unknown name、symlink、special file、bad permission、malformed record、checksum mismatch和指向 root外的路径全部保留并
  报 integrity error；不能为了启动成功自动删除；
- GC 与 put/commit通过现有 domain fence加 local per-key lock协调，不能删除刚发布的 object 或不属于 GC 快照的 key；
- fault point覆盖 envelope fsync 前后、publish rename 前后、delete unlink 后和 multipart complete 各阶段。

S3 adapter沿用现有 unknown-result校验策略：PUT/DELETE timeout后通过 bounded HEAD/GET确认，不因网络错误写入虚假成功。
统一 facade不能把 Local 强一致性假设反向施加给未通过 preflight 的 S3 provider。

## 11. rclone 与开发体验

P8 完成后，开发脚本不再生成 TOML 或为通用开发路径编排 object-server sidecar。仓库直接保留三个
可审阅、可单独使用的普通文件：

```text
scripts/config/
├── dev.toml
├── dev-test.toml
└── dev.env
```

- `dev.toml` 使用 config-relative path 指向持久的 `.data/open-compute`，监听开发端口并选择 Local backend；
- `dev-test.toml` 使用 config-relative path 指向隔离的 `.temp/dev-test`，使用不同的 loopback 端口并选择
  Local backend；该目录可丢弃，但脚本不默认每次删除，以便保留失败证据；
- `dev.env` 只包含两个 loopback 开发配置共用的、明确非秘密的固定 fixture token。它不得包含真实
  credential、主机绑定绝对路径或 S3 credential；
- `OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE`、`OPEN_COMPUTE_TEST_WORKERD` 和可选 `OPEN_COMPUTE_OCD_BIN` 仍由调用者
  environment 显式传入，因为它们是 host-specific verified input，不能提交一个伪造或本机专用值；
- `ocd` 本身不搜索或自动加载 `.env`。`dev.sh` 和 `dev-test.sh` 仅按仓库根解析后的 exact path
  显式加载 `scripts/config/dev.env`，避免 CWD 搜索、隐式 precedence 和生产 secret-loading 合同；
- 不为两个 TOML 引入 include、extends、template 或 env interpolation。文件只保留必要 override，其余使用同一
  `PlatformConfig` 默认与校验规则；它们必须能被普通 `ocd --config <path> config check` 直接读取。

脚本因此收缩为运行编排器：

- `scripts/dev.sh` 加载固定 env 文件，校验必要的 host input，然后以 `scripts/config/dev.toml` 前台启动
  `ocd`；
- `scripts/dev-test.sh` 加载同一 env 文件，以 `scripts/config/dev-test.toml` 启动真实 `ocd` process，
  执行 bounded readiness/HTTP probe，然后停止它；`/health/ready` 必须纳入 smoke 成功条件；
- 删除两个脚本中的 TOML here-doc 和 config temp-file publish，以及 object-server 相关的 executable
  检查、PID/trap、日志、端口等待、`.data/s3` 布局与 `OPEN_COMPUTE_DEV_S3` 分支；`dev-test.sh`
  只保留 `ocd` smoke 必需的 child cleanup、bounded readiness wait 和失败日志；
- S3 contract/Gate使用 repository-owned `open-compute-s3-fixture`，继续覆盖 SigV4、provider failures和 capability
  preflight；fixture endpoint、随机端口、credential 和每个 case 的 temp root 仍由 test harness 动态生成，
  不复用上述共享开发配置，也不以 rclone作为验收依赖；
- release artifact仍只有一个 `ocd`，不嵌入 rclone binary、Go runtime或 sidecar supervisor；
- production S3配置直接连接 operator提供的 S3-compatible endpoint。

删除 rclone 集成不意味着删除 S3 backend；它只删除“为了本地目录而额外启动一个 S3 协议转换进程”的开发路径。

## 13. 必测矩阵

### 13.1 配置路径解析

- 相对 `--config` 按 startup CWD找到 exact file，绝对 `--config` 行为不变，不执行任何隐式搜索；
- 运行命令的 CWD 与 config directory不同时，所有 TOML 相对路径仍只锚定 config directory；
- 嵌套 config directory、`.`、`..` 和 config-directory 外目标均得到 deterministic normalized absolute path；
- 绝对与等价相对配置生成相同 authority descriptor、data root和object root；
- config leaf symlink/FIFO/socket/device/directory、缺失文件、超限文件和非UTF-8全部fail closed；
- parent path含symlink时以实际打开文件的canonical parent为base，替换/移动race不能把已打开config与另一base混配；
- 相对 master key、auth secret file和S3 credential file解析后仍执行 no-follow、regular-file、owner/mode和size检查；
- `config check`、run、doctor、backup/restore和support bundle使用同一resolver，任何下游组件都不重新读取CWD；
- `~`、环境变量文本、glob和URI不展开；移动config后的新base行为有测试和运维说明。

### 13.2 Shared backend contract

- create-only首次成功、重复相同/不同body、replace、if-match成功/失败；
- HEAD/missing、full GET、首/中/尾/越界range、stream中断；
- metadata/content fields/ETag/last-modified canonical round-trip；
- lexical list、limit、pagination cursor、prefix边界、batch delete和missing delete；
- multipart create/retry part/list/complete/abort、乱序/重复/缺失part、restart reconciliation；
- concurrent create/replace/delete/read race只有允许结果，无 torn body/metadata；
- provider/local I/O失败映射为相同 domain-level稳定错误。

### 13.3 Local security 与 durability

- root、每一级 ancestor、leaf、staging、objects 和 multipart 位置的 symlink 攻击；
- `..`、absolute key、empty/dot segment、backslash、NUL、overlong key和prefix collision；
- FIFO/socket/device、异常hard link、错误owner/mode和group/world writable root；
- envelope/header/payload truncate、bit flip、错 key digest 和未知 format；
- 每个fsync/rename/commit/delete fault point后的fresh-process恢复；
- low disk、short write、fsync failure、orphan grace和bounded scan；
- Local `storage.path` 与 `data.path` 所有允许/拒绝的重叠组合；
- SSE-C disk scan不存在plaintext，wrong/missing key、tamper、range和multipart均fail closed。

### 13.4 Product / process Gate

- fresh `ocd` Local启动、restart、shutdown无rclone进程/端口/日志；
- Worker version、Assets、Workers Cache、KV/D1 backup、snapshot和AI Search分别在Local/S3运行；
- R2现有object/body/list/options/checksum/SSE-C/storage class/condition/multipart/restart matrix在两种后端运行；
- local authority marker/root lock冲突、S3 prefix marker冲突和config切换均在listener前失败；
- backup/restore同backend成功，backend/fingerprint不匹配失败且不改变authority；
- health/metrics/doctor/support bundle无secret、customer key、object key或path泄漏；
- test结束后无workerd、fixture、rclone、listener、staging或multipart泄漏。

## 14. 验收依据

冻结源码、实际检查命令、coverage 与最终单轮 Gate 结果保留在第 0 节。
当前执行方式统一见[测试手册](../references/testing.md)。
