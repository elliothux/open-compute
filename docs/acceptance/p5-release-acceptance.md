# P5：Vectorize、AI Search 与文档解析剩余发行验收

> 状态：**Active / release qualification pending**，2026-09-02。P5 本地核心实现与本机验收记录已经归档；
> 本页只保留不能由本轮一台 macOS arm64 开发机和一轮 Gate 证明的发行资格。归档见
> [Vectorize 与 AI Search](../implemented/p5-vectorize-ai-search.md)、
> [Xberg 文档解析](../implemented/p5-7-xberg-document-parsing.md)和
> [完成记录](../implemented/p5-vectorize-ai-search-results.md)。本计划不创建旧实现兼容义务。

## 1. 已完成前提

- Vectorize、AI Search、Markdown Conversion 与 Xberg parser 已进入唯一 production path；
- pinned type inventory、capability/deviation 与真实 Cloudflare 高风险 differential 已冻结；
- 40-file public corpus、15-case hostile corpus、离线 tokenizer artifact、S3/SQLite snapshot/restore 与后台 health
  属于归档完成记录的证据。
- 本轮最终一轮 Gate 与 coverage 的精确数据只在完成记录中补录，本页不重复或预先宣称 PASS。

## 2. 剩余资格

以下项目不冒充本轮已验证：

1. 使用 `crates/search/examples/exact_search_benchmark.rs` 复核并保留 10k/50k/100k/250k ×
   384/768/1024/1536 dimensions、1%/10%/100% selectivity、concurrency 1/4/16 的 release-mode report，冻结
   p95、RSS 与 quota Go；没有留存报告时不把设计文档中的工程数字提升为 release SLA。
2. Linux x64/arm64、macOS x64/arm64 的 Rust 1.98 release build、正式 package、单 binary size/hash 与隔离离线启动。
3. parser child 的完整 panic/abort/OOM/signal/orphan cleanup/crash recovery/soak 矩阵，以及各平台实际 CPU、
   address-space 与 process-group hard-limit 行为。Darwin 当前拒绝 `setrlimit(RLIMIT_AS)`；必须在 macOS 发行前
   落实并验证等价 RSS hard limit，不能把 Linux 的 2 GiB `RLIMIT_AS` 结果外推到 macOS。
4. 使用专属临时资源对 Cloudflare 托管 Markdown Conversion rich-document output/error/limit 做 differential；
   不复用或修改账号既有 Worker、Vectorize index、AI Search instance、AI Gateway 或数据源。
5. release artifact 的 license/NOTICE、签名、发布与受权限约束的外部资格。

## 3. 验收纪律

- 每次使用唯一前缀，先 inventory，后创建，结束逐一删除并复查 absent；
- 缺少目标架构、账号权限或正式 pin 时 fail closed，不用 mock/miniflare 替代 release evidence；
- parser/模型/runtime 不在 startup 或 request path 下载 artifact；
- 失败证据保留到 `.temp/<purpose>/failed/`，不重写归档历史结果；
- 任何实现修复都回到单轮 development checks，再重新开始单轮 final acceptance。

## 4. 完成条件

可复现 benchmark report、四平台、完整 parser process matrix、托管 rich-document differential 和正式 package evidence
全部通过后，把本页连同实际 revision、pin、命令、case counts、hash、size 与已接受限制移入
`docs/implemented/`；No-Go 或受外部权限阻塞的结论必须明确记录，不能写成已通过。
