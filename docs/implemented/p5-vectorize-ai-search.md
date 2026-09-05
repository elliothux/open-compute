# P5：Vectorize 与 AI Search 单机兼容方案

> 状态：**Core implementation archived / release acceptance active**，2026-09-02。本文记录已经落地的
> Vectorize、AI Search 与 Markdown Conversion Day1 设计、实现边界和验收合同。本机验收数据见
> [完成记录](p5-vectorize-ai-search-results.md)；跨平台发行、timing-three、完整 parser process matrix 与
> 托管 rich-document differential 见仍在维护的[发行验收计划](../acceptance/p5-release-acceptance.md)。
>
> PDF/Office 等 rich-format 解析的 Cloudflare API、Xberg 进程隔离、格式矩阵、固定公开 corpus 和实施 Gate 见
> [P5.7 文档解析方案](p5-7-xberg-document-parsing.md)。P5.8 保留其他条件式扩展，当前不实现。

## 1. 结论

Vectorize 和 AI Search 均可在当前单机架构内实现，不需要 Redis、独立向量数据库、独立模型服务进程或
第二个 workerd。推荐组合是：

```text
stock workerd + workerLoader
+ TypeScript facade / private transport
+ Rust exact vector engine
+ per-resource SQLite authority
+ SQLite FTS5 BM25
+ external S3-compatible object storage
+ operator-configured embedding/chat model endpoints
+ Cloudflare-compatible env.AI.toMarkdown facade
```

Day1 的核心选择是：

1. **SQLite 是唯一结构化 authority**。一个公开 Vectorize index 对应一个 SQLite；一个 AI Search
   instance 对应一个 SQLite。namespace 只是逻辑分区或父资源，不映射成独立数据库。
2. **先做精确向量搜索，不在 Day1 引入 ANN**。SMB self-deploy 的正确性、恢复和有界资源优先于
   Cloudflare 的 20M vectors/index 托管规模。
3. **AI Search 上传原始字节进入现有外接 S3**；解析文本、chunk、FTS、vector 和 generation 状态进入
   instance SQLite。一次查询不会跨多个本地 authority 才能得到一个完整 generation。
4. **Embedding/LLM 默认不内嵌进 `ocd`**。`ocd` 调用 operator model catalog 中的外部 HTTPS 或 loopback
   OpenAI-compatible endpoint；生产启动不探测远端 endpoint、不下载模型。
5. **继续使用现有 `workerLoader`**。新增产品由 loader realm 注入 facade，经 generation-authenticated
   loopback backend 调用 Rust，不修改或 fork workerd。
6. **兼容 Cloudflare 公开 binding 的常用行为，不复制其分布式内部实现**。结果 shape、错误边界、
   mutation 可见性、过滤顺序和主要限制进入合同；ANN recall、chunk 边界、模型输出和排序分数不要求逐位一致。
7. **Xberg 只是一种内部 parser 实现**。P5.7 对外使用标准 `[ai]` binding 的 `env.AI.toMarkdown()` 和
   AI Search Items API，禁止新增 tenant-visible Xberg/parse API。

可行性判定：

| 范围 | 判定 | 主要条件 |
| --- | --- | --- |
| Vectorize stable Workers binding | 高 | 接受单机额度，先用 exact search |
| Metadata index/filter | 高 | EAV 物化索引、严格类型和 pre-filter |
| AI Search built-in storage/items | 高 | 原始字节使用已配置 S3 |
| AI Search vector/keyword/hybrid | 高 | model endpoint contract 固定，FTS5 可用 |
| PDF/Office/ODF/Numbers 文档解析 | 中高，P5.7 | Xberg 最小 feature、parser child、固定 corpus 通过 |
| AI Search chat/query rewrite/rerank | 中高 | operator 必须配置相应 model/protocol capability |
| Markdown Conversion Workers API | 中高，P5.7 | pinned overload、CF response shape、parser child 与格式 Gate 通过 |
| 本地内嵌模型推理 | Day1 不做 | 会破坏轻量单 binary、离线启动和跨平台供应链 |
| Cloudflare 20M/index 或全球边缘规模 | 非目标 | 需要分布式 ANN、compaction 和多副本 |

### 1.1 当前实现快照

截至 2026-09-02，P5.0–P5.7 的本地核心已经进入唯一 production path：

- `VectorizeIndex`、`AiSearchNamespace`、`AiSearchInstance` resource、descriptor、toolchain import、loader
  facade、generation-authenticated backend、snapshot/restore、health/metrics/doctor/support bundle 已接通；
- Vectorize 使用 per-index SQLite、durable ordered mutation、indexed metadata pre-filter、三种 metric、exact
  top-k、512 MiB host-wide weighted warm LRU 与 bounded cold scan；
- AI Search 使用 per-instance SQLite 与外接 S3 immutable objects，`items.upload()` durable commit 后立即返回
  `queued`，maintenance 异步完成 parse/chunk/embed/FTS/vector generation；full reindex、cancel、delete/GC、
  restart recovery 与 query generation pin 均由持久状态机约束；
- namespace/direct instance、items/jobs、vector/keyword/hybrid、rewrite/rerank、non-stream/SSE chat 与
  multi-instance API 已在 pinned stock workerd 的同一个 P5 Gate 中执行；
- `env.AI` 只声明 `aiGatewayLogId` 与 Markdown Conversion 的 54 个 AI/AI Search stable members/overloads；
  `run/models/gateway/aiSearch/autorag` 等未声明能力稳定拒绝；
- Xberg parser child、40-file corpus、15-case hostile corpus 及 rich-document indexing 见同目录的 P5.7 文档。

本轮远端差分使用唯一 `oc-p5-diff-20260902-a7c4e19f*` / `oc-p5-existing-20260902-b19f3d7a`
前缀，验证后已逐一删除并复查 Worker、Vectorize index 与 AI Search instance 均不存在。已冻结的高风险行为包括：
Vectorize 32–1536 维、同 batch duplicate ID first-wins、insert-existing no-op 但 frontier 推进、`$ne/$nin`
包含缺失字段，以及 AI Search 默认配置、typed metadata、embedding/custom-metadata update 的 fenced full reindex
和 chat SSE framing。

## 2. 契约基线与参考边界

### 2.1 唯一 runtime 基线

本阶段绑定合同固定到 [`packages/runtime/workerd.lock.json`](../../packages/runtime/workerd.lock.json)：

```text
workerd release                  v1.20260830.1
workerd revision                 e9dda5963aba7ee4323960db795690ec78fec118
effective compatibility date    2026-08-30
workers-types                    5.20260830.1
workers-sdk                      f8085545bcaa2c639f171c25e4424685036a0e10
```

类型 inventory 以 pinned workerd 中的
[`vectorize.d.ts`](../../third_party/workerd/types/defines/vectorize.d.ts) 和
[`ai-search.d.ts`](../../third_party/workerd/types/defines/ai-search.d.ts) 为本地快照。本轮已经完成以下交叉校验并冻结
到 conformance catalog、deviation reference 与 stock-workerd Gate：

- 当前 Cloudflare 文档；
- pinned workers-types generated snapshot；
- pinned workerd 内部 facade；
- Wrangler 对 `vectorize`、`ai_search_namespaces` 和 `ai_search` 的配置 shape；
- 真实 Cloudflare Workers differential；所有远端临时 Worker、Vectorize index 与 AI Search instance 已按唯一前缀清除并复查。

若文档、类型和托管行为冲突，以本项目声明的 pinned latest contract 和实测行为形成一份唯一矩阵；不能通过
兼容历史 Vectorize V1 或旧 AI Search/AutoRAG API 引入双实现。

### 2.2 官方行为依据

截至 2026-09-02，Cloudflare 明确区分：

- Vectorize 接收用户已经生成的 vector，负责 mutation、metadata filtering 和相似度查询；
- AI Search 接收文件/数据源，负责解析、chunk、embedding、vector/BM25、fusion、可选 rerank 和回答生成。

主要来源：

