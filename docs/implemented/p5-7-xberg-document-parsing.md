# P5.7：基于 Xberg 的 PDF/Office 文档解析

> 状态：**Local core implementation archived / release acceptance active**，2026-09-02。本文记录
> [P5 Vectorize 与 AI Search](p5-vectorize-ai-search.md) 已落地的文档解析阶段；本机验收数据见
> [完成记录](p5-vectorize-ai-search-results.md)，四平台发行、timing-three、完整 parser process fault/soak 与
> 托管 rich-document differential 见仍在维护的[发行验收计划](../p5-release-acceptance.md)。

当前实现已经落地 `crates/document-parser`、`ocd __document-parser-v1` 隐藏 child、OCDP v1 frame、
标准 `[ai]` builtin、`env.AI.toMarkdown()` direct/handle overload、13 种实际 admission、40-file 固定公开
corpus 与 15 个 hostile fixtures。正式 pin 是 `xberg = "=1.0.14"`，crate SHA-256
`68568d75a993709564cb27361409b46988ec585f9fb59c8f91a113ff7f6b4e29`，最小 features 为
`tokio-runtime,pdf,office,excel,xml`，parser contract 是
`19decbaa581fb83acd9c35d489da8a1ba0e66a0336aa7dfc5b6b5eb00421a8dd`。固定 40-file corpus manifest
SHA-256 为 `599efa6fb8d5ae4517c1a62034bf4db69af7c152b77421709be5362882be31c1`；15-case hostile manifest
SHA-256 为 `c02d5091a29c5e411181593074e7b707ceb2126ae68d304b5bab1a8b9b65d542`。

Items → parse → chunk → embed → FTS/vector activation → retrieval/recovery 已接入 P5 的 durable state machine；
HTML `hostname`/受限 `cssSelector` 只做离线 normalization。XLSB 在固定 corpus 触发 calamine abort 风险、
Numbers 在 Xberg 1.0.14 固定样本上解析失败、ET 无合格 fixture，三者明确 No-Go，不出现在 `supported()`。

## 1. 结论

P5.7 采用 **Xberg 最小 Rust feature 集 + `ocd` 自派生的短生命周期 parser 子进程**，把本地通过 Gate 的
PDF、Microsoft Office 与 OpenDocument 文件转换成规范化
Markdown，再交给平台已有的 chunk、embedding、FTS 与 vector pipeline。

核心决策如下：

