# W1 Worker Loader 兼容审查

状态：**W1 声明子集 verified（2026-09-06）；W2 limits verified（2026-09-14）**。

## 结论

- Public Loader 类型直接来自 `@cloudflare/workers-types@5.20260830.1`；Generated Env 只组合已声明 binding。
- `load/get`、七类 modules、env、scoped outbound、user tails、namespace、缓存与撤销使用 fork 的原生能力。
- Dynamic Durable Object facet 通过 host-only factory 创建可撤销能力；tenant 不能委派、持久化或读取内部创建 authority。
- Worker cache identity 基于 immutable Version 与 entrypoint；每次调用独立注入 collector，route generation 只用于授权和 fence。
- Wasm、RPC、dynamic facets、rollback、restart、删除与同名 class 隔离通过正式 pin 产品路径。
- W2 固定的官方 1 秒 startup CPU limit 使 Dynamic Python 的本地 Pyodide cold boot 无法稳定通过；
  open-compute 不仿制 Cloudflare hosted deploy-time Python 预计算，因此该 Loader 变体不在当前资格范围。
- 显式 custom limits（包括 `{}`）在 W1 阶段拒绝；当前合同由已完成的
  [W2](w2-standard-limits.md) 实现并资格化。
- Streaming tails 和 Cloudflare fleet placement 不在 W1 支持范围。

W1 验收使用的 fork revision 为 `b3e1a27840299f493d9425dc4d9972381d02ef23`，upstream base 为
`dd8133e9b9656fb39f1434247a80aa7a249ee204`。四平台优化 binary 已构建并由 formal lock／Git LFS 固定；
当前 W2 formal revision 为 `d711abf405f2d56b6518a863bb5dbcace14289f1`。

## 历史验证

- 四平台 native delegation／tails／limits／facets tests 与严格退出通过。
- Build、generated check、format、Clippy、no-default-features、MSRV、metadata 和 dependency boundaries 通过。
- Coverage 和最终单轮 workspace Gate 均为 49 targets／1,148 cases；coverage 为 109,949 / 122,031 Rust lines（90.10%）。
- 同源 portable fixture 的 Cloudflare 临时资源已精确删除；Linux 特权 egress、四平台 `ocd` 发行和远端 Git/LFS push 未执行。

当前 authority 是 [formal pin](../../packages/runtime/workerd.lock.json)、[兼容矩阵](../references/cloudflare-compatibility.md)和[W2 实施记录](w2-standard-limits.md)。
