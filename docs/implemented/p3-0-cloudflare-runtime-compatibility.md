# P3.0：Cloudflare Runtime 兼容实现

状态：**implemented / hosted Conditional Go（2026-09-01）**。Workflow 托管端资格见
[剩余验收](../acceptance/p3-0-cloudflare-runtime-compatibility-acceptance.md)。

## 最终结果

- Stable Worker API 直接来自固定 `@cloudflare/workers-types`；不维护自有替代声明。
- [`workerd.lock.json`](../../packages/runtime/workerd.lock.json) 固定 runtime、types、compatibility date／flags 和构建输入。
- 工具链、descriptor 和 loader 使用同一 single-latest contract，不提供 tenant-selectable 历史 runtime 分支。
- Generated `Env` 只组合部署声明的 binding；类型存在不等于 capability 已授予。
- `fetch()`、`cloudflare:sockets.connect()`、`node:net` 和 `node:tls` 共用 public-only outbound；Service／DO connect 只走声明的 capability tunnel。
- KV、R2、D1、Durable Objects、Queues、Cron、Workflows、Cache、Version Metadata 和 WebSocket hibernation 均映射到各自持久 authority。
- Capability、case 和 deviation 的映射由 [`share/cloudflare-capabilities.json`](../../share/cloudflare-capabilities.json) 与
  [`test/conformance/catalog.json`](../../test/conformance/catalog.json) 持有。

当前支持面只在[兼容矩阵](../references/cloudflare-compatibility.md)和[能力偏差](../references/p1-deviations.md)维护；
管理面合同独立见 [P6](p6-cloudflare-v4-wrangler-compatibility.md)。

## 历史验证与限制

固定输入为 workerd `v1.20260830.1`、workers-types `5.20260830.1`。当日 inventory 为 2,097 members：
1,585 `supported`、512 `supported_with_deviation`、`blocked=0`。Build、193 个 JS tests、静态检查和最终单轮 workspace
Gate 40 targets／802 cases 成功；Rust line coverage 为 68,383 / 75,839（90.17%）。

Workers、Cache API、KV、D1、R2、Durable Objects 和 Queues 的 Cloudflare differential 通过并精确清理。
Workflow 在只读 preflight 遇到 OAuth code `10000`，因此整体 hosted verdict 保持 Conditional Go；未执行正式发行或跨平台资格。
