# W1：原生 Dynamic Worker Loader

状态：**verified（2026-09-06）**。源码位置和 fork 身份见 [workerd 索引](../workerd/README.md)；
逐项实现与兼容边界见 [Worker Loader](w1-dynamic-workers-worker-loader.md) 和
[兼容审查](w1-worker-loader-compatibility-review.md)。

## 结果与边界

- 保持一个 `ocd` 和一个受监督、正式固定的 workerd child；普通 Worker 通过 system Loader 原生装载
  Dynamic Worker，没有增加 JavaScript Loader、源码重写器或 stock/fork 双运行时。
- workerd fork 实现独立 namespace、受约束 capability delegation、原生 `load/get/WorkerStub`、
  entrypoint/RPC、Dynamic Durable Object facets、distinct in-flight 计数、缓存与撤销生命周期。
- open-compute 继续拥有账号、Script、Version、SQLite、路由和资源协议；workerd 拥有 isolate、capability、
  invocation lifecycle 与 private facets。租户不能获得内部 key、token、拓扑或宽权限网络。
- namespace 由 account、不可复用 Script UUID 和 binding name 派生。Version 升级与回滚共享 namespace，
  Script 删除排空后撤销；重建同名 Script 获得新身份。
- 正式 fork revision 为 `b3e1a27840299f493d9425dc4d9972381d02ef23`，upstream base 为
  `dd8133e9b9656fb39f1434247a80aa7a249ee204`。四平台优化二进制由 Git LFS 和唯一 formal lock 固定。

## 验证与限制

- 四平台原生 delegation、tails、limits 参数、facets、日期和 GC 回归通过。
- macOS arm64 上 build、generated check、format、Clippy、no-default-features、MSRV、metadata、dependency
  boundaries、coverage 与最终单轮 workspace Gate 通过；Gate 为 49 targets、1,148/1,148 cases，Rust 行覆盖率
  为 109,949/122,031（90.10%）。
- 显式 custom limits（包括 `{}`）仍拒绝。CPU、内存、subrequest 和连接预算的真实执行属于活动方案
  [W2](../workerd/w2-standard-limits.md)，W1 的接口存在不能作为 limits 已支持的证据。
- Linux 特权 egress、四平台 `ocd` package、正式发布和远端 Git/LFS push 未执行。
