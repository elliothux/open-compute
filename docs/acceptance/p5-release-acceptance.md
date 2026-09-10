# P5 剩余发行验收

状态：active，2026-09-02。核心实现与本地证据见
[Vectorize / AI Search](../implemented/p5-vectorize-ai-search.md)和
[P5.1 Xberg 文档解析](../implemented/p5-1-xberg-document-parsing.md)、
[P5.2 AI provider backend/profile](../implemented/p5-2-ai-provider-profiles.md)。

## 剩余 Gate

- [ ] 用 `crates/search/examples/exact_search_benchmark.rs` 保留 10k–250k vectors、
  384–1536 dimensions、1%–100% selectivity、concurrency 1/4/16 的 release-mode p95、RSS 和 quota 结论。
- [ ] 验证 Linux/macOS x64/arm64 的 Rust 1.98 release build、正式 package、单文件 size/hash 和隔离离线启动。
- [ ] 补齐 parser child panic/abort/OOM/signal/orphan cleanup/crash recovery/soak 及各平台资源限制。
- [ ] 用专属临时资源完成 Cloudflare Markdown Conversion rich-document output/error/limit differential。
- [ ] 核对 release artifact 的 license/NOTICE、签名、发布和受权限约束的外部资格。

macOS parser 无内存硬上限是 0.1.0 已接受限制，继续由
[后续 TODO](../p5-8-macos-document-parser.md)追踪，不阻断本次发行；其他限制仍需验证。
缺少平台、权限或固定输入时保持未验证，不用 mock 代替。完成后将实际结果并入对应 P5 implemented 文档并删除本文。
