# P5.1：PDF／Office 文档解析

状态：**implemented（2026-09-02），由 P5.3 直接演进**。

P5.1 建立了 `env.AI.toMarkdown()` 与 AI Search 共用的短生命周期 parser child、OCDP frame、
extension/MIME/magic admission、normalized Markdown，以及输入、输出、container、CPU、deadline 和 process-group
边界。它还固定了 parser 完成后才进入 durable chunk/embed/index generation 的事务边界。

当前格式、Xberg、OCR、图片和 VLM 合同已由
[P5.3](p5-3-document-formats-ocr-vlm.md)直接替换；没有保留 Xberg 1.0、13-format 列表、OCR-required 分支或旧
parser contract。当前支持面以[兼容矩阵](../references/cloudflare-compatibility.md)和源码 registry 为准。

macOS parser child 的 RSS hard limit 仍由[后续 TODO](../p5-8-macos-document-parser.md)追踪；其它发行资格见
[P5 剩余验收](../acceptance/p5-release-acceptance.md)。
