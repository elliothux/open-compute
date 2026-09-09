# P4：Next.js／vinext 应用资格

状态：**verified / Application Go（2026-09-01）**。

## 用户结果

固定 vinext `1.0.0-beta.8`、Next.js `16.2.7` workload 的同一 production artifact 已在 open-compute 和真实
Cloudflare Workers 上完成 HTTP、Chromium、SSR／RSC、Server Action、Assets、binding 和隔离对照。

- Wrangler 与 framework importer 消费同一冻结 output tree，不重新构建或重写 modules。
- Importer 保留 module 相对路径和类型，验证 generated config、binding、class 与 entrypoint，但不把 provider ID 当成本地 authority。
- open-compute 使用不可变 deployment；Cloudflare 使用一个 Worker Version 和一个 100% Deployment。
- Staged upload 的通用 body ceiling 修复只作用于精确 route，endpoint 自身限制和完整性校验不变。
- 两端资源均按本轮精确身份清理并复查不存在。

机器可读输入和 verdict 位于 [`vinext.json`](../../test/conformance/applications/vinext.json) 与
[`vinext-cases.json`](../../test/conformance/applications/vinext-cases.json)。

## 历史验证

- 20/20 selected mandatory cases 通过；两端 application runner 各 15/15，0 optional，14 excluded。
- Wrangler 与 importer 都发现 79 个相同 module names；inventory SHA-256 为
  `8311c2918f094d6bbd435c9db94d4ced97d8da8c710e008d6dd925214a3e29d1`。
- Build、197 个 JS tests、静态检查和 coverage 成功；coverage 为 68,401 / 75,859 Rust lines（90.17%）。
- 当日 workspace 历史运行共 894/894 case executions；当前 Gate 轮数以[测试手册](../references/testing.md)为准。

结论只覆盖这份固定 workload。ISR、Cache/Images、产品 binding 组合、promotion/rollback、restart、双账户产品隔离、
vinext/Next.js 全 API 和跨平台发行均不在 P4 verdict 中。
