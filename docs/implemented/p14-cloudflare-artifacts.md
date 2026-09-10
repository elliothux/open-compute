# P14 Cloudflare Artifacts

状态：Day 1 实现完成。当前能力真值由
[`share/cloudflare-capabilities.json`](../../share/cloudflare-capabilities.json)、
[`test/conformance/catalog.json`](../../test/conformance/catalog.json)和本页共同约束。

## 支持范围

open-compute 支持固定 `wrangler@4.127.1` 的标准 `artifacts` binding、Artifacts v4 管理 API、固定
`@cloudflare/workers-types@5.20260830.1` 的 53 个 Worker members/overloads，以及 repo-token Git Smart HTTP：

- account-scoped namespace 与 namespace-scoped repository；
- create/get/list/delete、公开 HTTPS import、独立 fork；
- read/write token 的 issue/list/revoke；
- commit log、commit/tree/blob、file/raw REST reads；
- Git upload-pack v1/v2 与 receive-pack v1；
- Worker `Artifacts` / `ArtifactsRepo` facade；
- immutable Worker Version binding、rollback、snapshot/restore 与 restart reconciliation。

ArtifactFS、event subscription、自动 build/deploy、Git LFS、SSH、private remote import、mirror 和无法真实执行的
jurisdiction placement 不在 Day 1 范围。它们不会以 placeholder、vendor field 或兼容分支存在。

## 唯一 authority

`ocd` 是唯一公开 listener、认证入口和 metadata authority。migration
[`018_cloudflare_artifacts.sql`](../../crates/storage/migrations/018_cloudflare_artifacts.sql)在
`control.sqlite` 中保存 namespace、opaque repository identity、lifecycle generation、token digest/expiry/revoke
metadata 与 immutable Version binding。repository pack/object/ref 是 data-dir 中以 opaque repository ID 命名的
bare Git repository；用户名称从不成为 host path。

live Git authority 与原有内部 immutable `ArtifactStore` 是两个 domain。前者由
[`git_repo.rs`](../../crates/artifacts/src/git_repo.rs)实现，后者继续持有 Worker/source/runtime blobs；没有双写、别名、
旧 schema reader 或互相伪装的路径。snapshot manifest 明确列出 Artifact Git files，restore 在发布 authority 前校验
完整文件集合和摘要。

repository lifecycle 只有一套状态机：`creating/importing/forking -> ready|failed`，以及
`ready|failed -> deleting -> tombstoned`。SQLite mutation 先建立 intent，文件系统 I/O 在 transaction 外执行；启动时
`creating/importing/forking/deleting` 根据实际 bare repository 收敛，损坏或不完整 repository 移入 quarantine 并
fail closed。删除先增加 generation、阻止新 lease、bounded drain，再 rename/delete 和 revoke token；超时发生在文件
mutation 前时回到 `ready`。

## API 与固定客户端

管理面注册 `/client/v4/accounts/{account_id}/artifacts/**` 的 namespace、repository、fork/import、token 和
content route family。request body、object response、repository bytes、concurrency、import timeout、lease drain 与 token
TTL 都由 `[artifacts]` 的有界配置控制。`public_origin` 只接受无 credential/path/query/fragment 的绝对 HTTP(S)
origin。

官方 namespace/repository list 使用 `limit` + opaque `cursor`，token list 使用 `page` + `per_page`。固定 Wrangler
4.127.1 的通用 list helper 会向前两者发送 `page`，所以仅在检测到该参数时返回其所需 page envelope；cursor 与 page
不能同时出现。这是固定客户端的可观察兼容，不是旧 open-compute API。

token plaintext 精确为 `art_v1_<40 lowercase hex>?expires=<unix_seconds>`。Bearer 接受完整 token；Git Basic
忽略 username，并以 `?expires` 前的 secret 作为 password。plaintext 只返回一次；持久层使用 installation key 计算
digest，并以 constant-time 比较授权。expiry、scope、revocation 和 repository generation 都由 SQLite authority
校验。

## Git 与安全边界

Git protocol 由固定 revision 的 `gitserver-core` 接入，repository/object 操作由进程内 `gix` 完成；生产启动不搜索
系统 Git、不下载 runtime、也不增加 sidecar。测试使用系统 Git 仅作为客户端。

公开 import 只允许无 userinfo、query、fragment 的 HTTPS URL。实现禁用 environment proxy 和 redirect，一次解析
hostname 后拒绝 private、loopback、link-local、metadata、unspecified、multicast 与 IPv4-mapped private 地址，并把
已审查地址固定给 TLS/HTTP client，防止 DNS rebinding。body/object/repository byte limits、wall timeout 和 disk
reservation 在 admission 边界执行。

外部请求不能注入 account、Version、binding ID、namespace identity 或 internal token。Worker transport 是
loopback-only private entrypoint，以 immutable Version binding ID、namespace resource ID、generation 和 descriptor
digest 重新授权。错误转换为固定 Artifacts error names/numeric codes，不向 tenant 返回路径、Git stderr、source、token
digest 或内部拓扑。

upload-pack 只接受当前 repository refs 已公告的 `want`，即使调用者知道一个不可达 object ID 也不能读取它。
receive-pack 在读取 pack 前限制 command 数量/字节并完整校验 branch/tag ref name；请求总字节超限稳定返回 413，
非法 packet/ref 返回 400。应用错误使用官方 `101xx`/`102xx`/`103xx`/`104xx` 数字码。

## Worker surface 与兼容性结论

runtime generator 注入 `ArtifactsBinding` 与一个通用 private `ArtifactsTransport.call()`；facade 逐字段验证 backend
response，不把 malformed response 当成功。固定类型包的 `ArtifactsRepo` 只包含 metadata、`createToken`、
`listTokens`、`revokeToken`、`fork`。当前网页文档额外展示的 `log`、`readCommit`、`readTree` 不在固定类型 authority
中，本次不手写扩展；对应对象读取仍由 REST routes 支持。

Artifacts 标记为 `supported_with_deviation`。`OC-ARTIFACTS-001` 只描述单机 bare Git/SQLite authority、operator
capacity 与 Cloudflare hosted placement/replication/quota 的差异；它不掩盖缺方法或降级实现。详细矩阵见
[Cloudflare 兼容矩阵](../references/cloudflare-compatibility.md)和[偏差清单](../references/p1-deviations.md)。

## 验证所有权

- storage tests：名称、jurisdiction、quota、token scope/expiry/revoke、状态迁移和同名重建 fencing；
- artifacts tests：initialize/push/read/fork/delete、object/path limits 与 import URL/egress rejection；
- service tests：REST envelope/cursor、token shape、Bearer/Basic Git、Git v1/v2 push/clone、Worker error mapping、
  immutable binding 和 startup reconciliation；
- snapshot tests：Artifact Git files 的 authenticated snapshot/restore 与 symlink/path rejection；
- runtime tests：固定 Worker facade 全方法、类型/错误与 malformed backend rejection；
- P6 real-process Gate：固定 Wrangler namespace/repository/token commands、Worker deploy 和 Worker binding；
- conformance：标准 config/upload route inventory、53 个 pinned members、positive/negative evidence 与
  `OC-ARTIFACTS-001` 关联。

本次完成验收的精确命令与结果记录在最终任务结果；历史 PASS 不替代当前源码的一轮 Final Gate。
