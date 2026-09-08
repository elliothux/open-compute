# W1：Dynamic Workers / Worker Loader

状态：implemented and verified，2026-09-06。W1 声明子集完成；custom resource limits 属于 W2，
完整 Dynamic Workers 仍有 6 个 blocked members。本次未发布或推送远端资源。

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

- custom limits（含 `{}`）在 W1 拒绝；CPU、内存和 subrequest enforcement 由
  [W2](../workerd/w2-standard-limits.md)实现。
- 25 个 stable Loader members 中 19 个有产品证据，4 个 custom-limit 和 2 个 experimental-control
  members 保持 blocked；非空 streaming tails 拒绝。
- Python child 首次加载可能由 workerd 下载固定 Pyodide bundle，因此只保证 `ocd` 启动离线。
- Anycast、全球 placement、跨地域复制和 fleet autoscaling 不在单机产品范围。

逐项兼容边界见[兼容审查](w1-worker-loader-compatibility-review.md)。

## 验证

固定输入为上述 fork、formal lock SHA-256
`5f92b595764892c166b36e61a81ef0b5313178554edefbb0b49a01dad7efb303`。

- 四平台 workerd 构建与 formal lock / Git LFS OID 一致；原生 delegation、tails、limits、facets、
  日期和 GC 回归通过。
- build、generated、fmt、Clippy、no-default-features、Rust 1.98、metadata、dependency boundaries、
  239 项 JS、25 项 Gate tooling 和文档构建通过。
- coverage：49 targets、1,148/1,148 cases，109,949/122,031 lines，90.10%。
- 最终非插桩 Gate：49 targets、1,148/1,148 cases，单轮通过。

Linux 特权 egress、四平台 `ocd` package、正式发布和远端 Git/LFS 推送未执行。
