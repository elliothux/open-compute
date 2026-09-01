# Cloudflare Workflow 远端 differential 剩余验收

状态：Active，2026-09-01。本文只追踪 Cloudflare 托管端 Workflow portable fixture 的外部资格，
不重新打开已完成的 Day1 runtime、binding、persistence、catalog 或本地 Gate 实现。

## 已完成前提

- `workflows/portable/lifecycle` 与其它 portable fixtures 共用 typed runner、唯一资源前缀、精确清理和
  absent 复查；open-compute 端真实 `platformd`、stock workerd、SQLite authority 和恢复路径已实现。
- capability/catalog 的 72 个 Workflow stable members 全部为 `supported_with_deviation`，目标 inventory
  的 `blocked=0`；`OC-WORKFLOW-001` 只描述单机执行拓扑，不掩盖功能缺口。
- Workers、Cache API、KV、D1、R2、Durable Objects 和 Queues 的同源 portable fixtures 已与真实
  Cloudflare 逐字段一致。

## 当前阻塞

当前 Wrangler OAuth 对 Cloudflare Workflow inventory API 返回
`Authentication error [code: 10000]`。失败发生在 runner 的只读 preflight；未创建 Workflow、Worker 或
其它 Cloudflare 资源。源码冻结后的七项合并复查又在 D1 inventory preflight 收到同一错误；该次运行只
完成并精确清理了 Cache API Worker，D1 及后续资源均未创建。此前分项成功的 Workers、KV、D1、R2、
Queues 等证据不等于当前 token 仍可访问这些 endpoint，也不能替代 Workflow endpoint 的实际授权。

刷新 Wrangler OAuth 或换用具有 Workflow API 权限的 credential 会修改外部账号凭据，需单独授权后再做。
在此之前，文档只能声明 Workflow 本地 contract 已完成，不能声明它已通过真实 Cloudflare differential。

## 完成条件

1. 获得明确授权后，用当前 pinned Wrangler 和账号 alias 只选择
   `workflows/portable/lifecycle` 执行 preflight。
2. runner 随机创建唯一 `oc-p34-*` Worker 与 Workflow，比较同一 source 的公开 status/JSON；除
   `OC-WORKFLOW-001` 允许的拓扑字段外不得归一化行为差异。
3. 按精确 name/ID 删除两端资源并复查 Worker 与 Workflow inventory absent；不得触碰账号中已有服务。
4. 记录 source digest、Wrangler version、Cloudflare account alias、报告路径和清理结果，随后将本文归档。

若该外部运行暴露实现差异，先回到单轮 focused development Gate；修复并冻结源码后重新执行受影响的
本地最终验收。若只是 credential/Cloudflare endpoint 不可用，保留本计划，不重复无意义的本地 Gate。
