# P5：Vectorize、AI Search 与文档解析实现记录

> 状态：**Completed / local one-round verification PASS**，2026-09-02。
> P5 核心实现、固定 fixture、本机覆盖率、Cloudflare 合同检查与用户要求的一轮 P5 Gate 均已完成；
> timing-three、跨平台和正式发行资格仍由独立计划追踪，不在本记录中冒充已通过。

## 1. 结论与范围

P5 的本地核心已经进入唯一 production path：Vectorize resource 与 exact engine、AI provider/model catalog、
AI Search namespace/instance/ingestion/retrieval/chat、Markdown Conversion，以及 Xberg parser child 共用现有
SQLite、外接 S3、generation-authenticated private backend 和 pinned stock workerd。没有增加 Redis、独立向量
数据库、常驻模型 daemon、tenant-visible Xberg API 或旧开发版本兼容路径。

归档设计分别见 [Vectorize 与 AI Search](p5-vectorize-ai-search.md)和
[Xberg 文档解析](p5-7-xberg-document-parsing.md)。timing-three、四平台 release、完整 parser process matrix、
托管 rich-document differential 与正式 package 仍由[发行验收计划](../acceptance/p5-release-acceptance.md)追踪。

## 2. 冻结输入

| 输入 | 冻结值 |
| --- | --- |
| workerd release / revision | `v1.20260830.1` / `e9dda5963aba7ee4323960db795690ec78fec118` |
| compatibility date | `2026-08-30` |
| workers-types / workers-sdk revision | `5.20260830.1` / `f8085545bcaa2c639f171c25e4424685036a0e10` |
| Xberg | `=1.0.14`，crate SHA-256 `68568d75a993709564cb27361409b46988ec585f9fb59c8f91a113ff7f6b4e29` |
| Xberg features | `tokio-runtime,pdf,office,excel,xml` |
| parser contract SHA-256 | `19decbaa581fb83acd9c35d489da8a1ba0e66a0336aa7dfc5b6b5eb00421a8dd` |
| public corpus | 40 files；manifest SHA-256 `599efa6fb8d5ae4517c1a62034bf4db69af7c152b77421709be5362882be31c1` |
| hostile corpus | 15 cases；manifest SHA-256 `c02d5091a29c5e411181593074e7b707ceb2126ae68d304b5bab1a8b9b65d542` |

## 3. 已确定的实现结果

- Vectorize 接受 32–1536 dimensions，使用 per-index SQLite、durable ordered mutation、metadata pre-filter、
  cosine/euclidean/dot-product exact search，以及 512 MiB host-wide weighted warm snapshot cache。
- 同 mutation batch 的 duplicate ID 为 first-wins；`insert` 已存在 ID 成功跳过且 mutation frontier 前进；
  `$ne`/`$nin` 包含缺失字段。默认 quota 为 100k vectors/index，control schema 上限为 200k。
- AI provider 配置显式冻结 tokenizer artifact path/SHA-256 与 generation capabilities；embedding、chunk、index 和
  custom metadata 变更使用 fenced full reindex，不原地重解释 active generation。
- AI Search 使用 per-instance SQLite 和外接 S3 immutable objects，支持 items/jobs、vector/keyword/hybrid、
  rewrite/rerank、non-stream/SSE chat 与 1–10 instance fan-out。相同 model contract 共享 query embedding。
- Markdown Conversion 与 AI Search ingestion 共用 `ocd __document-parser-v1` 的 OCDP v1 child protocol。
  public admission 为 CSV、DOCX、HTML、JSON、Markdown、ODS、ODT、PDF、TXT、XLS、XLSM、XLSX、XML；
  XLSB、Numbers 与 ET 均为明确 No-Go。
- parser hard limits 为 4 MiB/file、16 files、32 MiB/batch、16 MiB output、30 s wall deadline、30 CPU seconds、
  64 KiB stderr，以及 host/account/deployment 并发 4/2/1。Linux 使用 2 GiB `RLIMIT_AS`；Darwin 不支持设置
  `RLIMIT_AS`，因此 macOS RSS hard limit 明确保留在发行验收，而不伪称本轮已经验证。

## 4. 验证证据

- `bun run build` 通过；TypeScript 7 strict typecheck、Rolldown runtime/toolchain build 与维护中的示例、脚本、
  conformance fixtures 均由该入口完成。`bun run test:js` 通过 208/208。
- storage AI Search focused tests 21/21、storage Vectorize focused tests 18/18、service AI Search focused tests
  21/21 与最终 backend matrix 12/12、document parser crate/corpus 22/22、最终 parser backend matrix 11/11、
  search crate 16/16、Workers AI 2/2、Workers Vectorize 3/3 均通过。
- `ai_search_backend_tests.rs` 为 968 行，保留为一个 cohesive private service protocol/storage fixture matrix：
  同一 fixture 同时验证 namespace、instance、search、upload 和错误映射 authority，拆散会复制 setup 并弱化
  跨入口不变量；production `ai_search_backend` 已按 catalog/ingest/search/chat/namespace/protocol 所有权拆分。
- `cargo fmt --all --check`、workspace/all-targets/all-features Clippy `-D warnings`、no-default-features、
  Rust 1.98 MSRV、metadata 与 dependency boundaries 均通过。Clippy 在覆盖补测后重新完整执行并通过。
