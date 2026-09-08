# P4.0：vinext Build Reproducibility 调查

状态：**调查完成；原 No-Go 已撤回（2026-09-01）**。

同一 source、lock、路径和固定 Next build ID 的两次 vinext production build 会生成不同 preview／revalidation credentials，
从而改变 server chunks 的 bytes 与 content-hash 名称。两次 build 都成功，各产生 91 个 output/locator 文件；86 个共同路径中
80 个 bytes 相同、6 个不同，另各有 5 个不同名称的 server chunks。Generated Wrangler config 和 locator 保持一致。

这不是 open-compute importer 问题，也不是 Cloudflare Worker Version／Deployment 的发布要求。正确资格方式是冻结一次正式
output tree，让 Cloudflare 和 open-compute 消费完全相同的 artifact。跨独立 source build 的 byte drift 只保留为
`toolchain-only` deviation，不再阻塞 [P4 Application Go](p4-nextjs-vinext-qualification.md)。

机器可读输入仍由 [`vinext.json`](../../test/conformance/applications/vinext.json) 管理；当前 checker 不构建、不联网或创建资源。
