# I99–100：Worker 首次上传类型与 R2 同名重建

状态：**implemented**（2026-09-22；覆盖率与最终非插桩 Gate 均通过）。对应 [#99](https://github.com/elliothux/open-compute/issues/99)、[#100](https://github.com/elliothux/open-compute/issues/100)。

## 用户结果

- `@open-compute/sdk` 的 `client.workers.scripts.update` 可以类型安全地首次上传带 `worker_loader` binding 的 Worker，继续调用官方 SDK transport；不要求先创建空脚本。
- 删除 R2 桶并以同一名称新建后，Worker 上传绑定到**新**资源 ID；旧桶墓碑继续保留为历史记录，不被改写或复活。普通 Worker 和 Dynamic Worker host 使用相同解析规则。

## #99：首次上传的类型与 multipart 编码

当前 [`packages/sdk/scripts/generate.ts`](../packages/sdk/scripts/generate.ts) 只为 `versions.create` 覆盖官方参数类型；生成的 [`packages/sdk/src/generated.ts`](../packages/sdk/src/generated.ts) 中 `scripts.update` 仍是 `BaseScripts["update"]`。固定的 `cloudflare@7.1.0` 中 `ScriptUpdateParams.Metadata.bindings` 没有 `worker_loader`，而 open-compute 的脚本上传 endpoint 已接受此 binding。`versions.create` 要求脚本已存在，不能代替首次上传。

SDK 生成器现在引用官方 `ScriptUpdateParams`，按现有 `OpenComputeWorkerVersionCreateParams` 的方式替换 `metadata.bindings` 联合类型，生成 `OpenComputeWorkerScriptUpdateParams`；`scripts.update(scriptName, params, options)` 的返回类型保留 `ReturnType<BaseScripts["update"]>`。复用已有 `OpenComputeWorkerLoaderBinding`，仍调用官方 SDK transport。生成文件由生成器重建。

运行测试还发现官方 `BaseScripts.update` 为带 `files` 的 FormData 固定设置 `Content-Type: application/javascript`，服务端因此无法解析 multipart。生成的薄封装仅在上传文件时把该 header 置为 `null`，让官方 transport 写入含 boundary 的 multipart Content-Type；调用者传入的其他 headers 保留。此修复是首次上传真实可用的必要条件，不另建 HTTP transport。
SDK 的真实 multipart wire 保留 `File` 的 `application/javascript+module`；Bun 的 `Request.formData()` 回读会把该 `File.type` 变成 `text/javascript;charset=utf-8`，因此 SDK 回归直接检查原始 wire，而不以回读 MIME 推断上传内容。服务端在共同模块类型解析处同时接受官方 SDK 类型文档列出的 `text/javascript+module` 和 `text/javascript`，仍拒绝未知 MIME。

验收：`packages/sdk/tests/positive/signature-overrides.ts` 编译首次上传调用；负向类型用例继续拒绝错误 binding 形状；`packages/sdk/tests/client.test.mjs` 核对 `scripts.update` multipart metadata 确实编码 `worker_loader`。运行 `bun run --filter @open-compute/sdk typecheck`、SDK 测试、生成一致性与 package check。产品真实上传路径再覆盖不存在脚本时的第一次上传。

## #100：名称解析只选择可绑定资源

当前 [`ResourceRepository::list`](../crates/storage/src/resources/repository.rs) 返回包括 `tombstoned` 在内的记录，供管理和历史检查使用；`resources_live_name` 唯一索引允许墓碑之后重用名称。[`UploadInput::resource`](../crates/service/src/workers_http/v4/domain/upload.rs) 却在该列表中按名称取第一条。旧、新桶同名时，它可能选中旧墓碑 ID；后续版本绑定校验要求资源为 `Ready`，因此上传失败并被 v4 投影为 `503 / 9100005`。这也影响同一路径按名称解析的 Vectorize、AI Search 等资源；KV/D1 使用公开 ID 匹配。

`UploadInput::resource` 的共同入口现在先限定 `ResourceState::Ready`，再执行现有公开 ID 或名称匹配。不存在 ready 资源时维持明确的无效 binding 错误；`ResourceRepository::list` 的历史可见性、墓碑、物理前缀和数据库迁移不变。版本创建事务仍负责并发删除竞态的最终校验。

现有定向回归构造旧墓碑和同名新 ready R2、Vectorize 资源，断言生成的 `VersionBindingInput.id` 等于新 ID；新 R2 资源不可绑定时拒绝。SDK 定向测试覆盖首次上传类型、默认与自定义 headers 的 multipart metadata。P6 Wrangler/Loader 产品 Gate 先以 SDK 同形态的 bracketed multipart 首次 PUT 上传 `worker_loader` Script，检查 settings 和真实 Worker 调用；同一 Gate 的 R2 场景还在删除桶后用原名重建，部署按该名称绑定的 Worker，执行 put/get 并在 `ocd` 重启后重读。SDK 测试 12/12、storage/服务定向测试、`p6-cloudflare-sdk`、`p6-wrangler-resources`、完整 53-target 覆盖率 Gate 和最终非插桩 Gate 均通过；工作区行覆盖率为 90.01%。覆盖率 Gate 报告为 `.temp/gate-run/20260922T085838-77f1bb0e/report.json`，最终 Gate 报告为 `.temp/gate-run/20260922T092511-751a59fb/report.json`。

## 边界

这两项互不依赖；#99 修正 SDK 类型和首次上传 multipart header，#100 修正上传时的资源选择。它们都不提供 [#102](i102-dynamic-worker-binding-forwarding.md) 的 Dynamic Worker 资源能力转发。
