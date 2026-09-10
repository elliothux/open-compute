# P5：Vectorize 与 AI Search

状态：**implemented（2026-09-02）**。本地核心和定向验收完成；跨平台、parser process matrix 和正式发行见
[P5 资格](../acceptance/p5-release-acceptance.md)。

## 用户结果

- Vectorize 提供三种 metric、durable mutation、typed metadata filter/projection 和 exact top-k search。
- 每个 Vectorize index 使用独立 SQLite；warm snapshot 是可丢弃缓存，SQLite 是唯一结构化 authority。
- AI Search namespace／instance 支持 item upload、异步 parse/chunk/embed/index、keyword/vector/hybrid retrieval、rewrite/rerank 和 chat/SSE。
- 每个 AI Search instance 使用独立 SQLite，原始 document bytes 使用平台 object authority；generation 只有完整提交后才激活。
- Embedding 与 chat 使用 operator 配置的 OpenAI-compatible HTTPS 或 loopback provider；`ocd` 不内嵌模型、不在启动时联网或下载。
- `env.AI.toMarkdown()` 和 rich-document indexing 复用 [P5.1 parser](p5-1-xberg-document-parsing.md)，不暴露 Xberg 或私有解析 API。
- Delete、cancel、full reindex、provider retry、snapshot/restore 和 GC 使用持久 job／generation fence，旧结果不能覆盖新配置。

实现复用一个 `ocd`、一个 workerd、SQLite 和已选 object backend，不增加 Redis、独立向量数据库、常驻模型 daemon 或第二套 runtime。

## Day 1 边界

- Vector search 采用 exact-only；默认 quota 为 100k vectors/index，host-wide warm cache 为 512 MiB。
- 250k × 1536d 只作为压力证据，不是默认额度；ANN 只在实际 quota／latency 无法满足时再引入。
- Model、tokenizer、dimensions 和 provider capability 在 generation 中冻结；配置缺失只影响使用该 provider 的实例，不阻塞离线启动。
- Tenant 隔离、body／dimension／metadata／query bounds、错误脱敏和低基数 metrics 在现有 authority 边界执行。

当前 API 和 deviation 见[兼容矩阵](../references/cloudflare-compatibility.md)。

## 历史验证与限制

固定 workerd `v1.20260830.1` 上，P5 Gate、14-case contract Gate、provider fault、SQLite/S3 lifecycle、snapshot/restore、
真实 Cloudflare 高风险 differential 和 parser corpus 通过。Rust line coverage 为 82,164 / 91,240（90.0526%）。
当日 catalog 为 2,178 members：1,585 `supported`、593 `supported_with_deviation`、`blocked=0`。

本地 exact-only、operator-managed provider 和单机 topology 是接受限制。ANN、OCR、AI Gateway、AutoRAG、continuous ingestion、
额外 provider adapter 和全球 placement／replication 不在本阶段；provider/backend 重构由
[P5.2](p5-2-ai-provider-profiles.md)完成重构。