- `p5_search_gate` 使用正式 pinned archive、stock workerd、本地 Qwen embedding fixture、真实 SQLite/S3 fixture
  执行通过 1/1；最终报告见第 5 节。
- 真实 Cloudflare differential 使用唯一前缀 `oc-p5-diff-20260902-a7c4e19f*` 和
  `oc-p5-existing-20260902-b19f3d7a`。验证后删除 3 个 Vectorize indexes、1 个 Worker 与 2 个 AI Search
  instances，并复查 Vectorize/AI Search inventory 为空、Worker absent。
- 该 differential 冻结了 Vectorize dimensions、duplicate/insert-existing/filter/zero-vector 行为，以及 AI Search
  默认配置、typed metadata、embedding/custom-metadata update 的 full reindex 和 chat SSE framing；它没有覆盖
  托管 Markdown Conversion rich-document output/error/limit 矩阵。

## 5. 最终本机证据

| 项目 | 最终值 |
| --- | --- |
| source revision | base Git `c677e475a711473f809300e272a6895af4942acf`；冻结 working-tree Gate source SHA-256 `dee16b30ebb7bb11c30307b0aaca47d2c3498375b3e4105ee702b9ac55b85eab`；conformance source digest `4c7b8e3dff6f2b87423b476ab507d7caa8134662ad6298f47ac3df95c35540fa` |
| coverage | **90.0526%** Rust lines，82,164 / 91,240；46-target workspace 单轮采样加最终 focused regression profiles；报告为 `target/llvm-cov/summary.json`、`target/llvm-cov/lcov.info`、`target/llvm-cov/html/index.html`；`--fail-under-lines 90` 通过 |
| 一轮最终 P5 Gate | `OPEN_COMPUTE_GATE_ROUNDS=1 … ./test/gate.py p5`：1 target / 1 case，**PASS**，29.30 s；报告为 `.temp/gate-run/20260902T081138-b78109d1/report.json` |
| 最终 Cloudflare contract Gate | `OPEN_COMPUTE_GATE_ROUNDS=1 ./test/gate.py p3-contract`：1 target / 14 cases，**PASS**，2.05 s；报告为 `.temp/gate-run/20260902T081103-5d779028/report.json` |

此前 workspace 单轮在 45 个 Rust/真实进程 targets 上通过 883/883 cases，唯一失败是源码 digest 更新后尚未
刷新 `p3-contract` baseline；该失败证据保留在
`.temp/gate-run/failed/20260902T073006-516e783a/report.json`。随后发现并修复 AI Search model-contract CAS 被
过宽 immutable trigger 拒绝的根因，针对 storage/AI Search/Vectorize/parser 的回归、最终 P5 Gate 与最终
14-case contract Gate 均在冻结源码上通过。没有把修复前的 workspace 运行改写成最终全 workspace PASS，
也没有执行 timing-three。

## 6. Cloudflare 兼容性复核

按 `cf-compatibility-check` 对本次变更的类型、runtime、single-latest 和 authority 四道检查完成复核，
**无 P0–P3 actionable finding**：

| surface | 结论 | 证据 |
| --- | --- | --- |
| Vectorize | `aligned`，声明为 `supported_with_deviation` | pinned stable surface 27 members/overloads 全部有 compile/runtime evidence；三种 metric、mutation、namespace、metadata filter/projection、restart 与托管 differential 通过；唯一 deviation 为 `OC-VECTORIZE-001` 的单机 exact/本地 quota/无全球托管拓扑 |
| AI Search | `aligned`，声明为 `supported_with_deviation` | 与 Markdown Conversion 合计 54 members/overloads；namespace/instance/items/jobs、durable ingestion、keyword/vector/hybrid、chat/SSE、generation fencing 与 operator-pinned provider 均走 production authority 和最终 P5 Gate；deviation 为 `OC-AI-SEARCH-001` |
| Workers AI Markdown Conversion | `aligned`，声明为 `supported_with_deviation` | `env.AI.toMarkdown()` direct/handle overload、`supported()`、13-format admission、OCDP child limits 和错误映射有 compile/runtime/corpus coverage；deviation 为 `OC-AI-MARKDOWN-001` |
| cross-cutting catalog | `aligned` | 2,178 target members = 1,585 `supported` + 593 `supported_with_deviation`，`blocked=0`；2,178 条 `memberEvidence` 双射，最终 `p3-contract` 14/14 |

AI Gateway、完整 Workers AI inference、AutoRAG、托管 connector、全球 placement/replication/fleet quota 与
Cloudflare `/client/v4` 管理面属于显式 self-host/hosted 非目标；它们没有删除或伪造已声明的 stable tenant
surface。托管 Markdown Conversion rich-document differential 尚未执行，作为 release qualification 明确保留，
不影响上述本地合同 PASS，也不外推为托管端全量实测一致。

## 7. 已接受限制

本地 exact-only、operator-managed OpenAI-compatible provider、13-format parser admission 和声明的 Cloudflare
deviation 是当前 Day1 产品边界。ANN、OCR、AI Gateway、continuous R2/website ingestion、额外 provider adapter、
parser pool/dedup 和 Markdown Conversion REST adapter 不属于 P5 已实现范围；未完成的 release qualification
不允许通过 fallback、mock 或旧路径伪装通过。
