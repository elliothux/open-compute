# P19：AI Search 专用 Rerank provider

状态：**implemented、verified**（2026-09-30）。本阶段覆盖
[#131](https://github.com/elliothux/open-compute/issues/131)。当前公开合同与 deviation 以
[Cloudflare 兼容矩阵](../references/cloudflare-compatibility.md)和
[能力偏差](../references/p1-deviations.md)为准。

## 实现结果

- AI Search reranking 使用独立的 reranking model catalog 和两个闭集协议：Cohere `POST /v2/rerank` 的 `cohere_rerank_v2`，以及通用 `POST /v1/rerank` 子集 `rerank_v1`。Chat Completions 不再承担 rerank，也没有旧配置、prompt parser 或 fallback。
- provider codec 共用有界 HTTP、认证和稳定错误边界；严格拒绝缺失/重复 index、非有限或越界 score、超限 body、malformed JSON 与 provider failure，不把上游正文、endpoint 或 credential 暴露给 tenant。
- Search 保持 text-only。成功响应返回 `query_kind = "text"`；image/file/multimodal query 在公开边界 fail closed。文档 ingestion 的 OCR/VLM 不扩大 query modality。
- hosted differential 冻结了当前可观察语义：默认 rerank threshold 为 0.4；bounded retrieval pool 在最终 `max_num_results` 截断前 rerank；rerank 后顶层 `score` 等于 `reranking_score`，namespace merge 也按该顶层 score 排序；普通 `match_threshold` 只过滤 vector similarity，keyword-only candidate 不受其影响。
- keyword retrieval 内部使用按最大值归一化的 fusion score，公开 `keyword_score` 保留正的原始 BM25 诊断值。metadata boosting 在 rerank 前执行：按字段方向把候选 rank 归一化，取字段平均后乘 0.3，加到 retrieval score，再按全局最大值归一化。

## 验证

- 使用 Cloudflare `cf` CLI 创建临时 AI Search instance 做 hosted differential，覆盖 vector/keyword/hybrid、boost、rerank pool、threshold、顶层 score 与 namespace merge；临时 instance 已全部删除。
- 真实 provider qualification 的三条生产流程通过：百炼 embedding + DeepSeek Chat、百炼 embedding + Cohere v2 rerank + DeepSeek Chat、百炼 embedding + 百炼 `rerank_v1` + DeepSeek Chat。记录为 `.temp/gate-run/20260930T010602-efa11527/report.json`。
- 本地 `p5-search` fixture 覆盖两个 rerank wire protocol、默认/显式 threshold、score projection、失败语义与 restart 路径；最终 workspace Gate 和 90.05% Rust line coverage 通过。

没有保留旧 Chat rerank 或历史配置兼容路径；新增 provider 仍必须实现已声明的闭集协议，不支持任意 JSON template adapter。
