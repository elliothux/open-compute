# P10 Worker Loader 可行性复核

日期：2026-09-05。结论：**当时 No-Go；未实现。** 这是固定 stock workerd 的一次性调查；
后续 fork 实现见 [W1 Worker Loader](w1-dynamic-workers-worker-loader.md)。

## 固定输入

仓库 revision `2c36a0d52108bbf1f85f58f3fa305180057d5b71`，workerd
`v1.20260830.1` / `e9dda5963aba7ee4323960db795690ec78fec118`，compatibility date
`2026-08-30`，Wrangler `4.127.1`。使用已存在且与 formal pin 匹配的 darwin-arm64 binary，
未下载 runtime、启动网络 listener 或修改外部资源。

## 结果

| 行为 | 观察 | 判定 |
| --- | --- | --- |
| static native Loader | `load(code)` 返回同步 stub，fetch 成功 | 仅证明内部 primitive |
| Loader 转移 | child env 中传递 Loader 抛 `DataCloneError` | 普通 tenant 无法获得原生 Loader |
| code-level limit | `subRequests:1` 下完成 3 次 fetch | 接受但未执行限额 |
| entrypoint limit | `subRequests:1` 下完成 3 次 fetch | 接受但未执行限额 |
| named cache cleanup | 源码只有 abort erase | 无一般有界回收证据 |

固定源码没有 Loader capability serialization；`WorkerStubImpl` 未传递 limits，
standalone `LimitEnforcer::newSubrequest()` 为空实现。同步 Loader、有效 custom limits 和有界缓存生命周期
都是产品前置，因此当时不能用 JS facade、placeholder binding 或全进程重启冒充实现。

首次 probe 因 fixture 缺少 Options 退出 1；修正 fixture 后只执行未到达的限额观察并退出 0。
第二次 PASS 只证明“限额无效”被成功复现，不是产品 Gate。原始证据在
`.temp/p10-feasibility/failed/20260905T130604Z/` 与 `.temp/p10-feasibility/20260905T130632Z/`。
未运行 workspace Gate、coverage 或 Cloudflare hosted differential。
