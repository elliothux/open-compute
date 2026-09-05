# Behavior differences

AI Search and Markdown Conversion run on one node with operator-owned model endpoints.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| AI Search API | Namespace / instance / items / jobs / search / chat | Same declared surface |
| Models | Hosted Workers AI | Operator-configured OpenAI-compatible providers |
| Objects | Hosted storage | Local or S3 authority selected by the operator |
| Markdown Conversion | `env.AI.toMarkdown` | Same subset; local tokenizer estimate |
| Full Workers AI / AutoRAG | Available | Not provided |
| Global placement | Available | Not provided |

See [Compatibility](/platform/compatibility), [Unsupported](/platform/unsupported), and [Cloudflare AI Search](https://developers.cloudflare.com/ai-search/).
