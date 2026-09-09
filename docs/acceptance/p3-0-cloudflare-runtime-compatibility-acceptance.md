# P3.0：Cloudflare Workflow 远端 differential 验收

状态：blocked by Cloudflare credential，2026-09-01。

本地 Workflow contract、持久化、恢复和产品 Gate 已完成，见
[Runtime 兼容](../implemented/p3-0-cloudflare-runtime-compatibility.md)。本文只追踪真实 Cloudflare Workflow
portable fixture；其他产品曾通过不代表当前凭据仍可访问 Workflow endpoint。

## 阻塞

当前 Wrangler OAuth 在 Workflow inventory preflight 返回 `Authentication error [code: 10000]`。
失败发生在只读阶段，未创建资源。刷新 OAuth 或更换凭据会修改外部账号状态，需用户授权。

## 完成 Gate

- [ ] 用当前固定 Wrangler 和获授权凭据通过 Workflow inventory preflight。
- [ ] 以唯一 `oc-p34-*` 前缀创建 fixture Worker 与 Workflow，比较两端公开 status/JSON；
  仅允许已登记的 `OC-WORKFLOW-001` 拓扑差异。
- [ ] 按精确 name/ID 删除资源并复查 inventory absent。
- [ ] 记录 source digest、Wrangler version、账号 alias、报告和清理结果。

完成后把结果并入对应 implemented 文档并删除本文。若发现实现差异，恢复活动方案并修复；凭据仍不可用时不重复本地 Gate。
