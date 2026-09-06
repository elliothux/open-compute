# 行为差异

AI Search 与 Markdown Conversion 运行在单机上，模型端点由 operator 持有。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| AI Search API | Namespace / instance / items / jobs / search / chat | 已声明表面相同 |
| 模型 | 托管 Workers AI | operator 配置的 OpenAI-compatible provider |
| 对象 | 托管存储 | operator 选定的 Local 或 S3 authority |
| Markdown Conversion | `env.AI.toMarkdown` | 同一子集；本机 tokenizer 估算 |
| 完整 Workers AI / AutoRAG | 提供 | 不提供 |
| 全球就近存放 | 提供 | 不提供 |

见[兼容性](/zh/platform/compatibility)、[不支持](/zh/platform/unsupported)与 [Cloudflare AI Search](https://developers.cloudflare.com/ai-search/)。
