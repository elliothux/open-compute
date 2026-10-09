# P6：Cloudflare v4 API 管理面

状态：**implemented（2026-09-03）**。

## 用户结果

- `/client/v4` 是唯一在线管理 API；当前项目使用 `cloudflare.config.ts`、官方 cf、Vite 插件 v2 与 Build Output，
  CLI/应用配置合同由 [P20](p20-cf-cli-migration.md) 拥有。
- Dashboard、官方 Cloudflare SDK、automation 和 Lynx broker 调用同一 v4 transport。
- Cloudflare 已有资源使用官方 method/path/envelope/pagination/error；open-compute 特有能力只位于同一 transport 的 vendor namespace。
- 旧 `/operator/api/v1`、`open-compute.json`、Operator SDK 和自定义 upload transport 已删除，不保留 alias、redirect 或双写。
- Workers Scripts／Versions／Deployments／Settings／Secrets、Static Assets、KV、D1、R2、Vectorize、AI Search、Queues 和 Workflows 使用现有 domain authority。
- Compatibility date、flags、modules 和 bindings 在 immutable Worker Version 中冻结；未知或不支持字段 fail closed。
- 当前 cf multipart、official SDK typed upload 和 Assets direct upload 汇入同一个有界 admission path。
- Vendor extension 复用官方 SDK transport，不拥有第二套 auth、retry、pagination 或 raw-fetch client。

机器可读 OpenAPI subset 和 capability manifest 是 route／wire 支持面的 authority；当前事实见
[兼容矩阵](../references/cloudflare-compatibility.md)。

## Browser Run 管理面扩展

[P22](p22-browser-run.md) 在同一 `/client/v4` 鉴权与实例路由中增加 Browser Run Quick Actions、
DevTools session/browser、CDP WebSocket 和 Live View；逐 route 保留 raw JSON、PNG/PDF 和 upgrade，
不使用通用 v4 envelope 包装这些响应。`browser` upload binding 归属 immutable Version authority，
未配置 backend 时拒绝。Dashboard、固定 cf 与 Cloudflare SDK 的真实路径和验收结果由 P22 记录。
[兼容矩阵](../references/cloudflare-compatibility.md#browser-run)公开官方来源冲突与未资格化范围；
机器合同中 Browser 的全量 stable members 仍 blocked，不把常见路径通过声明为完整协议支持。

## 历史固定客户端与偏差

2026-09-03 的验收固定 Wrangler `4.127.1`、Cloudflare TypeScript SDK `7.1.0`、OpenAPI revision
`b8687f42e28fbfcb296a350f7dbf16349ea900af` 和 workerd `v1.20260830.1`。这些版本与 prerequisite 记录只证明当时输入；当前 CLI pin、
route/wire 支持和偏差以 [兼容矩阵](../references/cloudflare-compatibility.md) 与机器可读合同为准。

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

当次 canonical Clippy 仍有 99 个既有 service diagnostics；后续阶段的历史 workspace 通过不改写这次证据边界。
