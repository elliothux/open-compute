# Operator Dashboard

Dashboard 核心实现已完成。2026-09-03 的原始验证、截图与限制见[完成记录](operator-api-dashboard-results.md)。
管理协议已统一为 [P6 `/client/v4` 合同](p6-cloudflare-v4-wrangler-compatibility.md)，本页只保留界面和集成职责。

## 界面与调用边界

- Dashboard 是可选管理界面，由 `ocd` 对外提供访问入口；公开业务 listener 仍由 `ocd` 独占。
- 页面通过固定官方 Cloudflare SDK 与 open-compute extension 操作平台已有资源，不维护独立资源 authority。
- 认证、账号 scope、资源授权、secret 写入和审计在服务端执行；前端筛选或隐藏按钮不能替代授权。
- 列表使用服务端 filter／sort／pagination，详情与 mutation 使用同一资源身份；失效状态和错误应可见。
- Worker、资源、部署与运维页面围绕实际支持合同展示，不向用户暴露内部 token、物理路径或实现协议。
- 前端保持 Kumo、响应式布局、键盘／对话框交互以及端到端行为覆盖。

## 源码与维护

- [Dashboard 源码](../../packages/dashboard/)
- [Cloudflare extension](../../packages/cloudflare-extension/)
- [管理合同与资源范围](p6-cloudflare-v4-wrangler-compatibility.md)
- [当前兼容矩阵](../references/cloudflare-compatibility.md)

新增产品页面沿用已有服务与 API，不另建客户端协议；测试与实际验收分别按[测试手册](../references/testing.md)和完成记录执行。
