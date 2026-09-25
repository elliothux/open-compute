# I87、I103–116：Cloudflare 上传、Queue 与上游闭环

状态：**implemented（2026-09-25）**。本批按 Day1 模型直接更新当前 authority、wire、SDK 与文档，不保留旧 pin、旧上传格式、旧 Service descriptor 或 migration 兼容路径。

## 用户结果

- `#87`：`gitserver-core` 改为从固定的 `third_party/gitserver` fork submodule 构建；receive-pack 可解析既有 remote object 作为 thin-pack delta base，连续 push 与 clone/fetch 共用原 Artifacts authority。
- `#103`：formal management set 更新为 Cloudflare OpenAPI `425ceea95cdaa4c43dd462279e42d74fbc00441e`、`cloudflare@7.1.0`、Wrangler `4.138.0`，并同步 subset、capability、SDK、fixtures 与 Bun lock。随后 scan 发现 OpenAPI `01a855ec4bd180a1173f1b4587ef0fd0ca9f55e6` 的四个 AI Search item operation 尚无 stable SDK 对应变化，因此下一候选保持 `blocked`，不漂移 formal pin。
- `#104`：默认升级从 release-hosted `releases/latest/download/release.json` 解析 stable release，不再依赖匿名 GitHub REST 配额；tag manifest、`SHA256SUMS`、artifact 大小与 digest 全部校验。
- `#105`：staged target binary 在替换前校验 active registrations，并在只读 SQLite snapshot 上执行目标 migration。替换前保留 digest-bound binary/receipt；restart/readiness 失败自动恢复，进程中断遗留 backup 时后续命令 fail closed，并提供 `ocd upgrade --restore`。
- `#106/#112`：生成 SDK 的 Script/Version 上传统一发送一个 JSON `metadata` part 加具名 module parts，保留官方 client 的 auth/retry/timeout/error；Service `props`、Artifacts binding 与多步 Durable Object migration 均有精确类型。
- `#110`：AI Search binding 按 public `namespace + instance_name` 解析，namespace 缺省为 `default`；内部 Resource name 不再参与公开身份匹配。
- `#111`：共用 Version pipeline 在 Worker ready 之前 stage、runtime probe 并 publish 全部 reserved Workflow versions。确定性失败拒绝同批 Workflow 与 Worker；transient failure 留给既有 validating/recovery 路径重试。
- `#113`：framework toolchain 接受标准 Wrangler `artifacts` 配置并保留本地 resource identity；SDK upload union 增加与 Wrangler wire 一致的 open-compute 管理端类型。namespace 仍是稳定 account container，不发明 delete API。
- `#114`：operator 可用 `[private_services.<name>]` 把固定私网 HTTP endpoint 暴露为普通 Service Binding `fetch()` target。DNS/address、方法、路径、credential、caller 与 policy revision 都由 host authority 固定并逐调用重验；redirect、RPC、`connect()` 与普通租户私网出站 fail closed。
- `#115`：实现官方 Queue `messages.push` / `bulkPush` 路径与 SDK 方法，复用 durable scheduler enqueue、公开 Queue identity、限制、metrics 与 result-unknown 语义。HTTP pull/ack/peek/purge 不在支持面。
- `#116`：实现官方 Beta Worker Version DELETE。只有非 active、无 pin/持久 referrer 的历史 Version 可 tombstone；删除释放 binding referrer，但不删除当前 Worker 或外部产品数据，重复完成的 delete 可安全重放。同步实现 Wrangler 4.138.0 deploy 新增的 Beta Worker GET prerequisite，返回当前 Worker 身份和明确关闭的 workers.dev/preview 状态。

## 持久化与安全边界

新增 control V10 允许被同一 upload reservation 持有的 validating Worker stage Workflow version；V11 把 Service target policy revision 固定进 immutable Version。V11 遇到旧 extension service row 原子拒绝，避免无 revision 的 descriptor 被静默接受。V1–V9 已发布 migration 字节不变。

私网 Service proxy 只连接启动时解析并固定的 private/loopback address，不跟随 redirect，限制 8 MiB streamed request，移除 platform、tenant auth、cookie、hop-by-hop 与 credential response header；operator credential 只在 host-side request 注入。通用 Worker `fetch()` 仍由 public-only Network authority 管理。

Queue、Workflow、Version 与 upgrade 操作继续使用单机 SQLite authority、现有 fencing/idempotency/restart 语义，不引入分布式 saga 或兼容读写双轨。管理面和单机拓扑差异由持续维护的[兼容矩阵](../references/cloudflare-compatibility.md)与[偏差清单](../references/p1-deviations.md)拥有。

## 验证

实现期间通过了生成 SDK typecheck/request trace、toolchain Artifacts tests、upstream scanner fixtures、P6 contract closure，以及 Rust focused tests：thin-pack push/clone、AI Search namespace identity、Workflow publish-before-ready、Queue push/bulk durability、Beta Version delete、upgrade release/rollback/preflight、private Service policy/proxy 与 migration non-mutation。最终 workspace coverage、静态检查、Cloudflare 兼容性复查和单轮 workspace Gate 由本次变更的最终验收记录为准。
