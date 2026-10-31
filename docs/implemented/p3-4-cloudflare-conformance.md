# P3.4：Cloudflare Conformance

当前 CLI/应用配置与构建入口由 [P20](p20-cf-cli-migration.md) 拥有：`ocd cf`、`cloudflare.config.ts`、官方 Vite 插件 v2 与 Build Output。
本文旧客户端命令、版本、fixture 与 PASS 保留为当时证据，不要求恢复旧入口，也不证明当前 cf 的资格。

状态：**implemented（2026-09-01）**。

## 最终结果

- 固定 workers-types、workerd、官方文档／源码和 portable Cloudflare observation 形成一份机器可读 contract catalog。
- Capability 只使用 `supported`、`supported_with_deviation`、`unsupported` 和 `blocked`；类型存在不等于 runtime capability 已授予。
- 平台和应用分别判定。第三方应用通过不能替代 API、隔离和恢复 Gate，应用失败也不自动成为平台缺口。
- 同一 portable fixture 可在 open-compute 和真实 Cloudflare 执行，只归一化已登记的时间、provider identity 或拓扑差异。
- Account、Worker、deployment、entrypoint、binding、cache、lifecycle、error 和 metrics 的隔离由产品 Gate 负责。
- Conformance report 拒绝 duplicate、missing、unknown、ignored 和零执行 case，不把未运行项计入通过率。

当前支持面只在[兼容矩阵](../references/cloudflare-compatibility.md)、[能力偏差](../references/p1-deviations.md)和机器可读 catalog 中维护。

## 历史验证与限制

当日 inventory 为 2,097 个 stable tenant API members、`blocked=0`；Workers、Cache API、KV、D1、R2、Durable Objects
和 Queues 的真实 Cloudflare differential 通过并完成精确清理。最终本地 workspace 为 802/802 cases。

Wrangler OAuth 返回 `10000`，因此不声明真实 Cloudflare Workflow 托管端一致性。
