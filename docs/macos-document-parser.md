# macOS 解析子进程内存硬限制 TODO

状态：待实现；2026-09-05 明确接受为 0.1.0 发行限制，不阻断完整文档解析功能。

## 当前行为与边界

Linux 和 macOS 均保留 TXT、Markdown、HTML、XML、JSON、CSV、文本层 PDF、DOCX、XLSX、XLSM、XLS、ODT 和 ODS。
`AI.toMarkdown()`、handle `transform()`、`supported()` 和 AI Search 文档索引不缩减格式集合。

解析器由 `ocd` 按需启动为短命子进程，执行同一个二进制的私有 `__document-parser-v1` 入口。
它不是另行分发或常驻部署的 sidecar；生产发行仍只有一个 `ocd` 可执行文件。
解析进程有独立地址空间，不属于 workerd Worker isolate 的资源额度，但仍消耗宿主内存。

Linux 使用 `RLIMIT_AS` 强制限制解析进程地址空间。Darwin 拒绝当前 `RLIMIT_AS` 设置；
macOS 的 `document_parser.max_address_space_bytes` 因此不提供强制内存上限，也尚无等价 RSS 硬限制。
地址空间上限与 RSS 上限并非同一个度量，不能把 Linux 结果外推到 macOS。

现有输入、输出、容器展开、批次、并发、CPU、wall-clock timeout、进程组终止与回收限制继续生效。
这些约束降低资源消耗风险，但不保证恶意或异常文档不会造成宿主内存压力，也不保证宿主 OOM 时主服务不受影响。
本次明确接受的是 macOS 解析子进程缺少内存硬上限；不是取消其余限制，也不是宣称内存隔离已经验证。

## 后续工作

- [ ] 实现可执行的 macOS parser 内存硬限制，明确 RSS/地址空间度量与超限行为。
- [ ] 在 arm64/x64 正式二进制上验证正常解析、超限终止、CPU、超时、进程组与孤儿清理。
- [ ] 保留超限后的主服务可用性与 AI Search 失败/恢复证据，再关闭本 TODO。

历史 parser corpus 和运行报告保持原样；它们证明对应解析路径，不能证明尚未实现的内存硬限制。
