# P6.1：Operator Dashboard

状态：**implemented（2026-09-03）**。

## 用户结果与边界

- Dashboard 是 `ocd` 提供的可选管理界面，覆盖登录、账号上下文、Workers 与已支持资源、平台状态和维护动作。
- 当前管理协议统一为 [P6 `/client/v4`](p6-cloudflare-v4-wrangler-compatibility.md) 和 open-compute extension；旧 `/operator/api/v1`、Operator SDK 与项目配置已删除。
- 页面通过官方 Cloudflare SDK 和 extension 调用现有 domain authority，不建立前端资源状态或第二套协议。
- 鉴权、账号 scope、secret、审计和 mutation 校验在服务端完成；UI 隐藏按钮不是权限边界。
- Dashboard 作为 system-owned immutable assets deployment 内嵌，不携带 admin token、SQLite／S3 路径或内部 capability。
- UI 使用 Kumo，保留响应式布局、键盘操作、pending 防重复、确认 Dialog 和真实 API E2E。

源码位于 [`apps/dashboard/`](../../apps/dashboard/)；当前支持面见[兼容矩阵](../references/cloudflare-compatibility.md)。

## 历史验证

2026-09-03 的本地实现 Gate 为 **Implementation GO**：Dashboard Playwright 31/31、live client contract 12/12、
instrumented 和最终未插桩 workspace Gate 均为 42 targets／835 cases，Rust line coverage 为 72,186 / 80,085（90.14%）。
这些结果早于 P6 管理协议替换，只证明当时 UI 与资源流程；当前协议行为以 P6 和源码为准。

未验证正式发行、跨平台或 Cloudflare billing／plan parity。
