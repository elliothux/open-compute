# I99–100：Worker 首次上传类型与 R2 同名重建

状态：**implemented**（2026-09-22）。对应 [#99](https://github.com/elliothux/open-compute/issues/99)、[#100](https://github.com/elliothux/open-compute/issues/100)。

## 用户结果

- `@open-compute/sdk` 的 `client.workers.scripts.update` 可以类型安全地首次上传带 `worker_loader` binding 的 Worker，不要求预先创建空脚本。
- 删除 R2 桶并以同名重建后，Worker 绑定解析到新的 ready ResourceId；旧墓碑仍是不可变历史，不会被复活。

## 实现边界

SDK 生成器复用官方 `ScriptUpdateParams` 和 transport，只扩展 open-compute 已支持的 `worker_loader` binding 联合类型。带 files 的首次上传移除官方 client 错设的固定 `Content-Type`，让 FormData transport 写入含 boundary 的 multipart header；其他 caller headers 保留。

服务端资源名称解析只从 `Ready` 记录中选择，再执行公开 ID 或名称匹配。历史列表继续包含 tombstone，版本创建事务仍负责并发删除竞态的最终校验。这条共同规则覆盖 R2、Vectorize、AI Search 等名称绑定；KV/D1 仍按公开 ID 匹配。

## 验证记录

SDK 类型、multipart wire、同名 tombstone/ready 选择和无 ready 资源拒绝均有定向回归。P6 产品 Gate 覆盖首次上传、R2 删除后同名重建、Worker put/get 和 `ocd` 重启读回。2026-09-22 的 SDK tests 12/12、完整 53-target coverage Gate 和最终非插桩 Gate 通过，记录位于 `.temp/gate-run/20260922T085838-77f1bb0e/` 与 `.temp/gate-run/20260922T092511-751a59fb/`。

这两项不提供 [I102](i102-dynamic-worker-binding-forwarding.md) 的 Dynamic Worker 产品 binding 转发。
