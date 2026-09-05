# P11：Cloudflare Artifacts 兼容设计

状态：Day 1 合同与架构设计完成；待 G0、实施与验收。

本文细化 [P6 Cloudflare v4 API 与 Wrangler 子集兼容设计](implemented/p6-cloudflare-v4-wrangler-compatibility.md)
中的 `artifacts` binding、Artifacts v4 API、Worker binding 和 Git Smart HTTP data plane。Workers Logs、limits 与
Browser Run 分别见 [P7](implemented/p7-workers-logs-realtime-tail.md)、[P9](blocked/p9-workers-standard-limits.md)和
[P12](p12-browser-run.md)。平台内部 blob 的 Local / S3 持有方式见
[P8 对象后端设计](implemented/p8-local-s3-object-backend.md)。

## 1. 范围与结论

P11 的目标是让面向 Cloudflare Artifacts 编写的标准工具和 Worker 在 open-compute 上工作：

- `wrangler.jsonc` 中标准 `artifacts` binding；
- 固定 Wrangler 生成的标准 multipart upload metadata；
- `/client/v4/accounts/{account_id}/artifacts/**` 管理与只读对象 API；
- Worker 内 `env.<binding>` 的 Artifacts API；
- 使用 repo token 的 Git Smart HTTP clone、fetch 和 push；
- namespace、repository、token、fork/import、删除、备份与恢复的一致 authority。

Day 1 不把 Artifacts 做成 LynxOS 文件系统，也不把 Git repository 当作个人目录 ACL 的实现。Artifacts 是版本化、
内容寻址、以 Git 语义读写的开发制品仓库；LynxOS 的团队目录、个人目录和 `private/` 目录仍属于上层文件服务。

明确不在 P11 Day 1：ArtifactFS mount/API、Artifacts event subscriptions/Queues source、自动 build/deploy workflow、Git
LFS、SSH、private remote import、repository mirror，以及无法真实执行的 `eu`/`us` data-localization placement。它们可以
在后续专项中沿用同一 repo authority，但不能以 open-compute vendor field 提前出现。

选定的生产边界是：

1. `ocd` 是唯一公开 listener、认证入口和 metadata authority；
2. 正式 open-compute 发布物仍是单个原生 `ocd`，不能要求运行时搜索 `git`、自动下载 Git binary，或随包发布
   `artifactd` sidecar；
3. repository object/ref/pack 的实现使用进程内 Git engine，物理 repository 位于 operator data directory；
4. `crates/artifacts` 的内部 immutable `ArtifactStore`（由选定的 Local / S3 `ObjectBackend` 持有）与
   Cloudflare Artifacts 是两个 domain，不能直接把前者公开成 v4 Artifacts；它可以作为 snapshot/backup transport，
   但不是 live Git authority；
5. G0 必须先证明选定的进程内 engine 能完整且安全地支持目标 Git protocol。未通过前，`artifacts` upload 与所有
   Artifacts route 均 fail closed。

## 2. Compatibility authority

实施与验收固定到以下 authority：

