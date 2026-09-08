# P3.3：Workers Cache、Cache API 与 Images

状态：**verified / Platform Go（2026-08-30）**。

## 用户结果

- Workers Cache 是配置驱动的 HTTP response cache；Cache API 提供 `caches.default`、named cache 和 `put/match/delete`。
- `caches.default` 与同一 Worker 的默认 response cache 共用逻辑存储，named cache 独立；普通 RPC 不自动缓存。
- Cache metadata 使用 per-Worker SQLite，body 使用平台 object authority；版本隔离、显式跨版本共享、purge 和 stale refresh 都有 generation fence。
- Images binding 在有界 Rust engine 中处理输入字节，支持当前声明的 decode／transform／encode surface；engine 不联网，输出不自动进入 Cache。
- Version Metadata、Cache、Images 都从冻结 deployment descriptor 注入，不存在框架名称或 vinext 特判。
- Worker 删除、rollback、restart、S3 故障、cache corruption 和 image session cleanup 复用当前持久化与生命周期边界。

当前精确 API、限额和 deviation 由[兼容矩阵](../references/cloudflare-compatibility.md)维护。

## 历史验证与限制

2026-08-30 的 build、78 个 JS/TS tests、`p3-cache-images`、静态检查和 workspace Gate 成功；coverage 为
90.10%，当日历史三轮报告的第一轮覆盖 37 targets。结果证明声明的单节点支持面，不代表全球 cache/CDN、Cloudflare Images
托管服务或任意第三方应用兼容。