1. 采用活跃维护的 [Xberg](https://github.com/xberg-io/xberg)，不为新实现引入已进入 LTS 的
   [Kreuzberg v4](https://github.com/kreuzberg-dev/kreuzberg-lts)。
2. 用户仍然只安装和运行一个 `ocd`。解析任务由 `ocd` 启动自身的隐藏内部模式完成，不增加第二个 daemon、
   容器、Python、Java、LibreOffice 或常驻 parser 服务。
3. Xberg 不进入 `ocd` 的主服务进程。解析器 panic、abort、超时或内存失控只能终止当前 parser child，不能
   杀死 Workers、Queue、Workflow 或 indexing coordinator。
4. Xberg 和 parser wire 都是内部实现，不能成为 tenant API。Worker 只看到 Cloudflare 的 AI Search Items API
   和 Markdown Conversion `env.AI.toMarkdown()` API；chunking、tokenizer、embedding、FTS、vector、
   generation activation 继续由 open-compute 拥有。
5. Day1 不启用 OCR、PDFium、ONNX、Candle、Paddle、Tesseract、layout model、embedding、LLM、URL ingestion
   或 archive recursion。扫描 PDF 产生内部 `DOCUMENT_OCR_REQUIRED`，再映射为对应 CF API 的公开错误；OCR
   和其他条件式扩展属于 P5.8。
6. 公开格式范围由 Cloudflare 当前合同决定，不因为 Xberg 能解析更多格式就自动扩大。PPT/PPTX、RTF、ODP、
   DOC 和 EPUB 等 Xberg 能力默认仍返回 `UNSUPPORTED_CONTENT_TYPE`。

这仍是单机 Cloudflare Worker Platform 兼容能力，不实现 Cloudflare 的内部解析器，也不承诺逐字节相同的
Markdown。必须对齐的是 Worker binding 名称、方法、overload、输入/输出 shape、异步索引状态、失败边界、
可搜索内容和公开错误行为；不得另外发明 `XbergBinding`、`parseDocument()` 或 tenant 可见 parser protocol。

## 2. Cloudflare 合同边界

截至 2026-09-02，Cloudflare AI Search 对 rich formats 的公开列表来自
[AI Search data source](https://developers.cloudflare.com/ai-search/configuration/data-source/) 和
[Workers AI Markdown Conversion supported formats](https://developers.cloudflare.com/workers-ai/features/markdown-conversion/supported-formats/)。
AI Search 对单文件的公开上限为 4 MB，并使用 Markdown Conversion 处理 rich formats。

P5.7 只扩展 P5.3 已有的文本、Markdown、HTML、XML、JSON 和 CSV parser，目标矩阵为：

| 格式 | 扩展名 | P5.7 状态 | Xberg 路径 | 备注 |
| --- | --- | --- | --- | --- |
| PDF | `.pdf` | 支持 text-layer PDF | `pdf-native` | 扫描件只检测，不 OCR |
| Word OOXML | `.docx` | 支持 | `office` | 不支持 legacy `.doc` |
| Excel OOXML | `.xlsx`、`.xlsm` | 支持 | `excel` | 不执行 VBA，不索引宏源码 |
| Excel Binary | `.xlsb` | No-Go | `excel` | 固定 fixture 可触发依赖 abort 风险，不进入 admission |
| Excel legacy | `.xls` | 支持 | `excel` | OLE/BIFF，只读解析 |
| WPS Spreadsheet | `.et` | No-Go | 无 | 无真实、许可明确 fixture，不进入 admission |
| OpenDocument | `.odt`、`.ods` | 支持 | `office` / `excel` | ZIP/XML expansion 必须有界 |
| Apple Numbers | `.numbers` | No-Go | 禁用 `iwork` | Xberg 1.0.14 固定 fixture 解析失败 |
| 图片/扫描 PDF | 当前 Cloudflare 支持 | P5.7 不支持 | 无 | P5.8 OCR；返回明确错误 |

本地 `supported()` 的 rich formats 只包含 PDF、DOCX、XLSX/XLSM/XLS 与 ODT/ODS；再加已有
TXT/Markdown/HTML/XML/JSON/CSV，共 13 个扩展。No-Go 格式稳定 fail closed，不能根据扩展名伪装成功。

Cloudflare 当前没有把 PPT/PPTX、RTF、ODP 或 legacy DOC 列入 AI Search rich formats。即使 Xberg 的
`office` feature 能解析它们，P5.7 public admission 也必须拒绝，以避免无依据扩大兼容面和攻击面。

### 2.1 公开 API 的兼容原则

P5.7 不设计新的文档解析产品 API。兼容分母固定为以下两个 Worker-facing surface：

1. AI Search 的 `ai_search_namespaces` / `ai_search` binding 与 Items API；
2. Workers AI binding 中仅与 Markdown Conversion 有关的 `env.AI.toMarkdown` surface。

合同来源按以下顺序冻结：

1. 本仓库 pinned workerd 的
   [`ai-search.d.ts`](../../references/workerd/types/defines/ai-search.d.ts)、
   [`to-markdown.d.ts`](../../references/workerd/types/defines/to-markdown.d.ts) 和
   [`to-markdown-api.ts`](../../references/workerd/src/cloudflare/internal/to-markdown-api.ts) facade source；
2. Cloudflare 当前的
   [AI Search Items Workers binding](https://developers.cloudflare.com/ai-search/api/items/workers-binding/) 与
   [Markdown Conversion Workers binding](https://developers.cloudflare.com/workers-ai/features/markdown-conversion/usage/binding/) 文档；
3. 对真实 Cloudflare 的 source-compatible differential。

文档、类型和托管行为出现冲突时，本轮逐成员矩阵与 differential 冻结唯一结论。例如当前 AI Search 文档把
`items.upload()` 二进制输入写为 `ArrayBuffer`，pinned 类型写为 `Blob`；本地 runtime 可以接受实测成立的兼容
并集，但生成的 Env 类型和 capability 不能悄悄改成另一套私有签名。

Xberg crate API、内部 `OCDP` frame、parser contract hash、资源限制和错误 code 都不属于这个公开 surface。
tenant bundle 不能 import Xberg，不能调用 parser child，也不能看到 S3 key、临时路径或内部 parse metadata。

### 2.2 AI Search Items API

P5.7 不改变 P5.4/P5.5 已定义的 AI Search facade，只扩充它能异步索引的文件类型。以下 Cloudflare 调用必须继续
保持原方法名、参数、返回值和 polling 行为：

```ts
const instance = env.AI_SEARCH.get("docs");

const queued = await instance.items.upload("manual.pdf", content, {
  metadata: { language: "zh" },
});

const completed = await instance.items.uploadAndPoll(
  "manual.docx",
  content,
  { pollIntervalMs: 1000, timeoutMs: 30000 },
);

const item = instance.items.get(queued.id);
await item.info();
await item.logs();
await item.chunks();
await item.download();
```

具体合同：

- `items.upload()` 仍然在 durable ingest intent 和 index job 提交后立即返回 `AiSearchItemInfo`，不能等待 Xberg；
- `items.uploadAndPoll()` 只是 Cloudflare-compatible client-side polling facade，不另建同步解析状态机；
- 解析过程只通过 `queued | running | completed | error | skipped | outdated`、`error`、item logs 和 chunks 可观察；
- `download()` 返回原始文件，不返回规范化 Markdown；`chunks()` 返回 active generation 的公开 chunk shape；
- 同名 upload 保持 upsert 语义，失败 generation 不能覆盖前一份已激活索引；
- parser 的 `DOCUMENT_OCR_REQUIRED`、`DOCUMENT_ENCRYPTED` 等内部 code 必须映射成实测冻结的 item status、
  `error` 和 `errorType`，不能给 AI Search API 增加自定义字段；
- `ArrayBuffer`、`Blob`、`ReadableStream`、`string` 的接受矩阵由 official docs、pinned type 和远端 differential
  冻结，所有 stream 都必须有 4 MiB hard limit 和 deadline。

### 2.3 Markdown Conversion Workers binding

P5.7 增加标准 Wrangler AI binding 声明，不增加 `xberg` 私有 binding：

```toml
[ai]
binding = "AI"
```

它是 platform-provided builtin binding，不创建 tenant resource、SQLite 或 S3 object。Day1 只宣称该 binding 的
Markdown Conversion 子集兼容；`run()`、`models()`、AI Gateway、batch 和其他完整 Workers AI inference surface
仍是明确 deviation，不能因为存在 `env.AI` 就标记为 Workers AI supported。

必须实现 pinned overload：

```ts
type MarkdownDocument = { name: string; blob: Blob };

env.AI.toMarkdown(document, options): Promise<ConversionResponse>;
env.AI.toMarkdown(documents, options): Promise<ConversionResponse[]>;

const converter = env.AI.toMarkdown();
converter.transform(document, options): Promise<ConversionResponse>;
converter.transform(documents, options): Promise<ConversionResponse[]>;
converter.supported(): Promise<Array<{ extension: string; mimeType: string }>>;
```

单文件输入必须返回单对象，数组输入必须返回同序数组，即使数组长度是 1；不能统一包成数组或自定义 envelope。
成功和单文件转换失败分别使用 Cloudflare shape：

```ts
type ConversionResponse =
  | {
      id: string;
      name: string;
      mimeType: string;
      format: "markdown" | "text";
      tokens: number;
      data: string;
    }
  | {
      id: string;
      name: string;
      mimeType: string;
      format: "error";
      error: string;
    };
```

`name` 原样对应输入逻辑名但必须经过长度/字符校验，不能解释成路径；`mimeType` 是检测结果；`id` 只承诺
Cloudflare-compatible opaque string，不承诺与托管端相同；`tokens` 使用冻结 tokenizer/estimator 并记录 score
parity deviation，不允许返回 bytes 或 Unicode scalar 数冒充 token。

`supported()` 只返回当前实例真正通过 Gate 的 extension/MIME，排序和重复处理必须 deterministic。P5.7 未实现图片
转换，因此不能照抄 Cloudflare 列表把图片报成 supported；这项差异进入 capability/deviation 输出。

### 2.4 Conversion options 与错误

P5.7 支持以下当前公开的
[Conversion options](https://developers.cloudflare.com/workers-ai/features/markdown-conversion/conversion-options/) 常用子集：

| `conversionOptions` | P5.7 行为 |
| --- | --- |
| `output.format: "markdown" | "text"` | 支持；响应 `format` 与内容一致 |
| `html.hostname` | 支持，仅用于规范化相对链接，不发起网络请求 |
| `html.cssSelector` | 支持受限 selector；非法/过复杂 selector fail closed |
| `pdf.metadata` | 支持，控制公开结果中是否包含允许的 PDF metadata |
| `image.descriptionLanguage` | P5.8；P5.7 不接受 |
| `html/docx/pdf.images.convert`、`maxConvertedImages` | P5.8；P5.7 不接受 |

`gateway` 属于未实现的 AI Gateway surface，P5.7 必须返回稳定 unsupported error，不能静默忽略；
`extraHeaders` 不能成为任意 upstream header/SSRF 通道。pinned 类型中存在但官方文档未列出的字段，同样先进入
differential matrix，不凭类型的 `[key: string]: unknown` 自动接受。

错误分两层：

- 合法请求中的某个文件无法转换，返回该文件的 `{ format: "error", error }`，批量中其他文件仍按顺序返回；
- binding transport、frame、权限或整个请求 validation 失败，Promise 以 pinned Cloudflare error class/消息边界
  reject，不能返回 HTTP/SQLite/Xberg 私有错误对象。

具体哪些错误属于 per-file、哪些 reject whole call，必须覆盖 mixed-success batch、空数组、重复文件名、错误 MIME、
超限、encrypted、truncated、OCR-required、child abort 和 deadline 的 Cloudflare differential。

### 2.5 REST 与管理面边界

P5.7 的 compatibility verdict 是 tenant Worker API。Cloudflare 的
`/client/v4/accounts/{account_id}/ai/tomarkdown` 与 `/supported` REST API、账号 API token/permission 和通用
`/client/v4` envelope 仍不属于当前单机 runtime compatibility 分母。不得用自有 Operator API 冒充它们，也不得
为了 REST 再实现一套 parser。若以后纳入，必须作为 P5.8 的显式 data-plane adapter，复用同一 parser service，
并单独解决 account-scoped auth、multipart limits、response envelope 和 public-listener Gate。

## 3. 为什么选 Xberg

Kreuzberg v4 已明确进入 legacy/LTS，新的功能开发迁往 Xberg。Xberg 当前 workspace 使用 Rust 2024、
声明 Rust 1.92 MSRV、MIT license，并将 `pdf-native`、`pdf-pdfium`、`office`、`excel`、`iwork`、OCR 和模型
能力拆成 Cargo features；正式 pin 时已结合 crates.io source 与
[workspace Cargo.toml](https://github.com/xberg-io/xberg/blob/main/Cargo.toml)、
[xberg Cargo.toml](https://github.com/xberg-io/xberg/blob/main/crates/xberg/Cargo.toml) 交叉确认。

P5.7.0 的 POC 候选原为 `=1.1.0`；crates.io 实际可审计并通过当前 corpus 的正式 Day1 pin 是：

```toml
xberg = {
    version = "=1.0.14",
    default-features = false,
    features = [
        "tokio-runtime",
        "pdf",
        "office",
        "excel",
        "xml",
    ],
}
```

该 pin/checksum/feature set 已成为当前 parser contract；任何升级仍必须重新冻结完整
`cargo tree -e features`、license inventory 和二进制增量。不能依赖 Xberg `main` 的移动 revision，也不能
因为其 workspace 中存在 path patch 就假定 crates.io 包含同样修复；如果后续发布包未通过 fixture，
应等待修复版本或审查后显式 vendor patch，不能在生产构建时临时拉 Git HEAD。

以下 features 明确禁用：

```text
default/full/formats/server/api/mcp
pdf-pdfium
ocr/bundle-tessdata-eng/tesseract-dynamic
paddle-ocr*/candle-*/sceptre-ocr*/layout-detection
liter-llm/embedding*/retrieval*/url-ingestion/archives/tree-sitter
```

最小 feature 图必须证明：

- build 和普通测试不下载 PDFium、模型、tessdata、字体或其他 artifact；
- production startup 和 parse request 完全离线；
- 不出现 `hf-hub`、ORT、Tesseract、PDFium 或第二套 HTTP/TLS client；
- Rust 1.98、`--no-default-features`、Linux x64/arm64 和 macOS x64/arm64 都可构建；
- Xberg 及 transitive dependency 中的 native build、unsafe、license 和 security advisory 全部有 inventory；
- release binary/压缩包增量经过实测并由 P5.7.0 冻结 Go 阈值。

Xberg 自己的 `unsafe_code = "deny"` 不等于整棵依赖树没有 unsafe。open-compute workspace 的
`unsafe_code = "forbid"` 也不会约束第三方 crate，因此 parser 仍按不可信输入边界设计。

## 4. 进程模型

### 4.1 同一发行文件，不在主进程解析

生产中不增加公开子命令。`ocd` 内部以受控参数启动自身：

```text
ocd
  ├── pinned workerd child
  └── ocd __document-parser-v1       # 每个文档一个短生命周期 child
```

`__document-parser-v1`：

- 不出现在 help、operator API 或 tenant 可见能力中；
- 只从 stdin 接收一个版本化 frame，只向 stdout 返回一个版本化 frame；
- 不打开 control/data plane listener，不初始化 workerd、SQLite、S3、provider 或 master key；
- 使用清空后的环境和独立短生命周期工作目录；
- argv、环境、stdout/stderr 不携带文件正文、S3 credential、provider secret、SQLite path 或 internal token；
- 父进程拥有 spawn、deadline、stdout/stderr 上限、process-group kill、reap 和残留清理。

这会增加一个瞬时 OS process，但不增加第二个服务或数据 owner。平台进程模型、health、metrics、doctor 与
support bundle 已明确区分常驻 workerd child 和瞬时 parser child。

### 4.2 为什么不能直接调用 library

当前 release profile 使用 `panic = "abort"`。主进程内的 parser panic 或 native abort 会终止整个平台；
`catch_unwind` 不能接住 abort。文档解析又天然包含压缩包、字体、图片、XML、OLE 和损坏二进制等高风险输入。

子进程隔离主要提供 crash 和资源故障边界，不虚构成完整 kernel sandbox。parser child 仍运行在同一 OS 用户下；
P5.7 必须做到无网络代码路径、无 secret 输入、无数据目录路径和最小依赖面。Linux seccomp/Landlock、macOS
sandbox 等额外 confinement 只有在四平台 Gate 证明稳定后才可启用；没有证据时文档和 capability 不能声称
“内核级沙箱”。

### 4.3 并发与生命周期

Day1 默认每个文件启动一个新 child，不复用进程：

- indexing 本来就是异步流程，优先隔离而不是节省几毫秒 startup；
- 任意 parser 全局状态、allocator 碎片和前一个文档残留随进程退出清空；
- 全局 `document_parser.max_concurrency=4` semaphore 控制并发，另有 account=2、deployment=1 的默认上限；
- 同一 account 和 instance 另有公平队列，不能让一个 tenant 占满全部 parser slot；
- shutdown 先停止 admission，再等待有界 grace，随后杀死并 reap 全部 parser process group。

只有 benchmark 证明 spawn 成本成为主瓶颈，且复用后的 RSS、状态泄漏和 crash corpus 均通过时，才可在 P5.8
评估固定 worker pool；P5.7 不保留两套运行模式。

## 5. parser wire contract

### 5.1 输入 frame

父进程从 S3 读取已经通过 4 MiB admission 的 immutable object，并向 child 写入：

```text
magic "OCDP"
protocol_version u16
header_length u32
body_length u32
canonical JSON header
raw document bytes
```

header 只允许：

```json
{
  "request_id": "opaque-local-id",
  "filename": "manual.pdf",
  "declared_content_type": "application/pdf",
  "content_sha256": "...",
  "parser_contract_sha256": "...",
  "html_options": null
}
```

`html_options` 仅允许 HTML 请求携带，字段为可选 `hostname` 与 `css_selector`。`hostname` 只作为 HTTP(S)
base 解析相对链接，绝不触发网络请求；`css_selector` 经过长度、group 数、语法、遍历节点和输出字节上限后，
仅保留匹配节点。非 HTML 请求携带该字段、非法 URL/selector、未知 option 均 fail closed。

decoder 必须先校验 magic/version/长度/溢出，再分配 body；拒绝 trailing bytes、重复 JSON fields、无效 UTF-8、
digest mismatch 和超限 header/body。filename 仅参与诊断和格式交叉验证，不能当作文件路径。

### 5.2 输出 frame

child 成功返回：

```json
{
  "version": 1,
  "format": "pdf",
  "detected_content_type": "application/pdf",
  "markdown": "...",
  "markdown_sha256": "...",
  "page_count": 12,
  "sheet_count": null,
  "metadata": {},
  "warnings": [],
  "parser_contract_sha256": "..."
}
```

规则：

- Markdown 是 UTF-8、LF newline、NFC normalization 后的内容；
- 不允许 NUL、控制字符、非 finite number、重复 metadata key 或未知顶层字段；
- metadata 采用闭合 allowlist，默认只保留 title/author/subject/language/page/sheet 等非正文属性；
- warning 使用稳定 code，不透传 Xberg debug string、路径、原始 XML、宏、附件或文档正文；
- 父进程重新计算 Markdown digest，并检查输出总长度和 parser contract；
- parser 不生成 chunk ID、token count、embedding、vector 或索引 generation。

解析失败也返回同一协议的结构化 error；child exit code、signal、超时和无效 frame 由父进程映射成稳定平台错误。

### 5.3 parser contract hash

parser contract 至少覆盖：

```text
xberg crate version + Cargo checksum
enabled feature set
open-compute adapter/normalizer version
format allowlist
security/output limits
format-specific options
```

`item_generations` 增加 `parser_contract_sha256`、`normalized_content_sha256` 和有界 parse summary。parser 或
normalizer 变化属于 index-affecting config change，必须创建新的 instance index generation，不能让同一个 active
generation 混用两套解析合同。

## 6. 格式识别与内容语义

### 6.1 不信任扩展名和 Content-Type

admission 依次检查：

1. upload/R2 object 的 4 MiB 大小与 SHA-256；
2. extension 和 declared MIME 是否在 Cloudflare-compatible allowlist；
3. magic/container signature；
4. 对 ZIP/OLE 容器做有界结构识别；
5. Xberg 实际检测结果与平台 allowlist 是否一致。

ZIP-based OOXML、ODF、Numbers 不能只看 `PK`：必须有界检查 `[Content_Types].xml`、ODF `mimetype` 或 iWork
container marker。OLE2 `.xls` 需检查 `Workbook`/`Book` stream；`.et` 不在公开 admission。扩展、MIME、magic
冲突统一 fail closed，不自动按另一个格式“猜成功”。

### 6.2 格式特定规则

- PDF：按 reading order 输出 text layer；页分隔转换为稳定结构边界；脚注、表格和多栏质量由 fixture 衡量。
- DOCX/ODT：保留 heading、list、table、link、footnote/endnote 的文本语义；图片只保留无模型的 alt text。
- XLS/XLSX/XLSM/ODS：按 sheet 顺序输出标题和 Markdown table；使用 cached/display value，
  不计算公式、不执行宏、不访问 external link。
- embedded object/attachment：P5.7 不递归创建 child items，也不从文档中发起网络或文件请求；只记录有界 warning。
- password/encryption：不接受密码参数；返回 `DOCUMENT_ENCRYPTED`。
- empty document：返回 `DOCUMENT_EMPTY`，不创建空 embedding 或空 active generation。
- scanned PDF：text layer 为空或低于冻结的 page/text-density 判据时返回 `DOCUMENT_OCR_REQUIRED`；不能把空白
  Markdown 当作成功。

表格宽度、最大 sheet/page、公式/单元格展示和 PDF metadata 已由 parser contract、fixture 与 output bounds
固定。Cloudflare 没公开的内部行为不猜测为兼容合同。

## 7. 资源与安全限制

所有限制在 allocation/decompression 前检查，且同时存在 parent hard limit 与 Xberg parser limit：

| 资源 | P5.7 要求 |
| --- | --- |
| AI Search item bytes | 对齐公开 4 MiB hard limit |
| `toMarkdown` bytes/batch | 4 MiB/file、16 files、32 MiB/batch |
| 输出 Markdown | 16 MiB/request |
| wall deadline | 30 s；父进程硬杀并 reap |
| CPU/RSS | 30 CPU seconds；Linux 以 `RLIMIT_AS` 执行 2 GiB address-space hard limit；Darwin 拒绝设置 `RLIMIT_AS`，macOS RSS hard limit 留在发行验收 |
| ZIP/XML/OLE | uncompressed bytes、entry/node/depth、ratio、string/shared-string、relationship 数均有界 |
| PDF | page/object/font/image/stream/decode-work budget 均有界 |
| spreadsheet | sheet/row/column/cell/formula/shared-string/output cell 数均有界 |
| stdout/stderr | framed stdout 受 16 MiB output envelope 限制；stderr 最多 64 KiB，只保留去内容摘要 |
| concurrency | host/account/deployment 默认 4/2/1；无界 `spawn_blocking` 禁止 |

如果 Xberg public config 不能表达某个必须的 expansion/work limit，不能只在解析结束后检查输出。P5.7.0 要么在
adapter/container admission 前补上可证明的 bound，要么把对应格式判为 No-Go。不得 fork 一个“先全量解压到
内存、最后再看大小”的路径。

child 超时、OOM、signal、panic、无效 frame 和 parser error 必须满足：

- `ocd`、workerd、其他 indexing job 与 readiness 保持存活；
- 当前 job item 进入可重试或 permanent failure 的明确状态；
- 没有半个 generation 激活；
- 没有 orphan process、未清理 staging file 或未回收 pipe；
- 日志、metrics、support bundle 不包含文档正文或 secret。

## 8. 两条公开调用路径

Markdown Conversion 和 AI Search 共用同一个 admission、parser child、normalizer 与 limit owner，但生命周期不同：

```text
env.AI.toMarkdown(Blob)
  → binding/private backend authorization
  → bounded request-scoped bytes
  → format admission → parser child → normalize/options
  → ConversionResponse success/error

AI Search items.upload(stream/blob/string)
  → S3 immutable object + durable index job
  → format admission → parser child → normalize
  → staged chunks → embedding → FTS/vector activation
```

`toMarkdown` 是 request-scoped 同步 Promise：不创建 AI Search item/job/generation，不写 S3 或 SQLite 内容 authority，
客户端取消、Worker deadline 或 child deadline 到达后必须终止并 reap 当前 child。批量请求按输入顺序独立形成结果，
同时受一个有界并发预算；不能一次性把所有 Blob 和所有 Markdown 各复制多份到内存。

`toMarkdown` 只解析，不做 embedding，因此调用它不需要 `embedding_model`。AI Search 的 vector indexing 才使用
instance 创建时由 `AiSearchConfig.embedding_model` 选择并冻结的 model contract；解析器只输出规范化文本，不能
读取 provider endpoint/auth 配置或自行选择模型。embedding 配置、Smart Default deviation、operator provider 与
model 映射由 [P5 第 7 节](p5-vectorize-ai-search.md#7-operator-ai-provider-and-model-catalog)统一拥有。

AI Search 则保持 durable async 状态机：

```text
S3 immutable object
  → durable index job claim
  → format admission
  → parser child
  → validate normalized Markdown
  → platform text-splitter/tokenizer
  → persist staged chunks + parser contract
  → embedding batches
  → FTS/vector generation activation
```

parse 和 chunk 完成后，应先把 staged chunk rows、normalized digest、parser contract 和 resume cursor 事务化，再
开始远端 embedding。embedding 失败重试不应再次解析同一文档；process crash 后可以从已提交的 staged chunks
恢复。完整 Markdown 不是新的 authority，成功切分后无需作为永久副本保存；原始 S3 object + parser contract
仍可重建全部派生状态。

同一 `content_sha256 + parser_contract_sha256 + chunk_contract_sha256` 可以复用当前 instance 内已经提交的 staged
parse/chunk 结果，但不能跨 account 暴露内容，也不能引入独立 cache database。是否做跨 item dedup 属于 P5.8，
P5.7 先保持直接状态机。`toMarkdown` 不读取该 staged state，也不把 request-scoped 结果写成隐式全局 cache。

## 9. 固定公开文档 fixture

### 9.1 目的与目录

P5.7 不接受只用程序临时生成的空 ZIP/最小 PDF 证明解析质量。仓库已从公开、许可明确的其他项目中提交
**40 个真实固定文件**，来自 Apache Tika、LangChain4j、Apache POI 和
Apache PDFBox；其中 LangChain4j 的同内容跨格式文件本来就用于
[`ApacheTikaDocumentParserTest`](https://github.com/langchain4j/langchain4j/blob/b0b3b21e5f5679e86519ef3d979b7fbea0769f13/document-parsers/langchain4j-document-parser-apache-tika/src/test/java/dev/langchain4j/data/document/parser/apache/tika/ApacheTikaDocumentParserTest.java)，
可直接验证格式等价性，而不是拿 Xberg 自己的 corpus 给 Xberg 自证。

实际布局：

```text
test/fixtures/document-parser/
  corpus/
    apache-tika/<format>/<file>
    langchain4j/<format>/<file>
    apache-poi/<format>/<file>
    apache-pdfbox/pdf/<file>
  expected/<fixture-id>.json
  manifest.json
  README.md
  licenses/
    apache-tika-LICENSE.txt
    apache-tika-NOTICE.txt
    langchain4j-LICENSE.txt
    apache-poi-LICENSE.txt
    apache-poi-NOTICE.txt
    apache-pdfbox-LICENSE.txt
    apache-pdfbox-NOTICE.txt
```

fixture bytes、expected 和 manifest 都提交 Git，不依赖 Git LFS、运行时 bucket、测试时网络或上游 HEAD。网络只
允许由显式 maintainer import 工具在新增/升级 corpus 时使用；普通 build、Gate、coverage、发行和 production
startup 必须完全离线。

### 9.2 最终 40 个固定文件

来源 revision、逐文件 URL/size/SHA-256、license、oracle 与预期状态均固定在 `manifest.json`：

```text
Apache Tika     1e3d8f888380d8b302ce4787bc7d5fbb513f1867
LangChain4j     b0b3b21e5f5679e86519ef3d979b7fbea0769f13
Apache POI      6d94ace657249b487959dd654ca9d9b1c6014e4e
Apache PDFBox   44ae1f5a0371c37128b20fac2beecdfd0c93b503
```

路径前缀：

- `TIKA-PDF`：[`tika-parser-pdf-module/.../test-documents`](https://github.com/apache/tika/tree/1e3d8f888380d8b302ce4787bc7d5fbb513f1867/tika-parsers/tika-parsers-standard/tika-parsers-standard-modules/tika-parser-pdf-module/src/test/resources/test-documents)
- `TIKA-MS`：[`tika-parser-microsoft-module/.../test-documents`](https://github.com/apache/tika/tree/1e3d8f888380d8b302ce4787bc7d5fbb513f1867/tika-parsers/tika-parsers-standard/tika-parsers-standard-modules/tika-parser-microsoft-module/src/test/resources/test-documents)
- `TIKA-ODF`：[`tika-parser-miscoffice-module/.../test-documents`](https://github.com/apache/tika/tree/1e3d8f888380d8b302ce4787bc7d5fbb513f1867/tika-parsers/tika-parsers-standard/tika-parsers-standard-modules/tika-parser-miscoffice-module/src/test/resources/test-documents)
- `TIKA-INT`：[`standard-integration-tests/.../test-documents`](https://github.com/apache/tika/tree/1e3d8f888380d8b302ce4787bc7d5fbb513f1867/tika-parsers/tika-parsers-standard/tika-parsers-standard-integration-tests/src/test/resources/test-documents)
- `TIKA-APPLE`：[`tika-parser-apple-module/.../test-documents`](https://github.com/apache/tika/tree/1e3d8f888380d8b302ce4787bc7d5fbb513f1867/tika-parsers/tika-parsers-standard/tika-parsers-standard-modules/tika-parser-apple-module/src/test/resources/test-documents)
- `LC4J`：[`langchain4j-document-parser-apache-tika/.../resources`](https://github.com/langchain4j/langchain4j/tree/b0b3b21e5f5679e86519ef3d979b7fbea0769f13/document-parsers/langchain4j-document-parser-apache-tika/src/test/resources)
- `POI`：[`test-data`](https://github.com/apache/poi/tree/6d94ace657249b487959dd654ca9d9b1c6014e4e/test-data)
- `PDFBOX`：[`pdfbox/src/test/resources`](https://github.com/apache/pdfbox/tree/44ae1f5a0371c37128b20fac2beecdfd0c93b503/pdfbox/src/test/resources)

| # | 来源 | 文件 | 覆盖意图 |
| ---: | --- | --- | --- |
| 1 | TIKA-PDF | `testPDF.pdf` | 基础 text-layer PDF |
| 2 | TIKA-PDF | `testPDFVarious.pdf` | 多种 PDF 内容对象 |
| 3 | TIKA-PDF | `testPDFTripleLangTitle.pdf` | 多语言 metadata/Unicode |
| 4 | TIKA-PDF | `testOptionalHyphen.pdf` | optional hyphen/断词 |
| 5 | TIKA-PDF | `testOverlappingText.pdf` | 重叠文字去重/顺序 |
| 6 | TIKA-PDF | `testPDF_rotated.pdf` | 页面旋转与 reading order |
| 7 | TIKA-PDF | `testPDF_protected.pdf` | 加密/权限负向路径 |
| 8 | TIKA-PDF | `testOCR.pdf` | OCR/text-layer 差异；无有效 text layer 时必须 `OCR_REQUIRED` |
| 9 | TIKA-MS | `testWORD.docx` | 基础 DOCX |
| 10 | TIKA-MS | `testWORD_features.docx` | heading/list/table/format |
| 11 | TIKA-MS | `testWORD_numbered_list.docx` | 多级编号列表 |
| 12 | TIKA-MS | `testWORD_phonetic.docx` | 日文/phonetic Unicode |
| 13 | TIKA-MS | `testWORD_truncated.docx` | 截断 OOXML 负向路径 |
| 14 | TIKA-MS | `testEXCEL.xlsx` | 基础 XLSX、多 sheet |
| 15 | TIKA-MS | `testEXCEL-formats.xlsx` | 数字/日期/显示格式 |
| 16 | TIKA-MS | `testEXCEL_phonetic.xlsx` | 日文/phonetic cell |
| 17 | TIKA-MS | `testEXCEL_charts.xlsx` | chart/embedded object warning |
| 18 | TIKA-MS | `testEXCEL_macro.xlsm` | macro-enabled；绝不执行宏 |
| 19 | TIKA-MS | `testEXCEL.xlsb` | XLSB No-Go/abort-risk 证据 |
| 20 | TIKA-MS | `testEXCEL.xls` | legacy BIFF/XLS |
| 21 | TIKA-INT | `test-columnar.ods` | ODS 列/表格顺序 |
| 22 | TIKA-ODF | `testFooter.ods` | ODS footer 边界 |
| 23 | TIKA-ODF | `testFooter.odt` | ODT footer 边界 |
| 24 | TIKA-ODF | `testODTEmbedded.odt` | ODT embedded object，不递归索引 |
| 25 | TIKA-APPLE | `testNumbers.numbers` | Apple Numbers 基础表格 |
| 26 | TIKA-APPLE | `tableHeaders.numbers` | Numbers header/structure |
| 27 | LC4J | `test-file.pdf` | `test content` 跨格式等价组 |
| 28 | LC4J | `test-file.docx` | 同上 |
| 29 | LC4J | `test-file.xls` | 同上；sheet name/order |
| 30 | LC4J | `test-file.xlsx` | 同上；sheet name/order |
| 31 | LC4J | `blank-file.docx` | 空文档错误 |
| 32 | LC4J | `blank-file.xlsx` | 空 workbook 边界 |
| 33 | POI | `document/HeaderFooterUnicode.docx` | header/footer Unicode |
| 34 | POI | `spreadsheet/chinese-provinces.xls` | 简体中文 XLS |
| 35 | POI | `spreadsheet/unicodeNameRecord.xls` | Unicode name record |
| 36 | POI | `spreadsheet/unicodeSheetName.xlsx` | Unicode sheet name |
| 37 | PDFBOX | `org/apache/pdfbox/text/BidiSample.pdf` | RTL/bidirectional text |
| 38 | PDFBOX | `input/PDFBOX-5747-unicode-surrogate-with-diacritic-reduced.pdf` | supplementary Unicode/diacritic |
| 39 | POI | `spreadsheet/SimpleMacro.xlsm` | 第二个 XLSM；验证宏不执行 |
| 40 | TIKA-MS | `test-columnar.xlsb` | 第二个 XLSB；固定 No-Go/abort-risk 证据 |

以上清单有意不使用 Xberg/Kreuzberg 自己的 corpus 作为主验收证据。Xberg 的公开 corpus 可以用于升级前的补充
回归和故障复现，但不能替代独立项目的固定 fixture。

### 9.3 最终格式处置

40-file corpus 覆盖 English、简体中文、Japanese、Latin-extended、RTL 与 supplementary Unicode；每个 successful
fixture 都有 semantic oracle 与 exact normalized Markdown digest。XLSB 与 Numbers fixture 保留为真实 No-Go
证据，ET 因无固定 revision、逐文件可再分发证据的真实样本而明确 No-Go；不从网络随机找办公文件补数，也不把
改扩展名的 XLS 当作 ET 支持证据。

### 9.4 manifest 与许可证

`manifest.json` 是 corpus authority，每个 entry 至少包含：

```json
{
  "id": "tika-pdf-basic",
  "path": "corpus/apache-tika/pdf/testPDF.pdf",
  "sha256": "...",
  "size_bytes": 12345,
  "format": "pdf",
  "mime": "application/pdf",
  "languages": ["en"],
  "scripts": ["Latin"],
  "expected_status": "ok",
  "oracle": "expected/tika-pdf-basic.json",
  "source_repository": "https://github.com/apache/tika",
  "source_revision": "1e3d8f888380d8b302ce4787bc7d5fbb513f1867",
  "source_path": ".../testPDF.pdf",
  "license": "Apache-2.0",
  "attribution": "licenses/apache-tika-NOTICE.txt"
}
```

import 工具必须：

- 只接受 manifest 中的 exact HTTPS URL、revision、expected SHA-256 和最大 bytes；
- 下载到 `.temp/document-fixture-import/`，校验后由 maintainer 显式更新 tracked fixture；
- 拒绝 redirect 到非 allowlisted host、HTML error page、Git LFS pointer 和已存在目标覆盖；
- 生成 size/hash/provenance 报告，但不自动接受 license；
- import 后普通测试不得再联网。

Apache 项目的 repository license 不能替代逐文件审计。若 upstream LICENSE/NOTICE 对某个 sample 有专门条款，
必须随 fixture 保留；来源或权利不清的文件删除并换成许可明确的样本。

### 9.5 oracle 设计

不把 Xberg 当前完整 Markdown 直接当作唯一真相。每个 `expected/*.json` 同时保存：

```json
{
  "status": "ok",
  "must_contain": ["reviewed text fragment"],
  "must_not_contain": ["macro source", "external secret"],
  "min_normalized_chars": 100,
  "max_normalized_chars": 20000,
  "structure": {
    "min_headings": 0,
    "min_tables": 0,
    "page_count": null,
    "sheet_names": []
  },
  "normalized_markdown_sha256": "...",
  "retrieval_queries": []
}
```

验收分三层：

1. **语义 oracle**：人工审阅 `must_contain`、关键表格/标题/顺序、禁止内容和 error class；
2. **确定性 oracle**：同一 parser contract 的规范化 Markdown SHA-256 必须跨重启一致；
3. **检索 oracle**：选定 fixture 建立 chunks 后，固定多语言 query 必须召回预期 item/chunk；这验证
   parse → chunk → embed → search 全链路，不要求 embedding score 与 Cloudflare 逐位一致。

Xberg 升级导致 digest 变化时，必须先展示 semantic diff、解释原因、重新跑 retrieval 与攻击 corpus，再由
maintainer 更新 golden。测试代码不能在运行中“接受当前输出”为新 golden。

## 10. 测试与 Gate

### 10.1 dependency/发行 Gate

```text
Rust 1.98 MSRV
no-default-features
cargo tree -e features allowlist
license/advisory/unsafe/native inventory
offline clean build after explicit dependency preparation
Linux x64/arm64 + macOS x64/arm64
release binary and compressed package size delta
no runtime/build-script model or native-library download
```

### 10.2 parser contract Gate

- 当前 40-file seed corpus 全量执行，后续只增不以缩减样本掩盖 regression；
- supported extension、MIME、magic、大小写与 mismatch matrix；
- exact same content 的 PDF/DOCX/XLS/XLSX 等价组保留共同关键文本；
- 多语言 Unicode 不 mojibake、不丢 surrogate、不错误重排 RTL/LTR token；
- heading/list/table/page/sheet/read-order invariants；
- encrypted、blank、truncated、scanned PDF 返回正确稳定错误；
- macro、external relationship、embedded object 不执行、不联网、不递归创建 item；
- 每个结果通过 Markdown/output/metadata bounds 和 deterministic digest。

### 10.3 hostile corpus

公开 corpus 之外，`test/fuzz/corpus/document-parser/` 必须有 project-owned 的最小 hostile inputs：

```text
ZIP bomb / extreme compression ratio / duplicate central-directory entry
OOXML/ODF XML deep nesting / entity / oversized shared strings / relationship cycle
OLE stream length overflow / truncated sector chain
PDF object/xref cycle / corrupt font / huge dimensions / nested stream / invalid image
spreadsheet max row/column/cell/formula/shared-string counts
frame length overflow / invalid UTF-8 / trailing bytes / wrong digest
```

hostile fixture 可以由确定性生成器产生，但生成器、seed、expected error 和生成后 SHA-256 都必须 tracked；不能在
每次测试随机生成一个无法复现的文件。

### 10.4 process/recovery Gate

每种格式至少覆盖以下故障点：

```text
before spawn
after child start before input
mid input
after parse before output frame
partial/oversized/invalid output frame
child panic/abort/signal/OOM/deadline
after validated output before staged-chunk commit
after staged-chunk commit before embedding wake
ocd SIGKILL with parser child live
shutdown with queued/running parser jobs
```

断言：

- `ocd` 和 workerd 不因 parser child 失败退出；
- child/process group 被精确 reap，PID identity 不误杀其他进程；
- restart 重新发现 durable job，旧 claim/output 不能提交；
- staged chunks 只在完整事务后可恢复，active generation 不半切换；
- deadline/admission 下 `/health/live`、`/health/ready` 和现有产品保持响应；
- `.temp/`、staging、pipe、stderr evidence 的 ownership 与清理符合全局 Gate 规则。

### 10.5 Cloudflare differential

远端 differential 只比较公开可观测行为：

- `env.AI.toMarkdown(file)`、array overload、`toMarkdown().transform()` 和 `supported()` 的输入/输出 shape；
- `output.format`、`html.hostname/cssSelector`、`pdf.metadata` 与 unsupported option 的行为；
- mixed-success batch 的顺序、per-file error 与 whole-call rejection 边界；
- AI Search `items.upload/uploadAndPoll/get/info/logs/chunks/download/delete` 的状态和返回 shape；
- 当前支持扩展/MIME 与 4 MiB 边界；
- blank、encrypted、malformed、scanned rich document 的 item/job 状态和错误类别；
- PDF、DOCX、XLSX、XLS、ODS、ODT 的关键文本是否可搜索；
- filename/content-type/magic mismatch；
- table/sheet/formula cached value 的常用行为。

不以完整 Markdown 字节、page geometry、私有 parser metadata 或 embedding score parity 作为兼容 Gate。远端上传是
显式外部 mutation，使用独立随机资源和精确 cleanup，不进入默认 `--workspace`。

### 10.6 stock-workerd API Gate

fixture Worker 必须使用 pinned generated Env 类型编译，并在 stock workerd 中直接执行，不能用 Rust 单测、
Miniflare 或自定义 JavaScript object 替代。至少覆盖：

```text
[ai] binding declaration → env.AI
single document → single ConversionResponse
document[] → same-order ConversionResponse[]
env.AI.toMarkdown().transform single/array overload
env.AI.toMarkdown().supported deterministic actual capabilities
Blob type/name/MIME/option validation
format=markdown/text and success/error discriminated union
AI Search namespace binding and direct instance binding
upload immediate queued state and uploadAndPoll terminal/timeout behavior
item info/logs/chunks/download/delete after rich-document parse
deployment/generation/descriptor authorization and stale facade rejection
```

TypeScript compile fixture 必须包含 Cloudflare 官方示例的调用形式。任何为了测试通过而要求 tenant 改成
`env.XBERG`、`parseDocument()`、平台专用 header 或非 Cloudflare response field 的实现都判为 No-Go。

## 11. 已完成实施阶段

### P5.7.0：合同、dependency 与 corpus Gate（本地完成）

1. 冻结 pinned `Ai`/`ToMarkdownService`/AI Search Items 逐成员 API、Cloudflare rich-format snapshot 和 deviation matrix；
2. 对 crates.io `xberg = "=1.0.14"` 跑 feature/MSRV/license/offline 本机验证；four-target/size 留在 release acceptance；
3. 验证每种 Xberg format 的 API、security limits 和 deterministic output；
4. import、审计并提交至少 30 个公开 fixtures；当前实际提交 40 个；
5. `.et`、XLSB、Numbers 写明 No-Go/deviation；
6. 冻结 overload、response/error、options、limits 和格式 admission；托管 rich-document differential 留在 release acceptance；
7. 输出本地核心 Go，No-Go 格式不进入 capability。

### P5.7.1：parser child 与协议（完成）

- 新建文档解析 owner module/crate，保持现有依赖方向；
- 实现隐藏 child mode、binary frame、process supervision 和 limits；
- 实现 normalization、stable errors、metrics 和 content-free diagnostics；
- 完成 frame/hostile corpus 与 process ownership focused Gate；完整 abort/OOM/orphan/soak matrix 留在 release acceptance。

### P5.7.2：格式 adapter（完成）

- 按 PDF → DOCX/ODT → spreadsheet 的依赖顺序启用；
- 每增加一种格式先过对应 fixture/hostile/process matrix，再进入 allowlist；
- `.et`、XLSB 与 Numbers 维持明确 No-Go；
- unsupported Xberg 格式继续在 public admission 层拒绝。

### P5.7.3：Cloudflare API facade 与状态机集成（完成）

- toolchain 接受标准 `[ai] binding = "AI"`，以 immutable builtin-binding descriptor 注入 `env.AI`；
- loader facade 实现 `toMarkdown` direct/handle overload、`transform()`、`supported()` 和 pinned error mapping；
- 未实现的 Workers AI 成员稳定 fail closed，并进入 capability/deviation，不伪装 full Workers AI；
- 扩展 item generation parser contract 与 parse summary；
- durable parse → staged chunk → embedding resume；
- generation reindex、cancel、delete、snapshot/restore 和 GC；
- stock-workerd Markdown Conversion + item/job/search 全链路与多语言 retrieval oracle。

### P5.7.4：本机 hardening 完成；发行资格分离

- 本轮完成全 corpus、hostile、focused recovery、coverage 与用户要求的一轮本机 Gate；
- 四平台 release qualification、binary size、完整 crash/OOM/orphan/soak 与 timing-three 留在 active release acceptance；
- 更新 capabilities/deviations、operator metrics/runbook、single-binary、snapshot 和总架构文档；
- 记录 exact Xberg pin、feature tree、fixture manifest digest、case count 和 Gate report。

## 12. P5.8 边界

以下能力不在 P5.7 实现：

- 图片和扫描 PDF OCR；
- PDFium fallback、layout detection、table/image AI reconstruction；
- ANN/USearch derived accelerator；
- R2 continuous data source、website crawler；
- similarity cache；
- query rewrite、rerank/chat 的新增 provider adapter；
- parser worker pool、跨 item/account parse dedup；
- Cloudflare Markdown Conversion `/client/v4` REST adapter 与 account-scoped API token；
- 扩大到 Cloudflare 未公开支持的 PPT/PPTX、RTF、ODP、DOC、EPUB 等格式。

这些统一归入 P5.8 条件式扩展。每项都需要独立 Go 条件，当前不实现，也不能预留双实现、空配置或未使用的
production abstraction。

## 13. 完成与后续资格边界

P5.7 本地核心归档需满足：

- stock workerd 中的 `[ai]` binding、`env.AI.toMarkdown` direct/handle overload 和 AI Search Items API 通过
  pinned types、官方示例及 Cloudflare differential；
- 没有 tenant-visible Xberg API、parser wire 或自定义 response field；
- Cloudflare rich-format supported/deviation matrix 固定，所有 advertised extension 都有真实 fixture；
- Xberg exact version/checksum/feature tree、license、unsafe/native 与本机供应链证据完整；
- 至少 30 个 vendor 固定文件、逐文件 SHA-256/provenance/license/oracle 均已提交；
- parser child malformed input/output、timeout 和失败路径不影响 `ocd`/workerd；
- PDF/DOCX/XLSX/XLSM/XLS/ODS/ODT 完成产品链、恢复与多语言检索 Gate；
- XLSB、Numbers、`.et` 登记明确 No-Go/deviation，不允许模糊状态；
- production build/start/parse 无隐式下载，不依赖 Python、Java、LibreOffice、PDFium 或 OCR 模型；
- unsupported/P5.8 格式 fail closed；
- capability、deviation、operator、single-binary、snapshot、testing 和总架构文档同步；
- `docs/implemented/` 中记录实际 revision、fixture manifest digest、case count、coverage、本机结果和接受限制。

四平台结果、release binary/size/offline package、完整 crash/abort/OOM/orphan/soak、timing-three 与托管
rich-document differential 留在 active release acceptance，不冒充本轮本机证据。
