# P3.1：Static Assets

状态：**implemented（2026-08-29）**。Day 1 核心和本地 Gate 完成；Cloudflare direct differential 见
[P3 资格](../acceptance/p3-assets-service-bindings-acceptance.md)。

## 用户结果

- Worker-only、Worker + Assets 和 Assets-only 共用不可变 deployment；Assets-only 不生成伪 Worker。
- 已构建文件经扫描、manifest、可恢复上传和完整性校验进入平台 object authority。
- 支持默认 HTTP 路由和显式 Assets binding，包含 GET／HEAD、MIME、ETag／304、HTML handling、404／SPA、`_headers` 和 `_redirects`。
- `run_worker_first` 与路径规则决定 Worker-first／Assets-first；一次请求固定同一 deployment 的代码、配置和资源。
- Ready、promote、rollback、delete、GC、snapshot 和 restore 使用持久引用保护资源，进程或上传中断不会发布半成品。
- 读取复用有界 verified cache；丢失或损坏的已引用对象是系统错误，不伪装成普通 404。

Static Assets 不增加静态服务器、Node SSR、Redis、第二套 S3 配置或框架专用路径。当前精确支持面见
[Cloudflare 兼容矩阵](../references/cloudflare-compatibility.md)。

## 历史验证与限制

2026-08-29 的 build、59 个 JS 测试、静态检查、`p3-assets` real-runtime Gate 和 workspace 验收成功；
coverage 为 90.11%。后续 vinext workload 证明选定应用的 Assets/browser 路径，但不替代完整 Assets direct differential。

未声明 Cloudflare 全球 CDN、计费、Pages Functions、Range、预压缩变体或图片转换。
