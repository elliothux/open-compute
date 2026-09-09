# W1 Worker Loader 兼容审查

状态：**W1 声明子集 verified；W2 limits 仍未实现（2026-09-06）**。

## 结论

- Public Loader 类型直接来自 `@cloudflare/workers-types@5.20260830.1`；Generated Env 只组合已声明 binding。
- `load/get`、七类 modules、env、scoped outbound、user tails、namespace、缓存与撤销使用 fork 的原生能力。
- Dynamic Durable Object facet 通过 host-only factory 创建可撤销能力；tenant 不能委派、持久化或读取内部创建 authority。
- Worker cache identity 基于 immutable Version 与 entrypoint；每次调用独立注入 collector，route generation 只用于授权和 fence。
- Python、Wasm、RPC、dynamic facets、rollback、restart、删除与同名 class 隔离通过正式 pin 产品路径。
- 当前认证日期 `2026-09-08` 使用的 Pyodide `314.0.6_2026-08-17_2` bundle 与 workerd 共用正式
  lock；gzip 内嵌于单文件 `ocd`，解压字节经双摘要校验后从 data-dir 私有 cache 加载。其它官方
  child 日期/flag 组合继续由 workerd 原生版本选择处理，不属于这个单 bundle 的离线资格。
- 显式 custom limits（包括 `{}`）继续拒绝；CPU、内存与 subrequest enforcement 属于 [W2](../workerd/w2-standard-limits.md)。
- Streaming tails 和 Cloudflare fleet placement 不在 W1 支持范围。

正式 fork revision 为 `b3e1a27840299f493d9425dc4d9972381d02ef23`，upstream base 为
`dd8133e9b9656fb39f1434247a80aa7a249ee204`。四平台优化 binary 已构建并由 formal lock／Git LFS 固定；
macOS arm64 是本次完整 open-compute acceptance host，其他平台只证明 native workerd tests。

## 历史验证

- 四平台 native delegation／tails／limits／facets tests 与严格退出通过。
- Build、generated check、format、Clippy、no-default-features、MSRV、metadata 和 dependency boundaries 通过。
- Coverage 和最终单轮 workspace Gate 均为 49 targets／1,148 cases；coverage 为 109,949 / 122,031 Rust lines（90.10%）。
- 同源 portable fixture 的 Cloudflare 临时资源已精确删除；Linux 特权 egress、四平台 `ocd` 发行和远端 Git/LFS push 未执行。

当前 authority 是 [formal pin](../../packages/runtime/workerd.lock.json)、[兼容矩阵](../references/cloudflare-compatibility.md)和活动 W2 设计。
