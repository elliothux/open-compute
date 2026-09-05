# 概念

AI Search 区分：

- **Namespace** — instance 目录（父资源；不单独建库）
- **Instance** — 一份 SQLite authority，外加配置的 Local 或 S3 后端上的不可变源对象
- **`env.AI`** — 仅用于 Markdown Conversion 的平台 binding（`toMarkdown` / `supported` / `aiGatewayLogId`）

索引流水线：上传 → 解析（文档 parser 子进程）→ chunk → embed（operator provider）→ FTS 与向量 generation。检索模式：关键词、向量、混合。Chat 在检索后可选调用 generation provider；配置齐全时可 SSE 流式输出。

Provider 由 operator 固定（模型 revision、维度、tokenizer digest、超时）。open-compute 不内嵌模型权重，也不声称 Cloudflare 托管模型可用。

不提供：

- 完整 Workers AI 推理（`run`、`models`、AutoRAG、其它无关 `env.AI` 成员）
- Cloudflare 托管的自动 provider 选择、计费、Dashboard 连接器
- 全球索引就近存放 / 复制

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| 数据路径 | 托管 AI Search | 每 instance 一份 SQLite + Local/S3 对象 |
| 模型 | Workers AI | operator 的 OpenAI-compatible 端点 |
| Markdown Conversion | `env.AI.toMarkdown` | 同一子集 |
| 完整 Workers AI | 提供 | 不提供 |
