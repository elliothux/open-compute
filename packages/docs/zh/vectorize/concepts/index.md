# 概念

每个公开索引对应一份本机 SQLite authority。Mutation（`insert` / `upsert` / `deleteByIds`）持久且异步：Worker 收到 `mutationId`，applied 可见性由本地 coordinator 推进。查询（`query` / `queryById` / `getByIds` / `describe`）读取已应用的 frontier。

检索为**精确**检索（带 metadata 预过滤的全量扫描），不是 Cloudflare 的分布式近似拓扑。分数遵循索引度量：cosine、euclidean 或 dot-product。

Metadata 过滤使用已索引的 metadata 表面（`$eq`、`$ne`、`$in`、`$nin`、`$lt`、`$lte`、`$gt`、`$gte` 及组合）。过滤某属性前需先创建 metadata index，与 Cloudflare 合同一致。

不提供：

- 已弃用的 beta `VectorizeIndex` 类
- 托管全球就近存放 / 复制
- Cloudflare 计费或舰队级配额（例如将 1000 万–2000 万向量/索引作为本地承诺）
- 自动 embedding 生成（使用自有模型或 [AI Search](/zh/ai-search/)）

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| 索引 authority | 托管 Vectorize 服务 | 每索引一份本机 SQLite |
| 查询算法 | 近似 / 分布式 | 精确 / 本机 |
| 异步 mutation | `mutationId` | 公开形状相同；本机持久 coordinator |
| Beta `VectorizeIndex` | 遗留 | 不提供 |
