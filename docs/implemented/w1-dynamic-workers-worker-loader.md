# W1：Dynamic Workers / Worker Loader

状态：implemented and verified，2026-09-06；W2 custom resource limits 于 2026-09-14 完成。

## 结果与边界

- 普通 Worker 获得原生 `WorkerLoader` binding，支持 `load()`、`get()`、同步 `WorkerStub`、
  entrypoint/RPC 和动态 Durable Object facets；七类模块仍由 workerd 编译和验证。
- Wrangler binding 进入 closed v4 schema、immutable Version descriptor、SQLite authority 和 RuntimeSource；
  不存在第二套 JavaScript Loader、源码重写器或 stock/fork 运行时分支。
- namespace 由 account、不可复用 Script UUID 和 binding name 派生。Version 升级/回滚共享 namespace，
  Script 删除排空后撤销历史 Version；重建同名 Script 获得新身份。
- workerd 拥有 capability、引用计数、invocation lifecycle、tails 和 private facets；open-compute 拥有账号、
  Script、SQLite、路由、日志策略和资源协议。租户不能获得 loader key、内部 token、拓扑或宽权限网络。
- namespace 上限 1024，每个 named cache 64 项；同一 caller 最多 4 个 Worker、10 个 DO distinct child。
  活跃引用不可淘汰，撤销拒绝新调用，已接纳调用排空。
- 源码位于 `third_party/workerd/`，fork revision
  `b3e1a27840299f493d9425dc4d9972381d02ef23`，upstream base
  `dd8133e9b9656fb39f1434247a80aa7a249ee204`。四平台优化二进制由 Git LFS 和唯一
  `packages/runtime/workerd.lock.json` 固定；构建校验并离线内嵌，不下载或回退 stock。

## 已知限制

- custom limits（含 `{}`）在 W1 阶段拒绝；当前行为由已完成的
  [W2](w2-standard-limits.md) 单一路径实现。25 个 Loader members 中 23 个已有产品证据；2 个
  experimental-control members 继续 fail closed，非空 streaming tails 不开放。
- W2 按官方合同固定执行 1 秒 startup CPU limit。Dynamic Python child 的本地 Pyodide cold boot 不能
  稳定满足该限额，而本项目没有 Cloudflare hosted deploy-time Python 预计算，因此不再宣称这条 Loader
  变体已资格化；它保持 fail closed，直到存在等价的部署期预计算路径。Wasm、JavaScript、RPC 与 facets
  不受此限制。
- Anycast、全球 placement、跨地域复制和 fleet autoscaling 不在单机产品范围。

逐项兼容边界见[兼容审查](w1-worker-loader-compatibility-review.md)。

## 验证

固定输入为上述 fork、formal lock SHA-256
`de6a6f64a26f9e5640a8f2ceb2a6de5ab93968e1948ee8c36b35140b121e2de7`。

- 四平台 workerd 构建与 formal lock / Git LFS OID 一致；原生 delegation、tails、limits、facets、
  日期和 GC 回归通过。
- build、generated、fmt、Clippy、no-default-features、Rust 1.98、metadata、dependency boundaries、
  239 项 JS、25 项 Gate tooling 和文档构建通过。
- coverage：49 targets、1,148/1,148 cases，109,949/122,031 lines，90.10%。
- 最终非插桩 Gate：49 targets、1,148/1,148 cases，单轮通过。

Linux 特权 egress、四平台 `ocd` package、正式发布和远端 Git/LFS 推送未执行。
