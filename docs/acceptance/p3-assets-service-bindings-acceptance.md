# P3 Assets 与 Service Binding 远端资格

状态：active，2026-09-01。P3.1/P3.2 核心实现与本地 Gate 见
[Static Assets](../implemented/p3-1-static-assets.md)和
[Service Binding](../implemented/p3-2-service-bindings.md)。本文只追踪 Cloudflare 托管端 direct differential。

## 剩余 Gate

- [ ] Static Assets：以同一 portable fixture 比较 Assets-only、Worker + Assets、默认路由、
  `run_worker_first`、显式 binding、GET/HEAD、ETag/304、HTML handling、404/SPA、
  `_headers`、`_redirects` 和可移植失败。
- [ ] Service Binding：比较默认/命名 `fetch()` 与 RPC、参数、返回、异常、stream、self/A→B 调用及目标部署切换。
- [ ] 冻结账号 alias、Wrangler、workers-sdk、workerd lock、compatibility date、fixture 和源码 identity。
- [ ] 使用唯一资源名，创建前确认 absent，按精确 name/ID 删除并复查 absent。
- [ ] declared-supported contract 不得存在未登记差异；无托管观察面的本地生命周期保证明确排除。

Cloudflare 部署和删除需当次授权。两端必须运行同一公开输入；只可归一化已登记的 provider identity、
时间或拓扑字段。完成后将结果并入 P3.1/P3.2 implemented 文档并删除本文；发现实现缺口时恢复活动方案。
