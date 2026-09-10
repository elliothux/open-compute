# P5.2：AI provider backend 与 embedding profile

状态：**implemented（2026-09-11）**。本方案解决
[#41](https://github.com/elliothux/open-compute/issues/41)，并直接替换当前把 provider、固定 `/v1` root、操作路由和
embedding 模型事实混在一起的配置模型。

## 用户结果

- OpenAI-compatible endpoint 可以包含任意合法路径前缀；open-compute 使用 operator 配置的完整 endpoint URL，不再覆盖路径或拼接固定路由。
- Embeddings 与 chat completions 分别引用 operation-specific backend，同一供应商可以为不同操作使用不同 host、路径、凭据或静态 metadata headers。
- 每个 embedding 模型只配置 backend、远端模型名和一个可复用 `profile`；维度、输入上限与 tokenizer 不再在 model mapping 中重复填写，cosine metric 不进入配置。
- profile 由 operator 显式定义并可被多个模型复用，冻结精确维度、tokenizer revision／artifact digest 和输入上限；open-compute 不隐式猜测模型事实。
- 未配置任何 backend 仍是合法的离线部署；配置加载完整校验 backend/model/profile 引用和 artifact 声明，`ocd` 组合 AI Search 服务时读取并校验 tokenizer bytes。provider 暂时不可达只影响实际调用，启动不探测 provider、不下载模型或 tokenizer。

## 当前问题

当前 `[ai.providers.*].base_url` 必须是 canonical `/v1` root，随后 client 分别拼接 `/embeddings` 和
`/chat/completions`。这种模型存在四个问题：

1. `https://provider.example/compatible-mode/v1` 之类带前缀的 root 会被当前校验拒绝；endpoint 派生逻辑也会用 `/v1` 覆盖原 path，无法接入 #41 的 provider。
2. 一个 provider 同时承担供应商身份、认证、协议和多个操作地址，无法表达 embeddings/chat 位于不同地址的常见部署。
3. embedding model 配置要求重复填写维度、metric、token 上限、tokenizer family/revision/path/SHA；其中 metric 实际只能是 cosine。
4. embedding alias 仍受硬编码 Cloudflare model 表约束，新增兼容模型必须修改生产代码，operator catalog 并不真正通用。

只放宽 `base_url` 的 path 校验仍然保留操作路由拼接和多操作耦合，因此不是本阶段的最终模型。

## 权责模型

| 层级 | 负责 | 不负责 |
| --- | --- | --- |
| Backend | 一种固定 wire protocol、完整 endpoint URL、认证方式、受限静态 headers | 模型维度、tokenizer、索引 metric、tenant alias |
| Embedding profile | 维度、输入 token 上限、精确 tokenizer、是否发送 dimensions 参数 | endpoint、secret、远端模型名 |
| Model mapping | tenant-visible alias 到 backend、remote model、profile 的映射 | transport 实现、tokenizer bytes |
| Resolved contract | 展开并冻结所有影响 chunk/embed/index 的非 secret 事实 | secret value、并发、timeout 等运行策略 |

`backend` 是一次具体操作的调用目标，不是供应商账户的抽象。协议使用闭集；P5.2 首先提供
`openai_embeddings_v1` 和 `openai_chat_completions_v1`，不引入任意 JSON template、脚本或动态 adapter。

## 配置合同

### Backend

```toml
[ai.backends.bailian-embeddings]
protocol = "openai_embeddings_v1"
endpoint = "https://dashscope.aliyuncs.com/compatible-mode/v1/embeddings"
auth = { kind = "bearer", secret = { env = "DASHSCOPE_API_KEY" } }

[ai.backends.bailian-chat]
protocol = "openai_chat_completions_v1"
endpoint = "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions"
auth = { kind = "bearer", secret = { env = "DASHSCOPE_API_KEY" } }
headers = { "HTTP-Referer" = "https://open-compute.dev", "X-Title" = "open-compute" }
```

`endpoint` 是最终请求 URL。adapter 不追加、删除或替换 path segment。URL 必须 canonical；禁止 userinfo、query 和 fragment，
非 loopback 地址必须使用 HTTPS，`auth = { kind = "none" }` 仍只允许 loopback HTTP。redirect 不跟随。

认证使用闭集：

```toml
# 标准 Authorization: Bearer
auth = { kind = "bearer", secret = { env = "PROVIDER_API_KEY" } }

# 使用自定义 header 发送一个 API key
auth = { kind = "header", name = "X-API-Key", secret = { file = "/run/secrets/provider-api-key" } }

# 只允许 loopback HTTP
auth = { kind = "none" }
```

`headers` 只承载非敏感静态 metadata。secret 不允许通过 `${ENV}`、模板或普通 header value 注入，必须走现有 env/file
`SecretReference`。P5.2 不支持任意多个 secret headers；真实 provider 出现这种需求后，再增加语义和泄漏边界明确的 auth kind。

Header 在配置加载时统一校验：

- 名称按 ASCII lowercase 比较并拒绝重复；数量、名称和值都有硬上限，值禁止换行和控制字符；
- `Authorization` 只由 `auth.kind = "bearer"` 生成，普通 `headers` 和 `auth.kind = "header"` 均不得使用它；自定义 auth header name 也不能与普通 header 冲突；
- `Host`、`Content-Length`、`Content-Type`、`Accept`、`User-Agent`、`Cookie`、`Connection`、`Transfer-Encoding`、`Upgrade`、`TE`、`Trailer` 和 proxy/hop-by-hop headers 由平台拥有或禁止，operator 不能覆盖；
- adapter 先生成平台拥有的 request headers，再加入已经验证的认证与静态 headers，不接受 tenant request 提供 backend headers；
- secret value 永不进入 config serialization、日志、错误、metrics、doctor、support bundle 或持久化 contract。

### Embedding profile 与 model mapping

```toml
[ai.embedding_profiles."qwen/qwen3-1024"]
dimensions = 1024
max_input_tokens = 8192
send_dimensions = true
tokenizer = { kind = "qwen3", revision = "97b0c614be4d77ee51c0cef4e5f07c00f9eb65b3", artifact = { path = "/opt/open-compute/tokenizers/qwen3/tokenizer.json", sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" } }

[ai.embedding_models."@cf/qwen/qwen3-embedding-0.6b"]
backend = "bailian-embeddings"
remote_model = "text-embedding-v4"
profile = "qwen/qwen3-1024"
```

`remote_model` 是请求体实际发送的值；`profile` 是 open-compute 对 embedding 行为的本地、不可变描述。二者不要求同名，
同一 profile 可以被不同 provider 的 model mapping 复用。

profile 不区分 built-in 与 custom，也没有保留名字空间。这样既保留复用能力，又不让单二进制为一组不断变化且有独立 license 的
模型资源兜底。operator 必须显式提供 digest-pinned 的本地 tokenizer artifact；启动和请求路径都不下载 tokenizer。新增 provider
通常只需复用现有 profile 并增加 model mapping，只有模型的 embedding 行为确实不同时才新增 profile。

另一个 profile 示例：

```toml
[ai.embedding_profiles."company/embed-v2"]
dimensions = 768
max_input_tokens = 4096
send_dimensions = false
tokenizer = { kind = "custom", revision = "2026-08-01", artifact = { path = "/opt/open-compute/tokenizers/embed-v2.json", sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" } }

[ai.embedding_models."company/embed-v2"]
backend = "internal-embeddings"
remote_model = "embed-v2"
profile = "company/embed-v2"
provider_revision = "2026-08-01"
```

每个 profile 必须声明维度、平台采用的最大输入 token 数和 digest-pinned offline tokenizer。`send_dimensions` 默认为
`false`；为 `true` 时 adapter 发送与 `dimensions` 相同的数值，不再保留两个可能冲突的数字字段。

`provider_revision` 只在上游确实提供不可变 revision 时填写，不能要求 operator 编造一个无法验证的版本字符串。它存在时进入
resolved contract。配置解析不通过请求一个样本文本来推断维度或 revision；首次响应也不能成为持久数据契约。

### Generation model

Generation mapping 改为引用声明了 `openai_chat_completions_v1` 的 backend：

```toml
[ai.generation_models."company/chat"]
backend = "bailian-chat"
remote_model = "qwen-plus"
max_context_tokens = 32768
capabilities = ["chat", "rewrite", "rerank"]
```

Generation 不使用 embedding profile。`max_context_tokens` 与 capabilities 继续属于 model mapping；它们不能进入 backend，
因为同一 endpoint 可以承载多个能力不同的模型。

## 维度、tokenizer 与 metric

- **Dimensions 必须进入 resolved contract。** provider response、持久向量 bytes 和索引 generation 都依赖精确维度。它在可复用
  profile 中只填写一次。
- **Tokenizer 必须进入 resolved contract。** tokenizer 改变 chunk 边界及重建结果。profile 必须提供 revision 和 artifact SHA，
  持久化时只记录 revision/digest，不记录 host path。
- **AI Search metric 不进入用户配置。** P5.2 固定 cosine，并在 resolved contract 中显式记录。当前单值 `metric = "cosine"`
  是配置噪声，应删除。独立 Vectorize index 的 cosine/euclidean/dot-product 仍是公开索引配置，不受本方案影响。
- **Max input tokens 属于 profile。** 它是 open-compute 采用的安全上限，不必等于供应商宣传的理论 context window；configured
  chunk size 仍不得超过该值。

## 响应兼容边界

OpenAI-compatible 表示请求和核心响应结构兼容，不表示返回 JSON 必须逐字段等于 OpenAI 当前实现。adapter 应当：

- 忽略未知响应字段，但继续限制总 response bytes、集合大小和字符串大小；
- 对 embeddings 强制检查 item count、唯一且完整的 index、有限浮点数和 profile dimensions；
- 不再要求 response `model` 与请求的 `remote_model` 字节完全相等；该字段允许缺失或由 provider canonicalize，但存在时必须是
  bounded non-empty string；
- 对 chat/non-stream/SSE 继续检查 choices、index、role/content、终止标记、deadline 和 malformed frame；
- 保留现有 unauthorized、rate-limit、transient、permanent、timeout 和 malformed-response 稳定错误分类，不返回上游 body。

这些兼容放宽不允许接受维度漂移、NaN/Infinity、重复 index、不完整 SSE、redirect 或超限响应。

## 冻结、更新与失败语义

创建或更新 AI Search instance 时，service 把 backend 与 profile 展开成一个 secret-free resolved contract。至少冻结：

- backend protocol、canonical endpoint digest、auth kind／自定义 auth header name；
- 按 lowercase 排序的静态 header names 及 values digest；
- remote model 与可选 provider revision；
- profile identity/definition digest、dimensions、cosine、max input tokens、`send_dimensions`；
- tokenizer kind、revision 和 artifact SHA。

endpoint、protocol、auth kind/name、静态 header 或 remote model、provider revision、任一 profile 事实改变时，model contract
digest 必须改变，并通过现有 generation fence 完整 reindex 后再激活。secret value 轮换、timeout、并发和请求池大小不改变
embedding 语义，不触发 reindex。静态 header 可能参与上游 deployment/version routing，因此其值只存摘要，但变化仍保守地视为语义变化。

配置加载同步验证所有 backend/model/profile 引用和 tokenizer artifact 声明；服务组合时验证本地 tokenizer bytes。缺失、digest mismatch、未知 profile、protocol/operation
不匹配或旧 contract shape 一律 fail closed；不保留 `[ai.providers]`、`base_url`、隐式 path append、旧字段 alias 或双读路径。
已经发布的 SQLite migration bytes 保持不变；若实现需要 schema 变化，只追加下一条 migration。已有旧 contract 数据不被静默修复或
猜测，operator 必须用新配置显式启动一次受 generation fence 保护的重建。

## 实施范围

- 用 `backends` 替换 `providers`，让 core config、path/secret resolution、header validation、doctor、health、support bundle 和示例共同使用一个模型。
- 增加 operator embedding profile registry，删除 Cloudflare alias switch、公开单值 metric 及重复
  `request_dimensions` 数值。
- 让 embeddings/chat client 直接使用 backend endpoint 和协议；收敛共同的 bounded HTTP/auth/status 逻辑，不增加 pass-through
  wrapper。
- 更新 AI Search contract hashing、tokenizer registry、reindex 判定和 restart validation；移除 superseded 类型、字段和测试 fixture。
- 更新 operator 配置文档、默认配置注释、Cloudflare compatibility/deviation 描述及 #41 的验收证据。

## 非目标

- 任意 request/response transform、用户脚本、动态 module 或自定义 JSON template；
- 自动模型发现、启动时 capability probe、tokenizer/model 下载；
- provider fallback、负载均衡、跨 provider retry、成本路由或 multi-region control plane；
- Azure-specific query authentication、多个 secret headers、tenant-controlled headers 或非 OpenAI-compatible adapter；
- 改变独立 Vectorize 的公开 metric 支持面。

## 验收结果

- Config tests 已覆盖任意合法 path prefix、embeddings/chat 独立 endpoint、canonical URL、HTTPS/loopback、三种 auth、静态 header bounds/reserved-name/case-insensitive collision，以及所有 backend/model/profile 引用失败。
- Profile 与持久化 tests 已覆盖复用展开、tokenizer digest、维度/input bounds、`send_dimensions`、稳定 contract hash、旧 shape 拒绝和内嵌 digest 防伪造。
- Provider process tests 已证明请求命中配置的完整 path；额外 JSON 字段、canonicalized response model 和乱序 item 可接受，维度漂移、非有限向量、重复 item、超限 body、redirect 与 malformed SSE 继续失败。
- Bearer/custom-header credential 只从引用解析，不进入 resolved contract 或 support bundle；静态 header value 只以摘要进入持久 contract，secret reference 轮换不触发 reindex。
- 使用本地 operator credential 对阿里云百炼 `qwen3.7-text-embedding-flash` 做了真实 OpenAI-compatible 请求，返回并通过 1024 个有限浮点维度校验；凭据与响应正文未写入输出或仓库。
- Format、Clippy、no-default-features、Rust 1.98 MSRV、metadata、dependency boundary、文档构建均通过；workspace Rust line coverage 为 90.03%，随后一次完整未插桩 workspace Gate 通过。
