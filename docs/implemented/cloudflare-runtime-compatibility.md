# Cloudflare Runtime 兼容实现

全量兼容改造于 2026-09-01 完成；当时的成员统计、固定输入、实际检查和 hosted differential 见[完成报告](cloudflare-runtime-compatibility-results.md)。
当前支持与未实现能力只维护在[兼容矩阵](../references/cloudflare-compatibility.md)和[能力偏差](../references/p1-deviations.md)。

## 类型与运行时合同

- 稳定 Worker API 声明来自固定 `@cloudflare/workers-types`，由 [`packages/types/index.d.ts`](../../packages/types/index.d.ts) 直接消费；不手写替代上游接口。
- [`packages/runtime/workerd.lock.json`](../../packages/runtime/workerd.lock.json) 固定 runtime、types 和相关构建输入；声明字节／AST、版本及生成资产由 conformance 校验。
- 租户执行采用平台固定的 single-latest date／flags；工具链、descriptor 与 loader 使用同一语义，不提供历史内部运行时分支。
- Generated `Env` 只组合声明的 binding；类型包存在某 API 不表示平台已授予对应 capability。
- Standalone workerd 的接口存在不等于 Cloudflare 托管实现存在；native limits／Loader 的后续开发见[活动方案](../workerd/README.md)。

## 支持范围与证据

平台以 Workers runtime 和声明的产品 binding 为目标；单机部署接受的拓扑差异必须有明确 deviation。
管理面合同独立见 [P6](p6-cloudflare-v4-wrangler-compatibility.md)，不能从 Worker API 对齐推断管理 API 全量对齐。

[`share/cloudflare-capabilities.json`](../../share/cloudflare-capabilities.json) 与
[`test/conformance/catalog.json`](../../test/conformance/catalog.json) 关联成员、状态、case 和 deviation。
目标内未实现成员必须明确标记，不能用非目标或空实现掩盖；保留成功、错误、权限、生命周期和恢复行为证据。

具体 fixture、证据结构和平台／应用 verdict 见 [P3.4 conformance](p3-4-cloudflare-conformance.md)。
待取得的外部资格见[验收索引](../acceptance/README.md)，当前验证命令见[测试手册](../references/testing.md)。
