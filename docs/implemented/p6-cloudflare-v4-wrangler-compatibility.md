# P6：Cloudflare v4 API 与 Wrangler

状态：**implemented（2026-09-03）**。本地核心和 scoped Gate 完成；完整 hosted differential 见
[P6 资格](../acceptance/p6-cloudflare-v4-differential-acceptance.md)。

## 用户结果

- `/client/v4` 是唯一在线管理 API；`wrangler.jsonc` 是唯一 Worker 项目配置，上游 Wrangler 是标准部署客户端。
- Dashboard、官方 Cloudflare SDK、automation 和 Lynx broker 调用同一 v4 transport。
- Cloudflare 已有资源使用官方 method/path/envelope/pagination/error；open-compute 特有能力只位于同一 transport 的 vendor namespace。
- 旧 `/operator/api/v1`、`open-compute.json`、Operator SDK 和自定义 upload transport 已删除，不保留 alias、redirect 或双写。
- Workers Scripts／Versions／Deployments／Settings／Secrets、Static Assets、KV、D1、R2、Vectorize、AI Search、Queues 和 Workflows 使用现有 domain authority。
- Compatibility date、flags、modules 和 bindings 在 immutable Worker Version 中冻结；未知或不支持字段 fail closed。
- Wrangler multipart、official SDK typed upload 和 Assets direct upload 汇入同一个有界 admission path。
- Vendor extension 复用官方 SDK transport，不拥有第二套 auth、retry、pagination 或 raw-fetch client。

机器可读 OpenAPI subset 和 capability manifest 是 route／wire 支持面的 authority；当前事实见
[兼容矩阵](../references/cloudflare-compatibility.md)。

## 固定客户端与偏差

本轮固定 Wrangler `4.127.1`、Cloudflare TypeScript SDK `7.1.0`、OpenAPI revision
`b8687f42e28fbfcb296a350f7dbf16349ea900af` 和 workerd `v1.20260830.1`。

- Account subdomain 只为固定 Wrangler Workflow prerequisite 返回不可路由 label，不创建 DNS 或 route。
- D1 Time Travel 只使用显式、最多 8 个 retained checkpoints，不声明 Cloudflare always-on PITR。
- AI Search token list 只返回 installation-managed、无 secret 的 metadata；token mutation 不支持。
- Service Binding `props` 只接受最大 64 KiB、深度 32 的 canonical JSON object。
- 固定 Wrangler 的 Queue `delivery_delay` 按其 deprecated/no-effect 行为接受但不持久化。
- SDK multipart 的归一化只覆盖固定版本可唯一解释的 closed shape。

这些差异只让单机 SMB 的公开客户端主路径可用，不允许 secret 泄漏、损坏状态修复或 unsupported capability 假成功。

## 历史验证与限制

Build、generated contracts、format、no-default-features、MSRV、metadata、dependency boundaries 和 P6 scoped real-runtime Gate 成功；
最终受影响集合为 15/15 cases。`cf-compatibility-check` 复核无剩余 in-scope finding。

当次 canonical Clippy 仍有 99 个既有 service diagnostics，workspace Gate、coverage 和 hosted differential 未在 P6 冻结点闭环，
因此不把 scoped PASS 写成完整仓库或 hosted PASS。后续阶段的历史 workspace 通过不改写这次证据边界。