- [Cloudflare Artifacts REST API](https://developers.cloudflare.com/artifacts/api/rest-api/)；
- [Cloudflare Artifacts Workers binding](https://developers.cloudflare.com/artifacts/api/workers-binding/)；
- [Cloudflare Artifacts Git protocol](https://developers.cloudflare.com/artifacts/api/git-protocol/)；
- [Cloudflare Artifacts platform limits](https://developers.cloudflare.com/artifacts/platform/limits/)；
- `wrangler@4.127.1` 的 `config-schema.json`、Artifacts commands、upload builder 与 tests；
- 固定 `@cloudflare/workers-types` 版本中的 Artifacts types；
- 固定官方 OpenAPI snapshot、HTTP trace 与 Git packet fixtures。

Artifacts 当前是 Beta。网页只用于发现合同；route、字段、错误、pagination、媒体类型和 Worker type 必须进入
machine-readable conformance inventory 后才算平台承诺。Cloudflare 后续新增字段/route 不会被自动视为支持。

## 3. 与现有内部 ArtifactStore 的边界

仓库的 `crates/artifacts` 提供平台内部 immutable blob authority：内容经 SHA-256 标识并由 P8 的 Local / S3
`ObjectBackend` 持有。Cloudflare Artifacts 则拥有 namespace、repository、Git ref、commit/tree/blob、repo token
和 Smart HTTP 协议，两者不具备可互换的 wire contract。

| 能力 | 内部 ArtifactStore | P11 Cloudflare Artifacts |
| --- | --- | --- |
| identity | SHA-256 `ArtifactRef` | account + namespace + repo；Git object ID/ref |
| mutation | immutable blob put/get | commit/ref/pack 与 repo lifecycle |
| transport | internal service + Local/S3 ObjectBackend | v4 API、Worker binding、Git Smart HTTP |
| auth | internal platform scope | account token、Version binding、repo token |
| primary use | Worker/source/runtime artifacts | user-visible Git artifact repositories |

实现时在 `crates/artifacts` 内增加明确的 `git_repo` domain 模块和独立 types；禁止给现有 `ArtifactRef`、bucket key
或 backend key/URL/path 增加公开含义。若后续把 Git repo snapshot 送入内部 store，restore 必须从完整、验证过的
snapshot 重建，不能在 live ref transaction 中跨 SQLite/object backend 做伪原子提交。

## 4. Wrangler 与 Worker upload contract

### 4.1 `wrangler.jsonc`

唯一标准形态：

```jsonc
{
  "$schema": "./node_modules/wrangler/config-schema.json",
  "name": "artifact-app",
  "main": "src/index.ts",
  "compatibility_date": "2026-09-03",
  "artifacts": [
    {
      "binding": "ARTIFACTS",
      "namespace": "default"
    }
  ]
}
```

`remote` 若出现在固定 schema 中，只影响 Wrangler/Miniflare local-development routing，不进入 server-side immutable
Version state。Day 1 不增加 endpoint、token、provider、path、team 或 private 等自定义 key。

规则：

- `artifacts` 是 non-inheritable array，named environment 必须显式重复声明；
- 每项只接受固定 schema 中的 `binding`、`namespace` 和 local-only `remote`；
- binding name 参与所有 binding 类型的全局唯一校验；
- namespace 必须已存在且属于同 account；upload 不隐式创建 namespace；
- unsupported/unknown field 直接拒绝，不能 warning 后忽略。

### 4.2 Multipart metadata

固定 Wrangler upload builder 生成：

```json
{
  "bindings": [
    {
      "name": "ARTIFACTS",
      "type": "artifacts",
      "namespace": "default"
    }
  ]
}
```

P11 完成前，P6 decoder 识别该标准 binding 后返回标准 v4 failure；不能删除 binding 后创建一个功能不完整的
Version。P11 完成后：

- descriptor 是 immutable Version state；
- upload 时解析 `(account_id, namespace)` authority，但 snapshot 不保存 provider credential 或 host path；
- settings/download/rollback 按固定 Cloudflare response round-trip；
- 删除 namespace 或失去权限不能让旧 Version fallback 到同名、跨 account 或新建 namespace。

## 5. v4 REST API contract

API base 固定为：

```text
/client/v4/accounts/{account_id}/artifacts/namespaces
```

### 5.1 Route inventory

下表是 P11 目标族；每个 route 的 verb、path、query、body、response、status 和 error code 以固定 OpenAPI/trace 为准，
不能从相邻 Cloudflare product 推断。

| 能力 | 标准 route family | Day 1 |
| --- | --- | --- |
| namespaces | `POST/GET /namespaces`, `GET /namespaces/{namespace}` | 支持 |
| repositories | `POST/GET /{namespace}/repos`, `GET/DELETE /{namespace}/repos/{repo}` | 支持 |
| fork | `POST /{namespace}/repos/{repo}/fork` | 支持，异步状态受控 |
| import | `POST /{namespace}/repos/{repo}/import` | 只支持公开 HTTPS remote；通过 G0 与 egress policy 后支持 |
| log | `GET /{namespace}/repos/{repo}/log` | 支持 |
| commit | `GET /{namespace}/repos/{repo}/commit/{commit}` | 支持 |
| tree | `GET /{namespace}/repos/{repo}/tree/{tree}` | 支持 |
| blob | `GET /{namespace}/repos/{repo}/blob/{blob}` | 支持 |
| file/raw | repo 下 `file` / `raw` route family | 支持，path/query 由固定 fixture 锁定 |
| tokens | namespace issue、repo list、token revoke route family | 支持 |

固定 Wrangler CLI 当前覆盖：

```text
wrangler artifacts namespaces list
wrangler artifacts namespaces get <namespace>
wrangler artifacts repos create <repo> --namespace <namespace>
wrangler artifacts repos list --namespace <namespace>
wrangler artifacts repos get <repo> --namespace <namespace>
wrangler artifacts repos delete <repo> --namespace <namespace>
wrangler artifacts repos issue-token <repo> --namespace <namespace>
```

CLI 覆盖面小于公开 REST/Worker binding，不能用“Wrangler command 通过”替代完整 route inventory。

### 5.2 Envelope、raw body 与 pagination

- JSON management response 使用 Cloudflare v4 success/failure envelope；
- list route 使用固定 v4 pagination metadata，不复用 Operator API cursor；
- Git blob、file/raw 和 Git protocol 返回原始 bytes/stream，不套 JSON envelope；
- conditional/range/content-type/content-disposition 行为只按固定官方 trace 支持；
- 路由存在但媒体类型、query 或 object kind 不支持时返回固定错误，不回退为 JSON base64；
- object/read endpoint 必须 bounded streaming，不能把未知大小的 pack/blob 全量读入内存。

P6 的统一 v4 protocol core 负责 request ID、认证、envelope 和 error mapping；P11 的 raw/Git routes 明确注册为例外，
避免统一 response middleware 把 bytes 或 Git packet 改写成 JSON。

## 6. Worker binding contract

`env.ARTIFACTS` 是 namespace-scoped capability。固定 Workers types 和 differential 建立完整 method inventory：

```text
Artifacts.create(name, opts?)
Artifacts.get(name)
Artifacts.list(opts?)
Artifacts.import(params)
Artifacts.delete(name)

ArtifactsRepo.createToken(scope?, ttl?)
ArtifactsRepo.listTokens()
ArtifactsRepo.revokeToken(tokenOrId)
ArtifactsRepo.fork(name, opts?)
ArtifactsRepo.log(opts?)
ArtifactsRepo.readCommit(hash)
ArtifactsRepo.readTree(hash)
```

参数 camelCase、返回 handle、pagination、not-found、read-only、status、token plaintext-once 与 async operation behavior
全部按固定 types/official differential，不把 REST 的 snake_case DTO 直接暴露给 Worker。

本文不手写一个“相似 SDK”。runtime 使用与其他 bindings 相同的 typed facade + host transport：

```text
tenant Worker
  -> packages/runtime Artifacts facade
  -> scoped host transport
  -> Rust Artifacts service
  -> metadata authority + Git repo engine
```

关键 invariant：

- Worker 只能访问其 Version 已绑定的 namespace；method 参数不能切换 namespace/account；
- facade 不接收 API token、repo root、provider URL 或 physical ID；
- repository name 每次在绑定 namespace 内解析，删除/重建使用新的 opaque repo ID；
- list/read/async iterator 和 error class 必须与固定 types/runtime behavior qualification；
- method 计入 P9 subrequest/resource accounting；日志按 P7 做 redaction，不能记录 file/blob body 或 token plaintext。

Miniflare 的 Artifacts plugin 目前是 remote proxy，只能作为 binding 注入/shape 的证据，不能作为本地 storage engine 或
生产语义参考。

## 7. Git Smart HTTP data plane

### 7.1 支持范围

Day 1 目标：

- Git Smart HTTP `upload-pack` protocol v1/v2，用于 clone/fetch；
- Git Smart HTTP `receive-pack` protocol v1，用于 push；
- Bearer 与固定 Cloudflare 支持的 Basic token form；
- 标准 discovery、content type、pkt-line、sideband、thin pack 与 capability negotiation；
- atomic ref update、non-fast-forward/read-only/expired token rejection；
- bounded streaming、disconnect cancellation 和 temp pack cleanup。

Day 1 不支持 SSH、Git LFS、dumb HTTP、filesystem path access、server-side hooks 或任意 shell hook。Git object format
先锁定 SHA-1；Git SHA-256 repository 只有在 Cloudflare authority 与选定 engine 都通过 differential 后才开放。

### 7.2 入口与 remote

`ocd` 是唯一公开入口。REST create/get 返回的 `remote` 指向 operator 配置的 deployment-owned Artifacts HTTPS origin，
路径贴近 Cloudflare 返回的 Git remote，而不是伪装成 v4 management route，例如：

```text
https://<artifacts-origin>/git/<namespace>/<repo>.git
```

account 可由受信任 host mapping 或内部 route context 解析；最终 path/credential behavior 以 Cloudflare trace 固定。
禁止返回 `file://`、data-dir、loopback backend URL 或内部 repo ID。反向代理部署必须用 configured public origin 生成
URL，不能无条件相信来访 `Host` / forwarded headers；该 origin 最终仍路由到唯一公开的 `ocd` listener。

### 7.3 Token

- create/issue response 只返回一次 plaintext；数据库只保存 keyed hash、token ID、repo ID、scope、expiry、revocation；
- `read` token 不可 push；`write` 的读权限按固定 Cloudflare behavior qualification；
- token 不能访问同 namespace 其他 repo，也不能兑换 v4 account authority；
- token compare constant-time，认证失败使用不泄露 repo existence 的统一响应；
- Git access log 只记录 token ID/digest prefix，禁止 plaintext、Authorization 和 URL credential；
- revoke/expiry 对新请求立即生效；已经开始的 request 是否中断由固定 differential 决定并写入 deviation catalog。

## 8. 数据模型与 authority

SQLite 是 metadata authority，repository filesystem 是 Git object/ref authority。建议表：

```text
artifact_namespaces
  id, account_id, name, jurisdiction, created_at, updated_at, tombstoned_at

artifact_repositories
  id, namespace_id, name, description, default_branch, read_only,
  source, state, generation, created_at, updated_at, last_push_at, deleted_at

artifact_repo_tokens
  id, repo_id, secret_hash, scope, expires_at, revoked_at, created_at

artifact_repo_jobs
  id, repo_id, kind, state, source_redacted, attempts, error_class,
  created_at, started_at, finished_at
```

外键和 unique constraint 必须保证：

- namespace name 在 account 内唯一；repo name 在 namespace 内唯一；
- namespace `jurisdiction` immutable；Day 1 on-prem 只支持 omitted/unrestricted，`eu`/`us` 请求在没有真实 placement
  enforcement 时 fail closed，不能只保存标签后宣称 data localization；
- tombstoned resource 不能被旧 Version/repo token 复活；
- state transition 使用 compare-and-set generation；
- internal failure state 可以比公开 `creating/ready/importing/forking` 更细，但 wire response 只能返回固定公开状态；
- filesystem directory 使用 opaque repo ID，不使用未经处理的 account/namespace/repo name。

Repository 状态机：

```text
creating -> ready
importing -> ready
forking -> ready
* -> deleting -> tombstoned
creating/importing/forking -> failed -> deleting/retry
```

对外 response 的异步/同步时机由固定 Cloudflare trace 决定。内部 job 不能为了方便改变标准 status 或让 half-created
repo 被 Git remote 访问。

## 9. 进程内 Git engine 与磁盘布局

G0 比较并选择一个 Rust 可嵌入 Git implementation。正式方案必须满足：

- 不调用 PATH 中的 `git`，不启动长期 sidecar，不在 production startup 下载 binary；
- 依赖 license、供应链、unsafe、协议完整性和跨平台支持经过 audit；
- bare repo、pack/index、ref transaction、upload-pack/receive-pack、protocol v2 和 cancellation 有可验证实现；
- file descriptor、pack size、delta depth、object traversal、CPU 与 temp disk 都有 operator capacity guard；
- crash 后 lock/temp file 可清理，已提交 ref 保持原子；
- Linux/macOS/Windows 的 path、rename、fsync/flush semantics 有测试。

建议布局：

```text
<operator-data-dir>/artifacts/repos/<opaque-repo-id>.git/
<operator-data-dir>/artifacts/tmp/<opaque-job-or-request-id>/
<operator-data-dir>/artifacts/snapshots/<opaque-snapshot-id>/
```

路径只能由 validated opaque ID 组合。任何 API path、Git ref、tree path、archive entry 或 imported URL 都不能参与 host
path join。repository lock 顺序统一为 metadata generation -> repo mutation -> final metadata update，禁止持 SQLite write
transaction 跨整个网络 upload/import。

## 10. Fork、import 与删除

### 10.1 Fork

fork 必须创建独立 repository identity。对象层可以利用 hardlink/reflink/alternates 优化，但不能形成源 repo 删除后目标
repo 不可读的隐藏依赖。Day 1 最稳妥路径是：

1. 创建 target metadata 为 `forking`；
2. 从一致 source ref snapshot 复制/导入 objects；
3. 建立 refs/default branch；
4. 完整 fsck/可达性校验；
5. 原子切换 `ready`。

### 10.2 Import

import 是明确的 outbound capability，不接受 tenant 提供 provider credential 之外的 operator egress bypass：

- 只允许固定 `https`/受支持 Git URL scheme；
- DNS resolution、redirect、private/link-local/metadata IP、port 和 proxy 遵循统一 egress policy；
- credential 不写 source 字段、日志或 error；
- clone size、object count、pack/delta、duration 与 redirect 有 operator guard；
- egress 不可用或安全语义未实现时，route fail closed，不做“只允许内网所以跳过校验”的特殊模式。

私有 remote import 若固定 Cloudflare API 没有可安全表达 credential 的合同，Day 1 不支持；不新增 open-compute-only body
field。

### 10.3 Delete

删除先 tombstone metadata、拒绝新 API/Git/Worker operation，再等待 in-flight lease drain，最后移动到 recoverable quarantine
并由后台 maintenance 清理。用户可见语义以固定 API 为准；内部保留期是 operator policy，不承诺 Cloudflare 等价。

## 11. LynxOS 的正确映射

Artifacts 适合保存 agent 生成的应用源码、模板、构建输入和可审计配置，但不能直接承担实时桌面文件语义：

- 团队共享应用可以使用 team-owned repository；
- 个人应用使用 user-owned repository，发布到团队环境是显式 promote/copy/fork/deploy workflow；
- `private/` 不能只是同一 Git repo 中的一个 ACL 子目录，因为历史 commit、tree、pack 和 clone 会泄露旧内容；
- 私有内容必须使用独立 repository/namespace 或根本不进入 Artifacts；
- Lynx identity/RBAC 把用户动作兑换为 scoped open-compute capability，不能把 repo token 长期放入桌面文件；
- 应用发布仍创建标准 Worker Script/Version/Deployment，Artifacts repo 不等于已部署应用。

这些规则属于 LynxOS product policy，不写成 open-compute 的“约 20 人默认值”。open-compute 只暴露 operator capacity
与 Cloudflare-compatible protocol。

## 12. Limits、backpressure 与 availability

Cloudflare 商业 plan 数值不复制成本地默认值。P11 提供 operator-owned capacity knobs：

- namespaces/repos/tokens per scope；
- concurrent Git requests、pushes、fork/import jobs；
- request body、pack、object、file/raw response、repo total size、temp disk；
- object traversal/delta depth、wall time、CPU work budget；
- token TTL range、pagination size、job retry/retention；
- disk low-watermark、maintenance/GC concurrency。

这些值通过 operator config/capability response 暴露为 deployment capacity，不伪装为 Cloudflare limit。队列满、磁盘不足、
backend busy、timeout、corrupt repo 必须有稳定 error class 和 retryability；绝不无限排队、无限缓存 body 或让一个 push
拖垮 v4/control-plane listener。

## 13. Backup、restore、GC 与 crash recovery

- SQLite metadata 与 Git repos 必须形成带 generation/manifest 的一致 backup set；
- 单独复制 `.git` 或单独恢复 DB 都不算可用备份；
- snapshot 写入 staging，校验 refs/object reachability 后发布 manifest；
- restore 到空 data directory，验证 repo IDs、token hashes、default branch 与 refs 后才启动 listener；
- repo token plaintext不可备份，因为 authority 从不保存 plaintext；restore 后未过期 hash 保持有效；
- GC 不能与 receive-pack/fork/import/backup 无协调并发；
- startup reconciliation 处理 `creating/importing/forking/deleting`、stale locks、temp packs、metadata/repo 缺失；
- corruption 默认隔离并 fail closed，不静默回退到旧 snapshot。

## 14. Security invariants

- account token、Worker binding、repo token 是三类独立 authority，不能互换；
- namespace/repo 名称、Git ref/tree path、query 与 import URL 均不成为 host filesystem path；
- upload-pack 不泄露不可达 object；receive-pack 在 auth/read-only/ref validation 前不提交 ref；
- symbolic ref、alternates、submodule URL、hook、object replacement 和 protocol extension 有显式 allowlist；
- fork/import 不读取本机 path，不允许 `file://`；
- response/error/log 不含 provider credential、token plaintext、Authorization、object/file content 或 data-dir；
- Git packet、compressed object、delta chain 和 archive/path 输入按 hostile bytes 处理；
- repo job 与 request 使用 bounded temp directory，cancel/crash 后可回收；
- 删除/重建同名资源使用新 opaque ID，旧 token/Version snapshot 不能命中新资源。

## 15. Observability

P7 为 P11 提供统一日志/trace sink。推荐稳定维度：

```text
account_id, namespace_id, repo_id, operation, protocol,
result_class, http_status, bytes_in, bytes_out, duration_ms,
queue_wait_ms, object_count, job_state
```

repo/name 可作为 bounded/redacted display field，不能作为无界 metrics label。禁止记录 commit message、file path、Git packet、
blob body、import credential 和 repo token。Git request、v4 request、Worker binding call 使用同一 correlation ID，但内部
filesystem/S3/provider detail不出现在 tenant error。

## 16. 实施顺序

### AR0：冻结合同

- 固定 Wrangler schema/package integrity、Artifacts command 与 upload metadata fixtures；
- 固定 official OpenAPI、Workers types、REST/raw/Git traces；
- 建立 route/field/method/Git capability conformance inventory；
- 把当前内部 ArtifactStore 与公开 Artifacts type/name 分离。

Exit：所有公开承诺都有固定 authority；unknown route/field/method 均 fail closed。

### AR-G0：Git engine feasibility Gate

- 选定可嵌入、可发行、跨平台的 Git engine；
- 真实 `git clone/fetch/push` 通过 v1/v2/receive-pack、auth 与 atomic ref tests；
- 大 pack、恶意 pkt-line/delta、disconnect、cancel、crash/temp cleanup 有界；
- 不依赖 PATH Git、runtime download、私有 workerd fork或第二个 open-compute daemon；
- license、unsafe、dependency boundary 与维护成本复审通过。

Exit：若失败，P11 保持 unsupported；不能降级成仅 Wrangler CRUD 或自定义 zip store。

### AR1：metadata 与 lifecycle

- namespace/repo/token/job schema、migration 与 typed IDs；
- state machine、tombstone、generation CAS、reconciliation；
- data directory layout、disk guard、quarantine 与 maintenance。

### AR2：Git data plane

- discovery、upload-pack、receive-pack、streaming/backpressure；
- repo token、read-only、atomic refs、cancel；
- clone/fetch/push interoperability 和 negative protocol tests。

### AR3：v4 REST

- namespace/repo/fork/import/token/object/file/raw route families；
- v4 envelope/pagination 与 raw response exceptions；
- standard error mapping、public origin/remote 生成、capability manifest。

### AR4：Worker binding

- multipart decode/Version persistence/runtime snapshot；
- typed JS facade + scoped transport；
- fixed Workers types/API differential；
- P9 subrequest accounting 与 P7 logging。

### AR5：durability

- fork/import jobs、backup/restore/GC；
- startup/crash reconciliation、corruption quarantine；
- soak、disk exhaustion、concurrent push/read/delete。

### AR6：qualification

- fixed Wrangler subprocess matrix；
- official Cloudflare SDK/Worker binding fixtures；
- Git CLI/JGit/libgit2 client interoperability；
- Cloudflare differential 或独立 credential-blocked acceptance；
- compatibility/deviation/reference/capability authority 同步。

## 17. 必测矩阵

| case | 预期 |
| --- | --- |
| standard JSONC `artifacts` | metadata 精确为 `{name,type:"artifacts",namespace}` |
| local-only `remote` | 不进入 uploaded Version state |
| unknown binding field | upload fail closed，无部分 Version |
| cross-account namespace | upload/runtime/API 均拒绝且不泄露存在性 |
| create/list/get/delete repo | 固定 v4 envelope、字段、pagination、status |
| blob/file/raw | bytes 与 media headers 保持，不包 JSON |
| Worker binding | 只能访问绑定 namespace；method/error 与固定 types 一致 |
| clone/fetch protocol v1/v2 | 标准 Git client 成功、结果 refs/objects 一致 |
| write/read/expired/revoked token | 权限与错误稳定，日志无 plaintext |
| concurrent push same ref | atomic compare/update，无 silent lost update |
| interrupted push | 已提交 refs 完整；temp pack 最终回收 |
| read-only repo push | receive-pack 在 ref mutation 前拒绝 |
| fork source delete | 已 ready fork 仍完整可读 |
| import redirect/private IP/oversize | egress/size policy fail closed |
| delete during clone/push | lease/tombstone 行为确定，无 use-after-delete |
| crash in every lifecycle state | reconciliation 收敛到 ready/failed/tombstoned |
| backup + empty-dir restore | refs/objects/metadata/token hash 一致 |
| repo corruption | 隔离、告警、fail closed，不返回错误 object |

## 18. Definition of Done

P11 只有同时满足以下条件才可归档：

- `wrangler@4.127.1` 精确 pin 的 Artifacts config、upload 与全部现有 commands 对真实 `ocd` 通过；
- 标准 `/client/v4` Artifacts route、v4 envelope、pagination、raw bytes 与错误合同通过固定 trace；
- 固定 Workers types 的 binding method inventory 对 stock workerd 通过，无自定义 client/config key；
- 标准 Git clients 对 Smart HTTP clone/fetch/push、token、read-only 与 atomic refs 通过；
- 单个 `ocd` 正式发布物不依赖 PATH Git、startup download 或 bundled/managed sidecar；
- import/fork/delete/crash/restart/backup/restore/GC 和容量边界通过 security/failure tests；
- 内部 ArtifactStore 与 Cloudflare Artifacts authority 没有 wire/type/identity 混用；
- P7/P9 集成完成，或相应调用在 capability inventory 中保持明确 planned 且不会伪装完整支持；
- Cloudflare differential 完成，或剩余 credential 限制拆成独立 active acceptance；
- P6、reference、capability manifest、examples、runbook 与 Dashboard 同步。

文档变更本身只运行 `git diff --check`、链接和固定命令/源码核对。实现属于 protocol、persistence、filesystem、
security、runtime 与 release 变更，必须执行仓库 `AGENTS.md` 要求的 focused tests、coverage 与最终 workspace Gate。
