# P1：平台加固

状态：**implemented（2026-08-28）**。P1.0–P1.7 本地验证完成；长时 soak 和发行演练见
[P1 资格](../acceptance/p1-release-acceptance.md)。

## 最终结果

- Capability manifest 和维护中的兼容矩阵是支持范围 authority。
- 写入 admission、空间 reservation 和磁盘水位统一保护所有持久 mutation。
- 离线 snapshot 独占 data-dir，固定 release、schema、object authority 和 key fingerprint；restore 先验证，再向空目标发布。
- Master key 由 operator 独立保管；secret、错误、metrics 和 support bundle 不暴露敏感内容。
- 当前 schema 直接表达 Day 1 模型；已发布 migration 仍保持字节不变。
- 恶意输入、租户隔离、crash/restart、snapshot/restore 和 P0 回归均走真实生产路径。

当前恢复操作见 [`docs/references/runbooks/`](../references/runbooks/)，支持面见[兼容矩阵](../references/cloudflare-compatibility.md)。

## 历史验证与限制

在 Darwin arm64 和 workerd `v1.20260826.1` 上，workspace、real-runtime Gate、静态检查及 coverage 成功；coverage 为
43,685 / 48,521 Rust lines（90.03%）。10 分钟本地 mixed soak 成功，但 1 小时 developer soak、24 小时 RC soak、
release package 和 service rehearsal 未执行。

G0 的 `loader:D-abort` 限制继续接受；P1.8 hibernatable WebSocket 当时为 No-Go，不影响基础 WebSocket。
