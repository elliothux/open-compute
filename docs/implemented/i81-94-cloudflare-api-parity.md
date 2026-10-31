# I81–94：Cloudflare API、SDK 与生命周期缺口收敛

当前 CLI/应用配置与构建入口由 [P20](p20-cf-cli-migration.md) 拥有：`ocd cf`、`cloudflare.config.ts`、官方 Vite 插件 v2 与 Build Output。
本文旧客户端命令、版本、fixture 与 PASS 保留为当时证据，不要求恢复旧入口，也不证明当前 cf 的资格。

状态：**implemented（2026-09-17）**。本批次关闭 GitHub issues #81、#84、#86、#88、#89、#91、#92、#93 与 #94；实现直接更新当前 Day1 模型，不保留旧 SDK、旧 wire 或持久数据兼容分支。

## 最终合同

- `@open-compute/sdk` 的标准面由 pinned Cloudflare OpenAPI、仅含 Artifacts 的 observed-standard overlay 与 open-compute extension 共同生成。`client.artifacts` 使用唯一 first-party delegate 和官方 `BaseCloudflare` transport；`client.openCompute` 只承载本地扩展。surface report 对 operation 显式记录 `official`、`observed_standard` 或 `open_compute_extension`。
- Worker Version create 的参数联合包含 `worker_loader`；Assets upload 接受 `Record<string, string | File>`。两者仍调用官方 SDK method，multipart filename、MIME 与 base64 body 不经过第二套 transport。
- 官方 R2 List Objects route 已开放，返回官方 object/result-info 外形，并复用 Worker R2 的 catalog、opaque cursor、metadata reconciliation 与 restart-safe authority。
- Worker 管理错误在唯一的 `platform_error` 边界记录 request ID、稳定 platform code 和 secret-free message；Cloudflare v4 response 不暴露内部原因。
- Script upload 能重建固定 Wrangler 的完整 observability bracketed fields。Deployment、active pointer 与 Script observability patch 在一个 SQLite publish transaction 内提交；Version-only upload 拒绝 observability。
- D1 migrations 通过 `client.openCompute.d1.migrations.{list,apply}` 暴露。engine 只接受精确 `1..N` chain，已应用 ledger 必须是请求 chain 的精确前缀；drift 返回稳定 `409 / 9100006`。
- `WorkerCode.env` 只允许 structured-clone value 和 Service Binding。D1、KV、R2 与 Queue binding 不直接转移并由原生 structured clone 返回 `DataCloneError`；资源访问通过 `ctx.exports` Service wrapper 委派。
- Worker `force=true` 删除先持久化 intent 并 fence admission；generation retention 阻塞时受控轮换唯一 workerd generation。最终 transaction tombstone Worker authority、释放全部历史 Version referrer，并允许入站 Service target fail closed。启动在 runtime admission 前幂等完成遗留 intent。Cloudflare 会级联删除关联 binding／Durable Object 的差异由 `OC-DEPLOY-001` 显式登记；open-compute 保留独立的外部资源 authority。

## Authority

```text
pinned Cloudflare OpenAPI subset
  + openapi/cloudflare-observed-standard.json (Artifacts only)
  + openapi/open-compute-extension.json
  -> openapi/open-compute-sdk.json
  -> packages/sdk/src/generated.ts + packages/sdk/surface.json
```

Artifacts overlay 单独固定 evidence 与 digest，不改写上游 snapshot。R2 List Objects 现在属于 official subset；D1 migration 始终属于 vendor extension。公开节点不提供 generic raw request、历史 alias 或双 delegate。

## 持久化与恢复

平台 migration `V5__worker_delete_intents.sql` 只增加 force-delete intent 与阻止新 Version/Deployment 的数据库 guard；此前已发布 migration 未修改。observability publish 复用现有表，不新增 schema。D1 tenant migration ledger 不修复、重排或回填不完整 chain，任何 drift 都 fail closed。

## 验收

Focused 回归覆盖 SDK authority/类型/multipart/binary/pagination/path、R2 list wire、D1 chain 与 route boundary、observability 原子 publish、force-delete referrer/recovery，以及 workerd generation rotation。`bun run build`、frontend/JS、format、Clippy、no-default-features、MSRV、metadata 与 dependency-boundary checks 均通过。插桩 workspace Gate 的全部 Rust／真实 workerd／Wrangler／SDK／workflow／single-binary 目标通过；唯一失败是 `p3-contract::baseline-identity` 在清空 `HOME` 后受 Git LFS 全局 filter 影响。源码摘要已改为 `git hash-object --no-filters`，同一 clean environment 的定向 case 通过；按用户要求未重跑完整 Gate。

现有完整 Gate profile 加 D1 migration apply/list/drift 定向覆盖的最终 Rust line coverage 为 **90.06%（135285/150209）**，报告位于 `target/llvm-cov/`。新增 migration HTTP 成功与冲突路径、generation rotation、R2 list、observability 与 force-delete 均有回归覆盖。

Cloudflare 兼容性检查以固定 `cloudflare@7.1.0`、Wrangler `4.127.1`、workers-types `5.20260830.1`、formal workerd pin 与官方 [R2 List Objects](https://developers.cloudflare.com/api/resources/r2/subresources/buckets/subresources/objects/methods/list/)、[Worker upload](https://developers.cloudflare.com/api/resources/workers/subresources/scripts/methods/update/)、[Dynamic Workers](https://developers.cloudflare.com/dynamic-workers/api-reference/)、[Artifacts REST](https://developers.cloudflare.com/artifacts/api/rest-api/)、[D1 migrations](https://developers.cloudflare.com/d1/reference/migrations/) 和 [Worker delete](https://developers.cloudflare.com/api/resources/workers/subresources/scripts/methods/delete/) 合同为证据。检查确认 R2 list、observability、Artifacts 与 D1 vendor 边界匹配；Worker force-delete 的关联资源级联差异已挂到既有 `OC-DEPLOY-001`，没有新增 deviation ID。
