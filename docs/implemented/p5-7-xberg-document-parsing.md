# P5.7：PDF／Office 文档解析

状态：**implemented（2026-09-02）**。本地核心完成；跨平台、完整 parser fault/soak 和 hosted rich-document differential 见
[P5 资格](../acceptance/p5-release-acceptance.md)。

## 用户结果

- `env.AI.toMarkdown()` direct／handle overload 和 AI Search Items API 使用同一解析链路。
- Xberg 运行在 `ocd` 自派生的短生命周期 child 中；parser panic、abort、timeout 或 malformed frame 不进入主服务进程。
- Tenant 只看到 Cloudflare API；Xberg、OCDP frame、临时路径、S3 key 和 parser metadata 均为内部实现。
- Admission 同时检查扩展名、MIME 和 magic，并限制输入、输出、解压、XML、表格、页数、并发、CPU 和时长。
- Parser 完成后才把 normalized Markdown 交给 P5 durable chunk／embed／index generation；旧 claim 或半个输出不能激活。
- 生产构建和运行不依赖 Python、Java、LibreOffice、OCR 模型或网络下载。

支持的 13 类扩展为 TXT、Markdown、HTML、XML、JSON、CSV、PDF、DOCX、XLS、XLSX、XLSM、ODT 和 ODS。
XLSB 存在依赖 abort 风险，Numbers 在固定样本失败，ET 缺少合格 fixture，因此三者 fail closed；扫描 PDF 返回 OCR-required。

## 固定输入与证据

- Xberg `=1.0.14`，features 为 `tokio-runtime,pdf,office,excel,xml`。
- 固定公开 corpus 40 files，hostile corpus 15 cases；manifest SHA-256 分别为
  `599efa6fb8d5ae4517c1a62034bf4db69af7c152b77421709be5362882be31c1` 和
  `c02d5091a29c5e411181593074e7b707ceb2126ae68d304b5bab1a8b9b65d542`。
- Parser、stock-workerd API、AI Search indexing/retrieval、restart 和 P5 contract Gate 均在 P5 本地验收中通过。

当前 macOS parser child 没有可强制执行的 RSS hard limit；CPU、输入/输出、并发、timeout 和 child cleanup 仍生效。
OCR、PDFium fallback、parser pool/dedup、archive recursion 和 Cloudflare Markdown REST adapter 未实现。