- [AI Search 工作原理](https://developers.cloudflare.com/ai-search/concepts/how-ai-search-works/)
- [AI Search built-in storage](https://developers.cloudflare.com/ai-search/configuration/data-source/built-in-storage/)
- [AI Search chunking](https://developers.cloudflare.com/ai-search/configuration/indexing/chunking/)
- [AI Search keyword search](https://developers.cloudflare.com/ai-search/configuration/indexing/keyword-search/)
- [AI Search hybrid search](https://developers.cloudflare.com/ai-search/configuration/indexing/hybrid-search/)
- [AI Search limits](https://developers.cloudflare.com/ai-search/platform/limits-pricing/)
- [Vectorize Workers API](https://developers.cloudflare.com/vectorize/reference/client-api/)
- [Vectorize metadata filtering](https://developers.cloudflare.com/vectorize/reference/metadata-filtering/)
- [Vectorize limits](https://developers.cloudflare.com/vectorize/platform/limits/)

### 2.3 workerd、Miniflare 与 WDL 的用途

| 来源 | 可以参考 | 不能作为本地 backend |
| --- | --- | --- |
| workerd | `Vectorize` facade、参数/响应转换、异常映射、pinned 类型 | 当前 dynamic loader 下不能按每次部署修改静态 config；AI Search 只有类型不代表本地 engine |
| Miniflare | plugin/config wiring、remote binding shape、测试 fixture | 当前 Vectorize/AI Search 主要代理托管 Cloudflare，不提供本地 durable engine |
| WDL | provider catalog、credential routing、binding adapter、immutable deployment | compatibility matrix 未提供 Vectorize/AI Search；不能照搬 Redis/多服务拓扑 |

参考链接：

- [workerd Vectorize facade](https://github.com/cloudflare/workerd/blob/main/src/cloudflare/internal/vectorize-api.ts)
- [workerd AI Search types](https://github.com/cloudflare/workerd/blob/main/types/defines/ai-search.d.ts)
- [Miniflare Vectorize plugin](https://github.com/cloudflare/workers-sdk/blob/main/packages/miniflare/src/plugins/vectorize/index.ts)
- [Miniflare AI Search plugin](https://github.com/cloudflare/workers-sdk/blob/main/packages/miniflare/src/plugins/ai-search/index.ts)
- [WDL compatibility](https://github.com/wdl-dev/wdl/blob/main/docs/compatibility.md)

## 3. Day1 目标、非目标与支持层级

### 3.1 目标

- 一个 `ocd`、一个 data directory、一个 pinned stock-workerd child、一个现有 S3 配置；
- Vectorize latest stable `Vectorize` binding 的常用 API；
- AI Search 当前 beta binding 中 built-in storage 的常用 API；
- namespace、instance、item、job 和 direct instance binding；
- vector、keyword 和 hybrid retrieval；
- P5.7 对齐 Cloudflare 当前 rich-format allowlist 的 PDF/Office/ODF/Numbers 解析；
- 标准 `[ai]` binding 中 Markdown Conversion 的 `toMarkdown` direct/handle API，以及 AI Search Items API
  对 rich document 的异步处理；
- operator 映射后的 embedding、query rewrite、rerank 和 chat completion；
- 每个 mutation/job 的 durable enqueue、claim、retry、fence、cancel、restart recovery；
- 完整接入 resource lifecycle、binding descriptor、health、metrics、doctor、support bundle、snapshot/restore；
- 默认拒绝未实现能力，禁止静默降级为不同语义。

### 3.2 非目标

- Vectorize V1/beta legacy class 与历史 index 格式；
- Cloudflare 的 edge placement、跨地域副本、全球 eventual consistency；
- 20M vectors/index 托管规模、Cloudflare ANN recall 或延迟 parity；
- AI Gateway 管理面、Cloudflare billing/token ID；
- 完整 Workers AI inference platform；P5.7 只实现标准 `[ai]` binding 的 Markdown Conversion 子集，
  `run/models/gateway/batch` 等保持 deviation；
- website crawler、Browser Rendering、R2 data-source continuous sync；
- 图片 OCR、扫描 PDF OCR、音频/视频解析；这些属于当前不实施的 P5.8；
- Cloudflare 当前未列入 AI Search rich formats 的 PPT/PPTX、RTF、ODP、DOC、EPUB；
- Cloudflare 模型输出、chunk byte range、BM25/vector 最终分数逐位一致；
- 多节点同时写一个 data directory 或一个 SQLite；
- 在生产启动或请求路径下载模型、tokenizer、extension 或 native library；
- Cloudflare `/client/v4` Markdown Conversion REST、账号 API token/permission 和通用管理 API envelope；这些若
  后续纳入属于 P5.8 独立 data-plane adapter，不由 Operator API 冒充。

### 3.3 兼容层级

实现和验收按以下优先级判定：

1. **API shape**：方法、重载、字段、ReadableStream/SSE、Promise rejection；
2. **权限与隔离**：account/resource/binding/generation/descriptor；
3. **durability**：mutation 可见性、generation activation、crash recovery、cancel/fence；
4. **主要限制**：dimension、ID、metadata、batch、topK、file size、multi-instance fan-out；
5. **检索语义**：namespace 和 metadata pre-filter、metric ordering、BM25、RRF/max、threshold；
6. **检索质量**：通过固定 corpus 的 recall/order invariant 验证，不要求与 Cloudflare 内部 ANN 或模型逐位相同。

## 4. 总体架构

```text
Public/control HTTP
        │
        ▼
┌────────────────────────────────────────────────────────────┐
│ ocd                                                        │
│                                                            │
│  control.sqlite                                            │
│    resources / bindings / namespace parent refs            │
│                                                            │
│  Vectorize service          AI Search service              │
│    mutation coordinator       indexing coordinator          │
│    exact search engine        model endpoint client         │
│    metadata pre-filter        parse/chunk/vector/BM25        │
│                                                            │
│  <data>/vectorize/...       <data>/ai-search/...            │
│    per-index SQLite           per-instance SQLite            │
│                                                            │
│  existing S3 adapter ───────── AI Search immutable objects  │
└───────────────────────┬────────────────────────────────────┘
                        │ authenticated loopback
                        ▼
┌────────────────────────────────────────────────────────────┐
│ pinned stock workerd                                       │
│ workerLoader → immutable tenant Worker                     │
│ tenant env → TS facade → ctx.exports transport             │
└────────────────────────────────────────────────────────────┘
                        │
                        ▼
            configured model endpoint
              HTTPS or trusted loopback
```

必须保持以下 authority 规则：

- `control.sqlite` 决定资源、父子关系、binding 和 lifecycle；
- per-resource SQLite 决定 vector/item/chunk/job/mutation 当前状态；
- S3 决定 AI Search 原始文件字节是否存在，但 S3 list 不替代 SQLite catalog；
- memory snapshot、ANN 文件、model-response cache、workerd loader cache 都可以丢失并重建；
- 不把内部 indexing 工作提交给租户 Queue/Workflow；
- SQLite transaction 内不执行 S3、HTTP、模型调用或其他 async I/O。

## 5. crate 与源码归属

新增一个 `crates/search`，不拆成多个产品 crate：

| owner | 新增职责 |
| --- | --- |
| `core` | `BindingKind`、AI model endpoint/contract、limits、ID/error/value types |
| `storage` | Vectorize/AI Search paths、catalog、SQLite schema、repository、inspection、snapshot/restore |
| `artifacts` | AI Search S3 key、streaming put/get/delete、immutable reference verification |
| `search` | vector metric、top-k、filter expression、chunk、fusion、score normalization；纯计算 |
| `workers` | binding declaration/descriptor、deployment admission、RuntimeSource snapshot |
| `runtime` | workerd pin/config/lifecycle；不包含产品 engine |
| `service` | control/data plane、private backend、model endpoint HTTP、coordinator、health/metrics/doctor |
| `packages/runtime` | Vectorize/AI Search facade、transport、wire validator、loader injection |
| `packages/toolchain` | Wrangler config import、generated Env、binding reconciliation |

依赖方向：

```text
search ──────────────> core
storage ─────────────> core
artifacts ───────────> core
workers ─────────────> core + storage + artifacts
service ─────────────> core + search + storage + artifacts + workers + runtime
```

`search` 不打开 SQLite、不访问 S3/HTTP，也不依赖 `workers`/`service`。不要创建只转发调用的
`VectorizeService`/`AiSearchService` 多层 wrapper；抽象仅用于纯算法、authority repository 或安全边界。

## 6. 资源与 binding 模型

### 6.1 新资源类型

`BindingKind` 增加：

```rust
VectorizeIndex,
AiSearchNamespace,
AiSearchInstance,
```

稳定 token：

```text
vectorize_index
ai_search_namespace
ai_search_instance
```

三者都使用现有：

```text
creating → ready → deleting → tombstoned
availability: healthy / degraded / unavailable
resource generation
read/write CanonicalPermissions
referrer / delete fence
```

其中：

- Vectorize index 是普通 generic resource；
- AI Search namespace 是父资源，不创建本地数据库；
- AI Search instance 是 namespace 的 child resource，并创建一个 SQLite；
- namespace binding 的 `create()` 只能在绑定的 namespace/account 内创建 child；
- direct instance binding 只能访问固定 instance；
- namespace 删除在仍有 live instance 时返回 conflict；
- instance/index 删除在有 live deployment binding 时沿用现有 referrer fence。

P5.7 的标准 `[ai] binding = "AI"` 不对应 tenant resource，不加入上述 `BindingKind`。它沿用 Images 和 Version
Metadata 的 platform-provided binding 模型，扩展 `BuiltinBindingKind::Ai` 与 immutable descriptor，由
workerLoader 注入 `env.AI` facade。Day1 capability 仅包含 Markdown Conversion；descriptor 和 capability catalog
必须让 full Workers AI unsupported 状态可检查，不能因 binding 名为 `AI` 就宣称 `run()` 等成员已经实现。

### 6.2 control.sqlite 产品表

示意 schema：

```sql
CREATE TABLE vectorize_indexes (
    resource_id              TEXT PRIMARY KEY,
    storage_key              TEXT NOT NULL UNIQUE,
    schema_version           INTEGER NOT NULL,
    dimensions               INTEGER NOT NULL,
    metric                   TEXT NOT NULL,
    quota_vectors            INTEGER NOT NULL,
    quota_bytes              INTEGER NOT NULL,
    created_at_ms            INTEGER NOT NULL,
    FOREIGN KEY(resource_id) REFERENCES resources(id)
);

CREATE TABLE ai_search_namespaces (
    resource_id              TEXT PRIMARY KEY,
    created_at_ms            INTEGER NOT NULL,
    FOREIGN KEY(resource_id) REFERENCES resources(id)
);

CREATE TABLE ai_search_instances (
    resource_id              TEXT PRIMARY KEY,
    namespace_resource_id    TEXT NOT NULL,
    instance_key             TEXT NOT NULL,
    storage_key              TEXT NOT NULL UNIQUE,
    schema_version           INTEGER NOT NULL,
    model_contract_sha256    BLOB NOT NULL,
    created_at_ms            INTEGER NOT NULL,
    UNIQUE(namespace_resource_id, instance_key),
    FOREIGN KEY(resource_id) REFERENCES resources(id),
    FOREIGN KEY(namespace_resource_id) REFERENCES resources(id)
);
```

`instance_key` 对应 Cloudflare `AiSearchConfig.id`，不使用 display name 作为 identity。所有父子查询必须同时
限定 account 和 namespace resource；不能仅凭字符串 name 全局查找。

### 6.3 本地路径

```text
<data>/vectorize/<account-id>/<resource-id>/data.sqlite
<data>/ai-search/<account-id>/<resource-id>/data.sqlite
```

`storage_key` 沿用 KV/D1 规则：

```text
v1/<account-id>/<resource-id>/data.sqlite
```

目录在资源 `creating` 物化，使用 product staging/trash 和既有 path containment、symlink、fsync 规则。
Day1 直接扩展当前 layout、snapshot role 和 schema tuple，不为尚未发布的旧开发 layout 保留迁移分支。

## 7. Operator AI provider and model catalog

### 7.1 Cloudflare 的公开 embedding 配置

Cloudflare 不让 tenant 在 AI Search Worker API 中填写 embedding endpoint、HTTP header 或 provider API key。
公开配置属于 `AiSearchConfig`，核心字段是模型 alias：

```ts
const instance = await env.AI_SEARCH.create({
  id: "knowledge-base",
  embedding_model: "@cf/qwen/qwen3-embedding-0.6b",
  index_method: { vector: true, keyword: true },
});
```

当前 pinned type 同时包含：

```ts
type AiSearchConfig = {
  embedding_model?: string;
  ai_gateway_id?: string;
  token_id?: string;
  // generation/rewrite/rerank/index/chunk fields omitted
};
```

Cloudflare 的行为边界是：

- `embedding_model` 是 instance-level 配置，同时用于文档 chunk 和 query；不存在 per-request embedding override；
- 字段省略时使用 Smart Default，由 Cloudflare 选择并可能随时间更新推荐模型；
- [Models 文档](https://developers.cloudflare.com/ai-search/configuration/models/)与当前 Instance `update()`
  文档/type 对 embedding update 的表述不一致；本轮真实托管差分确认 `update({ embedding_model })` 被接受并启动
  reindex，因此本地以 fenced full-generation rebuild 实现，不原地改写 active vectors；
- Workers AI alias（`@cf/...`）使用 Cloudflare 托管模型；OpenAI、Google 等第三方 alias 需要先把 provider key
  配到 AI Gateway，再用 `ai_gateway_id` 把 instance 连到该 gateway；
- Cloudflare 的每个 AI Search instance 都连接一个 AI Gateway，embedding、rewrite、rerank 和 generation call
  都经过它；官方明确不建议对该 gateway 开 embedding cache 或 rate limit，以免返回错误 vector 或中断索引；
- `token_id` 是 Cloudflare service API token identity，不是 embedding provider key；tenant 不会在
  `embedding_model` 旁边直接提交 OpenAI key。

截至 2026-09-02，[官方 production embedding models](https://developers.cloudflare.com/ai-search/configuration/models/supported-models/)
包括以下合同；model catalog 必须按上游 snapshot 维护，不能由 provider 响应临时猜 dimensions：

| alias | dimensions | max input tokens | metric |
| --- | ---: | ---: | --- |
| `google-ai-studio/gemini-embedding-001` | 1536 | 2048 | cosine |
| `openai/text-embedding-3-small` | 1536 | 8192 | cosine |
| `openai/text-embedding-3-large` | 1536 | 8192 | cosine |
| `@cf/baai/bge-m3` | 1024 | 512 | cosine |
| `@cf/baai/bge-large-en-v1.5` | 1024 | 512 | cosine |
| `@cf/qwen/qwen3-embedding-0.6b` | 1024 | 8192 | cosine |
| `@cf/google/embeddinggemma-300m` | 768 | 512 | cosine |

### 7.2 open-compute 的 operator 映射

`PlatformConfig` 新增 `ai.providers` 与 model catalog。provider 保存 endpoint/auth，model entry 只保存
Cloudflare alias 到 provider/upstream model 及冻结模型合同的映射：

```toml
[ai]
default_embedding_model = "@cf/baai/bge-m3"
default_generation_model = "openai/gpt-5-mini"
max_provider_in_flight = 16
max_embedding_inputs_per_batch = 96
max_embedding_request_bytes = 2097152
max_embedding_response_bytes = 16777216
provider_timeout_ms = 30000
query_timeout_ms = 15000

[ai.providers.openai]
base_url = "https://api.openai.com/v1"
auth = { kind = "bearer", secret = { file = "/run/secrets/openai-api-key" } }

[ai.providers.ollama]
base_url = "http://127.0.0.1:11434/v1"
auth = { kind = "none" }

[ai.embedding_models."@cf/baai/bge-m3"]
provider = "ollama"
remote_model = "bge-m3"
model_revision = "operator-pinned-2026-09"
dimensions = 1024
metric = "cosine"
max_input_tokens = 512
tokenizer = "bge_m3"
tokenizer_revision = "operator-pinned-2026-09"
tokenizer_artifact = { path = "/opt/open-compute/tokenizers/bge-m3-tokenizer.json", sha256 = "<lowercase-sha256>" }

[ai.embedding_models."openai/text-embedding-3-small"]
provider = "openai"
remote_model = "text-embedding-3-small"
model_revision = "openai-contract-2026-09"
dimensions = 1536
request_dimensions = 1536
metric = "cosine"
max_input_tokens = 8192
tokenizer = "cl100k_base"
tokenizer_revision = "openai-contract-2026-09"
tokenizer_artifact = { path = "/opt/open-compute/tokenizers/cl100k-tokenizer.json", sha256 = "<lowercase-sha256>" }

[ai.generation_models."openai/gpt-5-mini"]
provider = "openai"
remote_model = "gpt-5-mini"
model_revision = "operator-pinned"
max_context_tokens = 400000
capabilities = ["chat", "rewrite", "rerank"]
```

这是明确接受的 self-host operator-config deviation。Cloudflare 托管 endpoint、credential 和 gateway；
open-compute 必须由 operator 在 provider 层提供 endpoint/auth，再由 model catalog 引用。它不改变 Worker-facing
`AiSearchConfig.embedding_model` 的字段和 alias，但 open-compute 配置文件本身不尝试兼容 Cloudflare Dashboard、
Wrangler 或 AI Gateway 配置。

`openai` 与 `ollama` 在上例中只是 operator 自定义的 provider instance name，名称本身没有 `kind` 语义；它们也可叫
`primary`、`local`。Day1 所有 `ai.providers.<name>` 都使用同一个、平台固定的 OpenAI-compatible adapter，配置中不提供
可切换的 `kind`/`protocol`：

```text
POST {base_url}/embeddings
POST {base_url}/chat/completions
Authorization: Bearer <secret>   # auth.kind=bearer 时
```

这里的“OpenAI-compatible”是本方案冻结并用 fixture 验证的请求、响应、错误和 SSE 子集，不表示任何声明兼容的服务
都天然通过。OpenAI 官方 endpoint、Ollama、TEI 或其他服务只有通过相同 contract suite 后才能配置。新增 Cohere、
Gemini native 等协议适配器属于 P5.8，届时才需要引入显式 adapter kind。

provider entry 的必填字段为：

```text
base_url                    # canonical /v1 root，client 追加固定 route
auth.kind                   # bearer 或 none；即使无认证也必须显式 none
```

`auth.kind=bearer` 还必须提供 `auth.secret: SecretReference`，复用平台现有的 `env` 和/或 absolute `file` 语义；
两者同时存在时解析值必须一致。`auth.kind=none` 不得携带 `secret`，且只允许显式 loopback HTTP。credential
缺失时不能自动降级成匿名请求。多个 provider 可以引用同一 secret，多个 model entry 也可以引用同一个 provider；
runtime 按 canonical origin 与 auth identity 共享有界 connection pool。

embedding model entry 的必填字段为：

```text
provider                    # ai.providers.<name> 的引用
remote_model
model_revision
dimensions / metric
max_input_tokens
tokenizer / tokenizer_revision
tokenizer_artifact.path / tokenizer_artifact.sha256
```

这些字段已经由 `AiConfig` 的 deny-unknown-fields 配置合同、离线 artifact no-follow/hash 校验和 fixture 冻结。
`request_dimensions` 只对 OpenAI-compatible contract 中声明支持该参数的 endpoint 生效；例如 Cloudflare 把
`openai/text-embedding-3-large` 也固定为 1536 维，本地必须在请求中显式选择 1536 并验证响应，不能先接收
OpenAI 默认 3072 维再截断。

外部和内部配置必须严格分层：

```text
AiSearchConfig.embedding_model
  → Cloudflare-compatible public alias
  → operator model entry
  → provider name + remote model
  → provider base_url + auth
  → fixed OpenAI-compatible route
  → bounded embedding request
```

tenant 省略 `embedding_model` 时，本地使用 `ai.default_embedding_model`，并在 instance 创建事务中立刻解析成
固定 model contract。open-compute 不实现会随时间漂移的 Smart Default：operator 修改默认值只影响之后创建的
instance，已有 instance 不自动切换或混用向量。这是为了保证单机索引可重建性的明确 hosted-topology deviation。

`ai_gateway_id` 和 `token_id` 首个 Go 仍 fail closed：本地不实现 AI Gateway/API-token resource，也不把这两个字段
重解释为 provider/auth selector。第三方 endpoint/auth 由 operator provider 预配置，因此常用的 `embedding_model` API shape
保持不变，但 gateway selection 是公开 deviation。若 P5.8 要兼容 `ai_gateway_id`，必须先有独立 gateway resource、
credential ownership、logging/retry/cache/guardrail 语义，不能只加一个字符串路由开关。

### 7.3 启动校验与配置失败语义

`ocd` 启动时必须先 canonicalize provider 与 model catalog，再开放 listener。任一已声明 entry 非法都使启动失败，
不能只跳过坏 entry，也不能等到首次 indexing/query 才发现配置缺失。最低校验为：

| 项目 | 必须拒绝 |
| --- | --- |
| provider | 非法/空名称；未被支持的 `kind`/`protocol` 字段；model 引用了不存在的 provider |
| alias/default | 重复 alias；`default_embedding_model` 未命中；Cloudflare alias 的公开 dimensions、metric 或 max input 与 pinned catalog 冲突 |
| URL | `base_url` 非绝对 URL；userinfo、query 或 fragment；不以 canonical `/v1` 结尾；非 loopback 的明文 HTTP |
| auth | provider 缺少 `auth.kind`；未知 kind；`bearer` 没有合法 `SecretReference`；env/file 同时存在但值不一致；`none` 携带 secret 或用于非 loopback endpoint |
| contract | 空 provider/`remote_model`/revision/tokenizer revision；零或超限 dimension/token limit；未知 metric/tokenizer |
| limits | request bytes、batch、timeout、global concurrency 为零、溢出或超过平台 hard ceiling |

`SecretReference` 只允许平台现有的 env/file 形式，并复用已有的名称、绝对路径、no-follow、权限、长度与一致性
校验。`bearer` 必须解析出一个非空 secret。启动可以验证 secret 可读，但不能联网验证 token。credential 缺失、
空值或 unreadable 都是启动错误，绝不能把 `bearer` 降级为 `none`。

配置测试至少覆盖：同一 provider 被多个 alias 复用、secret 内容 rotation、base URL/auth kind/remote model drift、默认
alias 切换、model 引用缺失、未知配置字段、redirect、跨 origin redirect、Authorization 不出现在日志/错误/support bundle，以及
`doctor --model <alias>` 的成功、401、429、timeout、错误维度和畸形响应。

### 7.4 安全边界

- tenant 只能提交 Cloudflare-compatible model alias，不能提交 model URL、header 或 credential；
- alias 必须存在于 operator model catalog，其 provider 引用与全部 model contract 字段完整，否则 create/update 失败；
- provider `base_url` 是 operator-trusted 配置，仍需严格 URL parse；禁止 userinfo、query、fragment、redirect 和非标准化 path；
- HTTPS 使用固定 webpki roots 和现有 aws-lc/rustls 供应链；loopback HTTP 仅因 operator 显式配置允许；
- model client 不跟随 redirect，避免把 Authorization 发送到不同 origin；
- model client 不缓存 embedding response；retry/admission 由 indexing coordinator 按 durable item/batch 管理，
  不把 operator reverse-proxy cache 当成 AI Search similarity cache；
- auth credential 继续使用 `SecretReference`，不写入 DB、snapshot、status、logs 或错误；
- production startup 只验证配置与 secret 可读性，不联网 probe、不下载模型或 tokenizer；显式
  `doctor --model <alias>` 才做远端能力检查；
- DNS/endpoint 政策属于 operator model backend，不复用 tenant public-only egress；两类 capability 必须隔离。

### 7.5 冻结 model contract

AI Search instance 创建时保存 canonical contract：

```json
{
  "embeddingAlias": "@cf/baai/bge-m3",
  "providerName": "ollama",
  "providerContractSha256": "...",
  "protocol": "openai_v1_embeddings",
  "endpointSha256": "...",
  "authKind": "none",
  "remoteModel": "bge-m3",
  "modelRevision": "operator-pinned-2026-09",
  "dimensions": 1024,
  "requestDimensions": null,
  "metric": "cosine",
  "maxInputTokens": 512,
  "tokenizer": "bge_m3",
  "tokenizerRevision": "..."
}
```

`providerContractSha256` 覆盖固定 adapter version、canonical `base_url` 和 auth kind；`endpointSha256` 覆盖追加
`/embeddings` 后的 canonical full URL。两者都不保存或公开原 URL，credential value 和 secret reference identity
不进入 contract。canonical JSON 的 SHA-256 写入 control 和 instance DB。规则：

- embedding model 可由 `update()` 请求修改，但必须构造完整新 generation，成功后一次原子切换；旧 generation
  在切换前继续服务，失败或重启不会让查询混读两套向量；
- auth secret 内容 rotation 不改变 model contract；provider 映射、auth kind、base URL、adapter version、remote model、dimensions、metric、
  tokenizer 或 revision 发生变化时，旧实例 fail closed，不静默使用新 backend 查询旧向量；
- operator catalog contract drift 不会自动重解释实例；tenant 只有显式 update/full reindex 才能采用新的冻结合同；
- generation/rewrite/rerank model 可以更新，但缺少对应 model/protocol capability 时 update 直接失败；
- query embedding 必须与 active index generation 的 model contract 完全相同；
- multi-instance 只对 model-contract hash 相同的实例共享一次 query embedding。

## 8. Vectorize SQLite authority

### 8.1 文件与 pragma

每个 index 一个数据库：

```text
<data>/vectorize/<account>/<resource>/data.sqlite
```

使用与 KV/D1 相同的 owned-file、WAL、busy timeout、foreign key、quota、quick/integrity check 和在线 backup
规则。一个 index database 内包含 config、applied vectors、metadata materialization 和 mutation log。

### 8.2 schema

```sql
CREATE TABLE index_meta (
    singleton                 INTEGER PRIMARY KEY CHECK(singleton = 1),
    schema_version            INTEGER NOT NULL,
    resource_id               TEXT NOT NULL,
    dimensions                INTEGER NOT NULL,
    metric                    TEXT NOT NULL,
    vector_count              INTEGER NOT NULL CHECK(vector_count >= 0),
    next_mutation_sequence    INTEGER NOT NULL,
    processed_sequence        INTEGER NOT NULL,
    processed_mutation_id     TEXT,
    processed_at_ms           INTEGER
);

CREATE TABLE vectors (
    rowid                     INTEGER PRIMARY KEY,
    id                        TEXT NOT NULL UNIQUE,
    namespace                 TEXT,
    values_f32le              BLOB NOT NULL,
    norm                      REAL,
    metadata_json             BLOB,
    mutation_sequence         INTEGER NOT NULL,
    created_at_ms             INTEGER NOT NULL,
    updated_at_ms             INTEGER NOT NULL
);

CREATE INDEX vectors_by_namespace
ON vectors(namespace, rowid);

CREATE TABLE metadata_indexes (
    property_name             TEXT PRIMARY KEY,
    value_type                TEXT NOT NULL,
    state                     TEXT NOT NULL,
    generation                INTEGER NOT NULL,
    created_at_ms             INTEGER NOT NULL
);

CREATE TABLE metadata_terms (
    vector_rowid              INTEGER NOT NULL,
    property_name             TEXT NOT NULL,
    ordinal                   INTEGER NOT NULL,
    value_type                TEXT NOT NULL,
    text_value                TEXT,
    number_value              REAL,
    boolean_value             INTEGER,
    PRIMARY KEY(vector_rowid, property_name, ordinal),
    FOREIGN KEY(vector_rowid) REFERENCES vectors(rowid) ON DELETE CASCADE
);

CREATE INDEX metadata_terms_text
ON metadata_terms(property_name, text_value, vector_rowid);

CREATE INDEX metadata_terms_number
ON metadata_terms(property_name, number_value, vector_rowid);

CREATE INDEX metadata_terms_boolean
ON metadata_terms(property_name, boolean_value, vector_rowid);

CREATE TABLE vector_mutations (
    mutation_id               TEXT PRIMARY KEY,
    sequence                  INTEGER NOT NULL UNIQUE,
    kind                      TEXT NOT NULL,
    state                     TEXT NOT NULL,
    claim_token               BLOB,
    claim_until_ms            INTEGER,
    attempt                   INTEGER NOT NULL,
    next_attempt_at_ms        INTEGER NOT NULL,
    item_count                INTEGER NOT NULL,
    payload_bytes             INTEGER NOT NULL,
    error_code                TEXT,
    error_message             TEXT,
    created_at_ms             INTEGER NOT NULL,
    completed_at_ms           INTEGER
);

CREATE TABLE vector_mutation_items (
    mutation_id               TEXT NOT NULL,
    ordinal                   INTEGER NOT NULL,
    vector_id                 TEXT NOT NULL,
    namespace                 TEXT,
    values_f32le              BLOB,
    metadata_json             BLOB,
    PRIMARY KEY(mutation_id, ordinal),
    FOREIGN KEY(mutation_id) REFERENCES vector_mutations(mutation_id) ON DELETE CASCADE
);
```

上面的 SQL 只用于说明职责与事务域；实际 Day1 authority 是
[`015_vectorize.sql`](../../crates/storage/migrations/015_vectorize.sql) 与 `crates/storage/src/vectorize/` 的 schema、
repository 和 inspection，严格 `CHECK`、状态枚举、长度、foreign key、quota 与 cross-table invariant 以源码为准。

### 8.3 vector 编码

- 接受 JS `number[]`、`Float32Array` 和 `Float64Array`；
- authority 统一编码为 IEEE-754 little-endian f32 BLOB；
- dimensions 必须完全等于 index config；
- 每个分量在边界验证 finite，不能持久化 NaN/Infinity；
- BLOB 长度必须为 `dimensions * 4`，读取不进行截断、补齐或修复；
- cosine 可保存预计算 norm；其他 metric 不保存无用派生字段；
- exact query 始终可从原始 f32 重算；派生 cache/ANN 不是 authority。

### 8.4 mutation 语义

`insert()`、`upsert()` 和 `deleteByIds()` 都先 durable enqueue，再返回 `mutationId`：

```text
private request
  → validate complete batch
  → SQLite tx: allocate sequence + mutation + items
  → commit
  → signal coordinator
  → return mutationId
```

后台 apply：

```text
claim queued/expired mutation with token + lease
  → validate persisted payload again
  → one SQLite tx:
       apply vectors and metadata terms
       update vector_count
       advance processed pointer
       mark mutation applied
  → publish wake/metrics
```

要求：

- mutation 顺序按 index-local monotonic sequence apply；
- 后序 mutation 不能越过未终结的前序 mutation，否则 `describe().processedUpTo*` 无意义；
- `insert` 对已有 ID 保留旧记录，`upsert` 完整替换 values/namespace/metadata，不能 merge；
- 同一 batch 内重复 ID 按远端 differential 冻结为 first-wins；`insert` 已存在 ID 是成功 no-op，但 mutation
  frontier 仍按顺序推进；
- query/get 只看到 applied table，不读 queued payload；
- apply 已提交但 transport 丢失时，mutation 仍由 ID/sequence 幂等确认；
- permanent invalid persisted payload 标记 failed 并阻塞 processed frontier，不能跳过伪装成功；
- 已完成 mutation payload 按 operator retention 有界清理，但 mutation result/frontier 保留必要摘要。

## 9. Metadata index 与过滤

### 9.1 存储模型

完整 metadata 以 canonical JSON 保存在 `vectors.metadata_json`。只有 control plane 明确创建的 metadata
index 才物化到 `metadata_terms`。

Day1 支持：

```text
value: string | number | boolean | string[]
operators: implicit $eq, $eq, $ne, $lt, $lte, $gt, $gte, $in, $nin
multiple fields: implicit AND
nested field: dot path
```

当前 Cloudflare limits 显示每个 index 最多 10 个 metadata indexes、每个向量每个 indexed field 仅索引
前 64 UTF-8 bytes；这两项作为默认兼容上限，仍由本地 contract snapshot 固定。完整 metadata 的总上限
按当前官方 10 KiB 验证。

### 9.2 filter 编译

filter 先解析为闭合 Rust AST：

```text
FilterExpr
  └── Vec<FieldPredicate>   // implicit AND
        property path
        typed operator
        typed scalar/list
```

再生成参数化 SQL/temporary candidate set，禁止把 field/value 拼接进 SQL。执行顺序：

```text
namespace predicate
  → metadata index predicates
  → candidate rowids
  → vector scoring
  → topK
```

禁止先取全局 topK 再过滤。类型不匹配、未建索引字段、非法 dot path、过多 predicate、过长 list 和
非 finite number 都返回稳定错误。`$ne/$nin` 按远端 differential 包含缺失 indexed field 的记录。

## 10. Exact vector engine

### 10.1 Day1 实现

`crates/search` 提供：

```rust
DistanceMetric::{Cosine, Euclidean, DotProduct}
score(query, candidate)
top_k(iterator, k)
normalize_public_score(...)
```

实现策略：

- contiguous row-major f32；
- query 一次验证和预计算 norm；
- 使用 f64 accumulator 降低长向量累计误差，再按合同转换 public score；
- 固定大小 binary heap，内存为 `O(topK)`；
- 相同 score 用稳定 `(score, vector_id)` 顺序，避免 restart 后随机漂移；
- CPU 工作进入独立、固定大小 Rayon pool，外层使用 semaphore 做有界 admission；
- 不占用 Tokio async worker，也不使用无界 `spawn_blocking`；
- client disconnect 不被当作可靠 cancellation，扫描仍受 deadline 和 CPU budget 限制。

### 10.2 warm snapshot

```text
SQLite authority
  ├── cold path: bounded page read → score
  └── warm path: Arc<IndexSnapshot>
                 generation
                 contiguous vectors
                 rowid/id/namespace/norm mapping
```

一个 host-wide weighted LRU 管理 snapshot；当前进程级容量冻结为 512 MiB。
snapshot key 至少包含 resource ID、applied mutation sequence 和 metadata index generation。mutation apply 后
发布新 generation；旧 Arc 可服务已经开始的查询，不能原地修改。

如果单个 index 超过 warm budget，使用最多 100k candidate 的有界 cold scan 或 admission failure，不能让 RSS 无界增长。

### 10.3 ANN 决策 Gate

Day1 不使用 `sqlite-vec`：它当前仍是 pre-1.0，Rust 注册 SQLite extension 涉及 FFI/unsafe，而本项目禁止
workspace source 使用 `unsafe`；其当前价值也不足以替代本方案所需的 durable mutation/filter/recovery。
[sqlite-vec](https://docs.rs/crate/sqlite-vec/latest)

已完成 benchmark 支持默认 100k exact-only 路径，因此 P5 不引入 ANN。只有将来额度或目标主机预算变化时才评估
[USearch](https://docs.rs/usearch/latest/usearch/struct.Index.html)：

```text
SQLite vectors ──build──> derived HNSW/USearch file
                         可删除、可重建、不进 snapshot authority
```

ANN 必须满足：

- SQLite 原始 f32 和 mutation frontier 仍是 authority；
- ANN manifest 绑定 resource/generation/dimensions/metric/build params/digest；
- query candidate 需用原始 f32 exact rescore；
- 高选择性 metadata filter 可以回退 exact，以保持 pre-filter；
- corrupt/missing/stale ANN 只导致重建或 exact fallback，不改变持久状态；
- native dependency、release size、四平台构建和 MSRV 需要独立 Go；
- 不允许为了 ANN 放宽 single-binary 或 offline-startup 合同。

## 11. Vectorize Workers binding 与 private wire

### 11.1 对外支持面

Day1 目标是 pinned latest `Vectorize` class：

| 方法 | Day1 行为 |
| --- | --- |
| `describe()` | config/count/processed frontier |
| `query()` | metric + namespace + metadata pre-filter + topK |
| `queryById()` | 从 applied vector 取 query values 后走同一查询路径 |
| `insert()` | durable async mutation；已有 ID 不被覆盖 |
| `upsert()` | durable async mutation；完整替换 |
| `deleteByIds()` | durable async mutation |
| `getByIds()` | applied values/namespace/full metadata |

不实现 deprecated `VectorizeIndex` legacy class。公开 `topK`、return values/metadata、ID/namespace/metadata/dimensions
限制使用当前 Cloudflare 上限或更低且明确登记的单机 quota；不能广告 20M/index。

### 11.2 loader 注入

新增：

```text
packages/runtime/src/vectorize/facade.ts
packages/runtime/src/vectorize/transport.ts
packages/runtime/src/vectorize/protocol.ts
```

`RuntimeSource` 为每个 binding 传递：

```json
{
  "bindingId": "...",
  "resourceId": "...",
  "resourceGeneration": 4,
  "capabilityVersion": 1,
  "permissions": { "read": true, "write": true },
  "descriptorSha256": "..."
}
```

tenant 无法获得 internal URL/token/account ID/SQLite path。transport 继续携带 startup-generation token、
deployment ID、binding ID、resource generation 和 descriptor digest，由 `ocd` private backend 重新查 authority。

### 11.3 binary request frame

最大 Workers mutation 可包含 1000 × 1536 个 float；不能把所有 f32 先膨胀成 JSON decimal。内部协议使用
版本化 binary frame：

```text
magic + protocol version
canonical JSON header length + header
item count
for each item:
  id length + UTF-8 id
  namespace presence/length/value
  metadata JSON length/value
  dimensions
  dimensions × little-endian f32
```

Rust decoder必须：

- 先检查总 body、item count、各字段和乘法溢出；
- 不分配超过声明 hard limit 的 buffer；
- 拒绝 trailing bytes、duplicate fields、invalid UTF-8、invalid JSON、dimension mismatch 和 non-finite f32；
- 在完整 batch 验证成功前不写 mutation；
- 不把内部 decode error 或 payload 回显给 tenant。

查询响应数量较小，可以用有界 JSON；`returnValues=true` 时若 profiling 证明 JSON 成为瓶颈，再使用同一版本化
binary response，由 facade 还原公开对象，不能把私有 frame 暴露给 tenant。

## 12. AI Search instance authority

### 12.1 私有索引所有权

AI Search 复用 `crates/search` 算法与 vector 编码；item、chunk、FTS、vector 和 generation 在同一 instance SQLite 中提交。
内部向量表不作为公共 Vectorize resource 暴露，generation 激活、quota、删除、引用与备份由 AI Search authority 管理。

### 12.2 schema

```sql
CREATE TABLE instance_meta (
    singleton                   INTEGER PRIMARY KEY CHECK(singleton = 1),
    schema_version              INTEGER NOT NULL,
    resource_id                 TEXT NOT NULL,
    namespace_resource_id       TEXT NOT NULL,
    instance_key                TEXT NOT NULL,
    config_json                 BLOB NOT NULL,
    model_contract_json         BLOB NOT NULL,
    model_contract_sha256       BLOB NOT NULL,
    config_generation           INTEGER NOT NULL,
    active_index_generation     INTEGER,
    desired_index_generation    INTEGER NOT NULL,
    created_at_ms               INTEGER NOT NULL,
    updated_at_ms               INTEGER NOT NULL
);

CREATE TABLE items (
    item_id                     TEXT PRIMARY KEY,
    source                      TEXT NOT NULL,
    item_key                    TEXT NOT NULL,
    object_key                  TEXT NOT NULL,
    content_sha256              TEXT NOT NULL,
    content_type                TEXT NOT NULL,
    filename                    TEXT NOT NULL,
    size_bytes                  INTEGER NOT NULL,
    status                      TEXT NOT NULL,
    next_action                 TEXT,
    desired_generation          INTEGER NOT NULL,
    active_generation           INTEGER,
    metadata_json               BLOB,
    last_error_code             TEXT,
    last_error_message          TEXT,
    created_at_ms               INTEGER NOT NULL,
    updated_at_ms               INTEGER NOT NULL,
    UNIQUE(source, item_key)
);

CREATE TABLE item_generations (
    item_id                     TEXT NOT NULL,
    generation                  INTEGER NOT NULL,
    instance_index_generation   INTEGER NOT NULL,
    config_generation           INTEGER NOT NULL,
    content_sha256              TEXT NOT NULL,
    state                       TEXT NOT NULL,
    chunk_count                 INTEGER,
    created_at_ms               INTEGER NOT NULL,
    activated_at_ms             INTEGER,
    PRIMARY KEY(item_id, generation),
    FOREIGN KEY(item_id) REFERENCES items(item_id) ON DELETE CASCADE
);

CREATE TABLE chunks (
    rowid                       INTEGER PRIMARY KEY,
    chunk_id                    TEXT NOT NULL UNIQUE,
    item_id                     TEXT NOT NULL,
    item_generation             INTEGER NOT NULL,
    instance_index_generation   INTEGER NOT NULL,
    ordinal                     INTEGER NOT NULL,
    text                        TEXT NOT NULL,
    start_byte                  INTEGER NOT NULL,
    end_byte                    INTEGER NOT NULL,
    token_count                 INTEGER NOT NULL,
    values_f32le                BLOB,
    norm                        REAL,
    metadata_json               BLOB,
    UNIQUE(item_id, item_generation, ordinal)
);

CREATE VIRTUAL TABLE chunks_fts_porter USING fts5(
    chunk_id UNINDEXED,
    text,
    tokenize = 'porter unicode61'
);

CREATE VIRTUAL TABLE chunks_fts_trigram USING fts5(
    chunk_id UNINDEXED,
    text,
    tokenize = 'trigram'
);

CREATE TABLE index_jobs (
    job_id                      TEXT PRIMARY KEY,
    source                      TEXT NOT NULL,
    description                 TEXT,
    state                       TEXT NOT NULL,
    target_index_generation     INTEGER NOT NULL,
    claim_token                 BLOB,
    claim_until_ms              INTEGER,
    attempt                     INTEGER NOT NULL,
    next_attempt_at_ms          INTEGER NOT NULL,
    cancel_requested            INTEGER NOT NULL,
    created_at_ms               INTEGER NOT NULL,
    started_at_ms               INTEGER,
    ended_at_ms                 INTEGER,
    end_reason                  TEXT
);

CREATE TABLE index_job_items (
    job_id                      TEXT NOT NULL,
    item_id                     TEXT NOT NULL,
    target_item_generation      INTEGER NOT NULL,
    state                       TEXT NOT NULL,
    chunk_cursor                INTEGER NOT NULL,
    PRIMARY KEY(job_id, item_id)
);

CREATE TABLE item_logs (...);
CREATE TABLE job_logs (...);
CREATE TABLE ingest_intents (...);
CREATE TABLE object_gc (...);
```

实现使用同一 instance SQLite 中由平台显式维护的 FTS5 table；FTS row 与 chunk generation 在一个事务中写入并
由 inspection 检查，不依赖 trigger 静默修复。实际 authority 是
[`016_ai_search.sql`](../../crates/storage/migrations/016_ai_search.sql) 与 `crates/storage/src/ai_search/`，上面的 SQL
仅解释 ownership，不替代已落地 schema。

### 12.3 generation 规则

两类变化分开处理：

1. **单 item 内容变化**：为该 item 构建新 generation；成功后一个事务切换
   `items.active_generation`，其他 item 不受影响。
2. **index-affecting config 变化**：创建新的 instance index generation，重建全部 item；所有 item 完成后
   一个事务切换 `instance_meta.active_index_generation`，查询期间不能混合两套 tokenizer/chunk/model/index。

query-only 配置如 threshold/max results 可以随 config generation 原子生效；chunk size/overlap、keyword
tokenizer、index method、custom metadata materialization 和 embedding model update 都必须 fenced full reindex。

## 13. AI Search S3 对象与上传状态机

### 13.1 对象布局

AI Search 原始文件使用现有 system S3 authority，不放进 tenant R2 prefix：

```text
<system-prefix>ai-search/v1/<account-id>/<instance-resource-id>/objects/
  sha256/<first-two-hex>/<sha256>
```

对象不可变、content-addressed。DB 保存 exact key、SHA-256、size 和 content type。tenant 从
`item.download()` 获得 stream，但永远得不到 S3 credential、provider endpoint 或 signed URL。

### 13.2 upload 状态机

`items.upload(name, content, metadata)` 是同 source/key 的 upsert：

```text
1. validate name/metadata and reserve an ingest intent in SQLite
2. stream input through bounded staging while computing size + SHA-256
3. PUT deterministic immutable object to S3
4. SQLite tx:
     verify ingest intent/generation
     upsert item catalog
     enqueue index job/item generation
     mark intent committed
5. wake indexing coordinator
6. return queued item info
```

约束：

- 不在 SQLite transaction 内执行 S3 PUT；
- upload body 必须 streaming、限速/限长并支持 deadline；
- Day1 对齐当前 AI Search 4 MiB 单文件上限；
- crash 在 PUT 前：intent 可清理，无对象；
- crash 在 PUT 后、DB commit 前：startup reconcile 由 intent 精确 HEAD/delete；
- DB commit 后、wake 前：startup/periodic scan 会重新发现 job；
- 相同 digest PUT 幂等；
- 删除/替换只在 catalog 不再引用旧 object 后写入 `object_gc`；
- GC 使用 exact key 和 digest，不能 prefix-wide 猜测删除。

备份不要求内容保密，但仍必须校验完整性、路径、digest、size 和精确 ownership。

## 14. 文档解析与 chunking

### 14.1 Day1 文件类型

| 类型 | Day1 | 处理 |
| --- | --- | --- |
| UTF-8 plain text | 支持 | newline normalization |
| Markdown | 支持 | 保留结构化文本边界 |
| HTML | 支持 | `html2text`，禁止网络加载 |
| JSON | 支持 | bounded deterministic text projection |
| CSV | 支持 | bounded row/column projection |
| PDF/Office/ODF/Numbers | P5.7 | Xberg parser child；见独立方案 |
| image/scanned PDF OCR/audio/video | P5.8 | P5.7 返回 `OCR_REQUIRED` 或 unsupported，不隐式调用模型 |

所有 parser 必须限制解压/嵌套深度、输出文本 bytes、JSON nodes、CSV row/column、HTML nodes 和处理 deadline。
HTML parser 不执行 script、不请求子资源、不解析外部 entity。

### 14.2 Rust 选择

- [`text-splitter`](https://docs.rs/text-splitter/latest/text_splitter/)：递归按段落/句子/字符边界切分和 overlap；
- `html2text`：HTML 到受限文本；
- SQLite bundled FTS5：porter/trigram/BM25，[FTS5 文档](https://www.sqlite.org/fts5.html)；
- JSON 使用已有 `serde_json`；CSV 是否引入 `csv` crate 由 dependency Gate 决定。
- P5.7 当前正式 pin `xberg = "=1.0.14"`，仅启用 `tokio-runtime,pdf,office,excel,xml`，并通过
  [`ocd` 自派生 parser child](p5-7-xberg-document-parsing.md) 隔离 crash 和资源故障。

P5.0–P5.6 不引入 broad parser。P5.7 只在独立 dependency/corpus/process Gate 通过后引入 Xberg 最小 feature；
仍不引入 Kreuzberg v4、PDFium、ONNX Runtime、Tesseract、LibreOffice 或 OCR native/model stack。

### 14.3 Cloudflare Markdown Conversion API

Xberg 不是公开 API。P5.7 必须通过标准 Wrangler `[ai]` binding 暴露 pinned Cloudflare surface：

```text
env.AI.toMarkdown(file | files, options)
env.AI.toMarkdown().transform(file | files, options)
env.AI.toMarkdown().supported()
```

单文件/数组 overload、`MarkdownDocument { name, blob }`、`ConversionResponse` success/error discriminated union、
`output.format`、HTML/PDF 常用 options、错误边界和 supported 列表详见
[P5.7 API 合同](p5-7-xberg-document-parsing.md#23-markdown-conversion-workers-binding)。AI Search ingestion 继续
只通过 `instance.items.upload/uploadAndPoll/get/...` 暴露异步状态；不能增加 `parse()` 或返回 Xberg metadata。

该 `[ai]` binding 只宣称 Markdown Conversion subset。完整 Workers AI inference 与 Cloudflare `/client/v4`
Markdown Conversion REST 不在 P5.7 compatibility 分母，必须登记 deviation。

### 14.4 token 与 chunk contract

Cloudflare 使用 recursive chunking：优先自然段/句子，再按 token limit 继续拆分，overlap 为 0%–30%。本方案
采用相同模型，但不承诺相同私有 parser/tokenizer 产生逐字节相同边界。

规则：

- `chunk_size` 是 token 数，不是 UTF-8 bytes；
- model catalog 必须声明一个已知 tokenizer/revision；
- tokenizer identity 进入 model contract hash；
- 不允许未知模型静默改用字符数；如果 operator 显式声明 approximate tokenizer，必须登记 deviation；
- chunk 先归一化文本，再记录 `start_byte/end_byte`；offset 指向归一化后的 UTF-8 文本，不冒充原始 PDF/HTML bytes；
- chunk ID 由 instance/item/content/config/generation/ordinal 的 canonical digest 生成；
- overlap 最大 30%，chunk 不能超过 embedding model max input；
- 空文档、全空白文档和 parser 无有效文本的状态/error shape由 differential 固定。

## 15. Embedding 与 indexing coordinator

### 15.1 provider client

Day1 先实现 OpenAI-compatible：

```text
POST <base_url>/embeddings
POST <base_url>/chat/completions
```

实现复用 workspace 的 Hyper 1.x、hyper-util、rustls/aws-lc、webpki roots 与 `hyper-rustls 0.27.9`，没有
引入第二套 Reqwest/TLS stack。本机 Rust 1.98 与 license/build 检查属于本轮验收，四平台留在 release acceptance。

embedding batch 响应必须验证：

- HTTP/content-type/response bytes；
- output 数量、index/order 与 input 一致；
- exact dimensions；
- 所有值 finite；
- model alias/contract 未在请求期间变更；
- retry-after 和 transient/permanent error 分类；
- provider body/header/error 在 tenant 响应和日志中脱敏。

### 15.2 本地 OpenAI `/v1/embeddings` fixture

本地开发使用 Hugging Face Text Embeddings Inference（TEI）作为真实模型 fixture。TEI 由 Rust/Candle
实现，在 Apple Silicon 上使用 Metal，并提供 OpenAI-compatible `POST /v1/embeddings`。它只用于开发和
provider contract 验证，不进入 `ocd`、发行包、生产启动依赖或平台 snapshot。

本 fixture 固定为：

```text
server                  text-embeddings-inference 1.9.3
model repository        Qwen/Qwen3-Embedding-0.6B
model revision          97b0c614be4d77ee51c0cef4e5f07c00f9eb65b3
served model            @cf/qwen/qwen3-embedding-0.6b
dimensions              1024
metric                  cosine
Cloudflare input limit  8192 tokens
listen                   127.0.0.1:8080
development cache       .temp/tei-hf/
```

模型 alias、dimensions、metric 和 Cloudflare input limit 来自当前
[AI Search supported models](https://developers.cloudflare.com/ai-search/configuration/models/supported-models/)；
TEI 自己报告的更大 input limit 不扩大 open-compute 对外合同。

Apple Silicon 安装：

```sh
brew install text-embeddings-inference
text-embeddings-router --version
```

第一次开发启动允许显式下载；缓存必须留在仓库统一忽略的 `.temp/`，不能进入源码、snapshot 或正式测试
输入：

```sh
mkdir -p .temp/tei-hf
HF_HOME="$(pwd)/.temp/tei-hf" \
  text-embeddings-router \
  --model-id Qwen/Qwen3-Embedding-0.6B \
  --revision 97b0c614be4d77ee51c0cef4e5f07c00f9eb65b3 \
  --served-model-name '@cf/qwen/qwen3-embedding-0.6b' \
  --hostname 127.0.0.1 \
  --port 8080 \
  --api-key local-test-key \
  --max-client-batch-size 32
```

只有日志出现 `Starting HTTP server: 127.0.0.1:8080` 和 `Ready` 后才算 fixture ready。进程存在、权重下载
完成或 backend 开始 warm-up 都不能替代 readiness。首次运行可能下载约 1.1 GiB 权重；正式 Gate 必须先准备
并校验固定 revision 的模型缓存，再在断网环境启动，禁止把下载成功算作 offline startup 通过。

先验证 Bearer boundary：

```sh
curl -sS -o /dev/null -w '%{http_code}\n' \
  http://127.0.0.1:8080/v1/embeddings \
  -H 'Content-Type: application/json' \
  -d '{"model":"@cf/qwen/qwen3-embedding-0.6b","input":"test"}'
```

预期为 `401`。再验证固定 revision 和 server bounds：

```sh
curl -sS http://127.0.0.1:8080/info \
  -H 'Authorization: Bearer local-test-key' \
  | jq -e '
      .model_id == "Qwen/Qwen3-Embedding-0.6B" and
      .model_sha == "97b0c614be4d77ee51c0cef4e5f07c00f9eb65b3" and
      .max_client_batch_size == 32
    '
```

标准 embedding 请求只调用 `/v1/embeddings`，不允许测试代码改用 TEI 私有 `/embed`：

```sh
curl -sS http://127.0.0.1:8080/v1/embeddings \
  -H 'Authorization: Bearer local-test-key' \
  -H 'Content-Type: application/json' \
  -d '{
    "model":"@cf/qwen/qwen3-embedding-0.6b",
    "input":[
      "Cloudflare Workers 是什么？",
      "SQLite 是一个嵌入式数据库。"
    ]
  }' \
  | jq -e '
      .object == "list" and
      .model == "@cf/qwen/qwen3-embedding-0.6b" and
      (.data | length) == 2 and
      .data[0].index == 0 and .data[1].index == 1 and
      (.data | all(
        .object == "embedding" and
        (.embedding | length) == 1024 and
        (.embedding | all(type == "number"))
      )) and
      .usage.prompt_tokens > 0 and
      .usage.total_tokens == .usage.prompt_tokens
    '
```

2026-09-02 的本地探索实际取得：TEI `1.9.3` 在 Metal 上 ready；无凭据请求为 `401`；两个字符串的批量
请求返回 `object=list`、有序 index `0/1`、两个 `object=embedding`、每个 1024 维、served model 精确一致，
并返回非零 `usage`。这只是 provider fixture 的探索证据，不是 P5.3 Gate、Cloudflare differential 或产品
实现完成证据。

P5.3 的 maintained fixture 还必须覆盖：

- `input` 为单个 string 和 string array；Day1 不声明 token-ID array；
- `encoding_format=float` 的默认/显式行为；`base64` 只有实测并实现后才进入支持面；
- 请求 model 不匹配、空 input、过量 batch、超长文本和 body ceiling；
- response 缺字段、乱序/重复 index、错误 dimensions、NaN/Infinity 和超大 body；
- `401`、`400`、`413`、`429` + `Retry-After`、`5xx`、timeout、disconnect；
- 同一 frozen input 的重试维持顺序和 model contract，但不要求跨 backend 浮点 bytes 完全相同；
- provider 不可达时 `ocd` 仍可 offline startup ready，只有依赖该 provider 的操作失败或降级；
- provider fixture 进程、端口和 `.temp/tei-hf/` 由测试 harness 精确拥有并在结束时回收。

测试日志不得记录 API key、input 文本或完整 embedding。正式 fixture key 由 harness 临时生成；这里的
`local-test-key` 仅用于手工 loopback 开发，不能进入生产配置。

### 15.3 coordinator 所有权

新增 `SearchCoordinator`，参考现有 scheduler 的 wake/claim/lease/fence/backoff 模式，但 job authority 保留在
各 index/instance DB；不要把 vector mutation 或 AI indexing table 塞入 `scheduler.sqlite`，也不要调用租户
Queue/Workflow。

```text
control.sqlite enumerate ready resources
  → bounded per-resource due frontier
  → in-memory wake heap + notification
  → claim token/lease in resource DB
  → async S3/provider work outside tx
  → short SQLite progress/activation tx
  → bounded safety reconciliation
```

避免为每个资源创建永久 Tokio timer。一个全局 coordinator 维护有限 frontier，使用游标公平轮询 account/resource。
Vector mutation 和 AI indexing 使用独立 pool/admission，避免大批 embedding 阻塞小型 vector mutation。

### 15.4 indexing 状态机

```text
queued
  → claimed
  → fetching/parsing
  → chunked
  → embedding batches
  → keyword/vector staged
  → activating
  → completed

transient failure → retry_wait → claimed
permanent failure → error
cancel request → cancelling → cancelled
stale config/item generation → outdated
```

每个阶段保存 deterministic progress。embedding batch 成功后写入该 generation 的 staged chunk rows；crash 后
从首个未完成 batch 继续，不要求 provider 幂等。提交 batch 前重新检查 claim token、item generation、instance
config generation 和 cancel flag；旧 worker 的结果必须被 fence。

`cancel()` 不能保证中断已经发出的 provider HTTP，但可以设置 durable cancel flag；返回后旧调用结果即使到达也
不能激活 generation。

## 16. AI Search retrieval pipeline

### 16.1 单实例 search

```text
1. authorize binding + resolve active instance generation
2. validate exactly one of query/messages
3. optional query rewrite
4. tokenize keyword query and/or generate query embedding
5. run vector exact search and FTS5 BM25 in parallel
6. apply metadata filter before ranking in both branches
7. fuse vector + keyword candidates
8. optional rerank bounded candidate window
9. context expansion 0..3 adjacent chunks
10. threshold/max-results/metadata-only projection
11. return search_query + chunks + scoring_details
```

请求开始后 pin active generation。并发 config/item activation 产生新 generation，不改变已经开始的查询。

### 16.2 FTS5 query

- `porter` 和 `trigram` 各使用固定 FTS table；
- `keyword_match_mode=and|or` 在 Rust 中 tokenize/quote 后构造；
- 用户文本不能直接作为 FTS5 query syntax；
- 禁止 tenant 注入 column selector、NEAR、wildcard 或其他非声明语法；
- FTS5 `bm25()` 原始分值只用于排序/内部 normalization；公开 score 映射到 0–1；
- sort tie 使用稳定 chunk ID；
- keyword tokenizer 或 `index_method` 变化触发 full reindex，与 Cloudflare 当前文档一致。

### 16.3 fusion

默认 RRF：

```text
rrf_score = Σ 1 / (K + rank)
```

`K` 冻结为本地 retrieval parameter `60`；它不是 Cloudflare 公开合同，也不在 API 中伪称托管内部值。

`max` 需要先把两个 branch 归一到 0–1：

- vector metric 使用经 differential 固定的 public score normalization；
- BM25 使用单调、有限、有界转换；
- 缺失 branch 不贡献 score；
- `scoring_details` 返回本地 branch score/rank/fusion method；
- 不以 exact score parity 作为 Cloudflare compatibility Gate，但相同 corpus 的 threshold/order invariant 必须稳定。

### 16.4 rerank、rewrite 与 chat

- `rewrite_query=true`：调用 catalog 中 rewrite/chat-capable model，结果作为 `search_query`；
- `reranking=true`：调用显式 rerank adapter，对有限 candidate window 重排；
- provider/catalog 不支持时 create/update 失败，不能继续用 vector score 假装 rerank；
- `chatCompletions()` 从检索结果构造有 token budget 的 context，再调用 generation provider；
- `stream=true` 以 SSE `ReadableStream` relay，限制 event/header/body 和总 deadline；
- disconnect 不等于 provider 已取消；平台仍需停止向 tenant 写入，并按 bounded drain 处理 upstream；
- provider 原始错误、prompt、credential、内部 context 不进入平台日志；tenant 获得稳定、脱敏错误。

### 16.5 multi-instance

namespace `search/chatCompletions`：

- `instance_ids` 1–10；
- 每个 ID 必须属于绑定 namespace/account；
- duplicate ID 在边界稳定拒绝；missing/deleting/unavailable instance 按 `return_on_failure` 合同整体失败或进入
  实例级脱敏 `errors`；
- 按 model-contract hash 分组，同组 query embedding 只调用一次；
- fan-out 受全局/namespace/request concurrency cap；
- 每个 instance pin 各自 active generation；
- merge 后 chunk 增加 `instance_id`；
- `errors` 只记录允许 partial failure 的实例级脱敏错误；
- `return_on_failure=false` 时按合同 fail whole request，不能返回看似完整的部分结果。

## 17. Worker API 支持矩阵

### 17.1 Namespace binding

| 方法 | Day1 |
| --- | --- |
| `get(name)` | 返回 scoped instance facade；实际调用时再 authorize/not-found |
| `list(params)` | 支持 page/per_page/search/created_at ordering |
| `create(config)` | 支持 built-in storage；在绑定 namespace 内创建 child |
| `delete(name)` | delete fence、job cancel、object GC、tombstone |
| `search(params)` | 1–10 个 instance bounded fan-out |
| `chatCompletions(params)` | non-stream 与 SSE stream |

### 17.2 Instance binding

| 方法 | Day1 |
| --- | --- |
| `search()` | vector/keyword/hybrid |
| `chatCompletions()` | provider 已配置时支持 non-stream/stream |
| `update()` | query-only immediate；index-affecting full generation rebuild |
| `info()` | canonical config/status/model aliases，不泄露 provider secret |
| `stats()` | item status、vector dimensions/count、S3 logical bytes/count |
| `items` | list/upload/uploadAndPoll/get/delete |
| `jobs` | list/create/get；info/logs/cancel |

### 17.3 Item binding

| 方法 | Day1 |
| --- | --- |
| `info()` | durable item state |
| `download()` | S3-backed bounded ReadableStream |
| `sync()` | built-in item reindex |
| `logs()` | cursor pagination，脱敏、有界 retention |
| `chunks()` | active generation 的 offset pagination |

### 17.4 Config 支持

首个 Go 支持：

```text
id
rewrite_query / reranking
embedding_model / ai_search_model / rewrite_model / reranking_model
index_method.vector / index_method.keyword
fusion_method
indexing_options.keyword_tokenizer
retrieval_options.keyword_match_mode
chunk / chunk_size / chunk_overlap
score_threshold / max_num_results
custom_metadata
metadata
```

`embedding_model` 省略时解析 operator-pinned `ai.default_embedding_model`；提供时必须命中 catalog。
`update({ embedding_model })` 走 fenced full-generation reindex，与托管差分中接受 update 的行为一致。只启用
keyword index 且明确关闭 vector 时可以不解析 embedding contract；默认 vector index 必须在 create 时就验证
model/provider 配置完整，不能等首次 upload 才失败。

首个 Go 明确拒绝并登记 deviation：

```text
type=r2
type=web-crawler
source / source_params / sync_interval
token_id / ai_gateway_id
semantic cache / cache_threshold
website/R2 continuous sync
```

`boost_by` 与 custom prompt 当前未进入本地支持面，提交时稳定拒绝，不能忽略。由于 upstream AI Search 仍是
beta，所有 `[key: string]: unknown` 扩展字段都按 unsupported 拒绝，
不会未经声明透传给 provider。

### 17.5 Markdown Conversion binding

| 方法/字段 | P5.7 |
| --- | --- |
| `[ai] binding = "AI"` | platform-provided immutable builtin binding |
| `env.AI.toMarkdown(file)` | 单对象输入/输出 |
| `env.AI.toMarkdown(files)` | 数组输入、同序数组输出 |
| `env.AI.toMarkdown().transform()` | 与 direct overload 相同 |
| `env.AI.toMarkdown().supported()` | 只列已通过本地 Gate 的 extension/MIME |
| `conversionOptions.output/html/pdf` | 支持 P5.7 明确列出的常用字段 |
| image/OCR conversion options | P5.8，P5.7 fail closed |
| `AI.run/models/gateway/batch` | 不在本阶段，稳定 unsupported/deviation |

`ConversionResponse`、error mapping、mixed-success batch、token estimate 和 Cloudflare differential 以
[P5.7 文档解析方案](p5-7-xberg-document-parsing.md)为唯一 owner，Xberg 类型或错误不能进入 facade。

## 18. 限制与 admission

### 18.1 Vectorize 兼容 hard limits

当前 Cloudflare limits 可作为协议 hard ceiling：

```text
dimensions                  32..1536, float32
vector ID                   <= 64 UTF-8 bytes
namespace                   <= 64 UTF-8 bytes
metadata                    <= 10 KiB/vector
metadata indexes            <= 10/index
indexed string bytes        <= first 64 UTF-8 bytes
Workers mutation batch      <= 1000
topK                        <= 100
topK with values/all meta   <= 50
```

本地资源 quota 在创建时冻结：默认 100k vectors/index，schema ceiling 200k，并同时执行逻辑 bytes quota；
不沿用 Cloudflare 20M 作为本地承诺。

### 18.2 AI Search hard limits

首个 Go：

```text
AI Search item file          <= 4 MiB
toMarkdown file              <= 4 MiB
toMarkdown batch             <= 16 files / 32 MiB
toMarkdown output            <= 16 MiB
parser deadline/address      30s / 2 GiB
parser CPU/stderr            30 CPU seconds / 64 KiB
multi-instance request       <= 10 instances
custom metadata fields       <= 5
context expansion            0..3
max search results           1..50
chunk overlap                0..30%
```

另需本地：

- items/instance；
- extracted text bytes/file；
- chunks/item 和 chunks/instance；
- vectors/instance 和 local DB bytes；
- indexing jobs/instance、queued bytes、logs retention；
- provider concurrency/request bytes/response bytes；
- exact search CPU time、candidate count、warm cache bytes；
- simultaneous upload/download/SSE streams。

所有额度均在 authority boundary 原子检查。global、account、resource 三层 admission 顺序固定；任何队列都必须
有最大长度或 bytes，不能以“后台任务”名义无界积压。

## 19. Backup、restore 与 GC

### 19.1 snapshot authority 扩展

当前 snapshot manifest 只识别 control/scheduler/KV/D1/DO。P5 必须直接扩展 Day1 current schema：

```text
SnapshotFileRole::VectorizeSqlite
SnapshotFileRole::AiSearchSqlite
SnapshotImmutableReference role: ai_search_object
source_schemas: vectorize, ai_search
excluded_local_state: 增加 vector_search_cache / ann_cache（若存在）
```

snapshot 流程：

1. 持有 data-dir exclusive ownership；
2. 暂停新 mutation/job claim，bounded drain；
3. 在线 backup 每个 ready Vectorize/AI Search SQLite；
4. 从 instance DB 枚举被 active/catalog 引用的 S3 immutable objects；
5. HEAD 校验 exact key/size/digest，写 immutable reference；
6. 不备份 warm snapshot、provider cache 或 ANN derived files；
7. restore 到 fresh data-dir 后先校验所有 DB/object，再原子安装；
8. 启动 coordinator 前执行 cross-database/resource inspection。

由于 AI Search 原始对象不是重复拷贝进 snapshot，retention/GC 必须把所有 committed snapshot manifest 的引用
纳入 pin set。snapshot 删除后才允许回收最后一个 live/snapshot reference。

### 19.2 删除与 GC

- resource delete 先 fence 新调用和 job claim；
- 等待/过期回收 active claim；
- instance/index local DB 通过精确 staging/trash 收敛；
- AI Search S3 object 先写 durable GC intent，再逐个 exact delete；
- S3 删除失败保持 `deleting/degraded`，可恢复重试；
- tombstone 前必须证明 local path 和 owned S3 objects 已按策略收敛；
- 不进行 account-wide 或 prefix-wide 模糊删除；
- support bundle 不列文件正文、向量值、prompt 或 metadata，只给计数/bytes/frontier/error code。

## 20. Security 与故障语义

### 20.1 tenant 隔离

- namespace/instance/index/item/job 所有 lookup 都携带 account/resource identity；
- binding authorization 重新检查 immutable deployment、descriptor digest、resource generation 和 permissions；
- tenant 输入不能选择 internal endpoint、S3 key、SQLite path、provider credential 或 arbitrary model URL；
- direct instance facade 与 namespace-derived facade 使用同一 private authorization kernel；
- `get(name)` 返回对象不代表已授权，实际方法调用仍 fail closed；
- mutation/job ID 是 opaque identity，不可被用于跨实例枚举。

### 20.2 输入与资源攻击面

- f32、dimension、metadata JSON、filter AST、FTS query、filename/content type 全部边界验证；
- parser 对 zip bomb/HTML explosion/JSON nesting/CSV width/输出文本有硬限制；
- provider response 与 tenant request 同样不可信；
- query candidate、rerank window、context bytes、SSE event 数量有界；
- FTS 只接收平台构造的表达式；
- SQLite read/write deadline 与 progress handler 不能被单个 query 绕过；
- exact vector scan 使用独立 CPU pool，避免耗尽 async runtime；
- 所有持久状态读取拒绝 corrupt enum/digest/length，不进行 silent repair。

### 20.3 错误分类

至少区分：

```text
invalid request / unsupported option
binding permission/type/generation mismatch
resource not ready/deleting/unavailable
quota/admission exhausted
provider unavailable/timeout/invalid response
mutation queued/failed/frontier blocked
index job cancelled/outdated/permanent error
storage/S3 integrity failure
search deadline exceeded
```

公开错误映射到 pinned Cloudflare shape；内部日志记录 request/resource/job/mutation ID 和稳定 error code，但不记录
tenant content、vector values、metadata、query、prompt、provider body 或 secret。

## 21. Health、metrics、doctor 与 operator API

### 21.1 health

组件：

```text
vectorize_storage
vectorize_mutations
ai_search_storage
ai_search_indexing
ai_model:<alias>
```

provider 未显式 probe 不应阻止 `ocd` startup ready；但使用该 provider 的实例可以 degraded/unavailable。DB
corruption、frontier blocked 或 S3 authority mismatch 必须 fail closed，并影响对应 product readiness/availability。

### 21.2 metrics

低基数指标：

```text
vector mutations queued/claimed/applied/failed/expired
vector query duration/candidates/scored/cache-hit/deadline
vector DB bytes/count and warm-cache bytes/evictions
AI jobs by state/retry/cancel/outdated
parse/chunk/embed/index stage duration
provider requests by capability/outcome, batch inputs, response bytes
AI search duration by retrieval type and outcome
FTS/vector/rerank candidate counts
S3 upload/download/GC outcome
```

label 只能包含 product、stage、metric、outcome、model/protocol 等 operator-bounded enum；不能使用 account/resource/item/query
作为 Prometheus label。

### 21.3 doctor/operator

`doctor` 默认离线检查：

- control/resource/path/schema/invariant；
- per-DB quick check 和 mutation/job frontier；
- FTS5 availability；
- S3 object catalog 的 bounded sample 或显式全检；
- model contract 与当前 catalog 是否一致；
- derived cache 是否 stale（不自动修复）。

只有显式 provider probe 才联网。Operator API 提供 resource/job/mutation inspect、pause/resume/reconcile 和精确
repair/GC 操作；不能暴露 vector values、document body、prompt、credential 或内部 claim token。

## 22. 依赖维护

版本与 feature authority 是根 [`Cargo.toml`](../../Cargo.toml) 和 [`Cargo.lock`](../../Cargo.lock)。
搜索使用 SQLite／FTS5、safe Rust vector math、Rayon、固定 tokenizer 与受控 HTTPS provider；
文档解析依赖及离线约束见 [P5.7](p5-7-xberg-document-parsing.md)。
依赖升级仍需验证 tokenizer/model identity、最小 feature、license、离线行为与声明的平台范围。

## 23. 实施与验收依据

P5.0–P5.7 已进入唯一 production path，实际执行和限制见[完成记录](p5-vectorize-ai-search-results.md)。
跨平台、完整 parser process matrix 与 hosted rich-document 资格见[独立验收计划](../acceptance/p5-release-acceptance.md)。

## 24. 关键测试与故障矩阵

### 24.1 Vectorize

```text
create: after control row / after directory / after schema / before ready
delete: with binding / with mutation / after fence / after local move / restart
mutation: before enqueue commit / after commit before wake / during claim
          after vector apply before response / lease expiry / stale token
query: mutation visibility / filter-before-topK / deadline / warm generation swap
corruption: BLOB length / NaN / metadata term mismatch / frontier gap / stale cache
```

### 24.2 AI Search

```text
upload: before S3 / during stream / after PUT before DB / after enqueue before wake
model:  embedding omitted/default/explicit/invalid / create-vs-update / provider/auth missing
        dimensions/tokenizer/revision drift / credential rotation / restart contract pin
parse: malformed/oversized/empty/HTML expansion/JSON depth/CSV width
       PDF/OOXML/ODF/Numbers corpus / encrypted / truncated / OCR-required
       parser child abort/timeout/OOM/invalid frame/orphan cleanup
api:   toMarkdown direct/handle single/array overload / supported / options
       mixed success / per-file error / whole-call rejection / stale descriptor
embed: partial batch / wrong count/order/dim / NaN / timeout / 429 / 5xx
index: after staged chunks / after FTS / before activation / stale config / cancel
query: provider failure / FTS escaping / hybrid branch failure / rerank failure
       context expansion boundary / multi-instance partial failure / SSE disconnect
delete/GC: live item / snapshot-pinned object / S3 failure / restart
restore: missing object / digest mismatch / model contract mismatch / derived cache absent
```

每个 crash point 必须证明：

- 没有半个 generation 对查询可见；
- 旧 claim/provider result 不能提交；
- startup 不依赖未持久化 wake；
- 重试不会重复激活或错误计数；
- 删除只影响精确 resource/object；
- support bundle/log/error 不泄露内容或 secret。

## 25. Benchmark 与 Go 结论

P5.0 在 Apple M2 Max（12 cores、32 GiB）完成 release-mode exact-search benchmark；数字是本机工程证据，
不是对所有部署主机承诺的托管 SLA：

| vectors | 384d p95 | 768d p95 | 1024d p95 | 1536d p95 |
| ---: | ---: | ---: | ---: | ---: |
| 10k | 8 ms | 17 ms | 23 ms | 35 ms |
| 50k | 39 ms | 83 ms | 112 ms | 175 ms |
| 100k | 81 ms | 175 ms | 256 ms | 367 ms |
| 250k | 214 ms | 447 ms | 640 ms | 967 ms |

内存/过滤/并发补充证据：

- snapshot：100k × 768/1024/1536 分别为 308.1/410.5/615.3 MiB；250k × 1536 为 1.538 GiB；
- 100k × 768 metadata selectivity 1%/10%/100% 分别为 3/18/181 ms，证明 pre-filter 控制 scored candidates；
- 同一 snapshot concurrency 1/4/16 的 wall time 为 172/178/451 ms，Rayon pool 与 admission 保持有界；
- 隔离的 100k × 1024 run：p95 227 ms、max RSS 415,809,536 bytes、snapshot 410.5 MiB。

因此 Day1 选择 exact-only，默认 quota 100k vectors/index、512 MiB host-wide warm cache；250k 只保留为
schema ceiling/扩容证据之外的压力点，不作为默认承诺。mutation、upload、provider retry、hybrid ordering 的有界性
由 focused lifecycle/fixture Gate 覆盖。

ANN 仅在以下任一成立时进入 P5.8：

- 声明的默认 vectors/index 下 warm p95 无法达到预算；
- exact scan 消耗导致其他产品 readiness/latency 超出 Gate；
- 目标额度提高到 exact search 明显不可行的数量级。

## 26. 完成与后续资格边界

本地核心归档需满足：

- supported/deviation matrix 覆盖 pinned Vectorize、AI Search 和 Markdown Conversion type members；
- control/resource/binding/toolchain/runtime/private backend 全链路实现；
- SQLite/S3 authority、snapshot/restore、delete/GC 和 crash recovery 通过；
- stock pinned workerd 执行真实 tenant Worker，不使用 Miniflare/mocked runtime 替代；
- provider fixture 使用真实 HTTP process、streaming 和 fault injection；
- P5.7 在 stock workerd 中以标准 `[ai]` binding 通过 `toMarkdown` direct/handle API，不暴露 Xberg 私有 API；
- P5.7 advertised rich format 均有固定公开 fixture、parser child 隔离和多语言 retrieval 证据；
- Cloudflare differential 验证 API shape、限制、mutation/filter/retrieval 高风险行为；
- exact search benchmark 支撑公开单机 quota，或条件式 ANN 已单独 Go；
- no-default-features、Rust 1.98、dependency boundaries、coverage 和一轮最终 Gate 通过；
- capability catalog、deviation reference、operator docs、single-binary/snapshot docs 和总架构同步；
- 文档移入 `docs/implemented/` 并附实际 revision、输入、case count、coverage、Gate report 和已接受限制。

四平台 release build/size/offline startup、完整 parser crash/abort/OOM/orphan/soak、
托管 Markdown Conversion rich-document differential 和正式发布签名不冒充本轮本机证据，统一留在 active release
acceptance。它们不反向创建旧实现兼容路径，也不削弱本地 resource、authority、runtime 和 recovery Gate。
