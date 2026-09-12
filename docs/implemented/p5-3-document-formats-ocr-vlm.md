# P5.3：Cloudflare 文档格式、OCR 与可选 VLM

状态：**implemented（2026-09-11）**。尚需真实 provider、hosted differential、正式平台构建和发行尺寸证据的项目见
[P5 剩余发行验收](../acceptance/p5-release-acceptance.md)。

本阶段统一完成 [#45 `chunk: false`](https://github.com/elliothux/open-compute/issues/45)、
[#46 durable parse cache](https://github.com/elliothux/open-compute/issues/46)、
[#47 document formats](https://github.com/elliothux/open-compute/issues/47) 和
[#48 OCR/image extraction](https://github.com/elliothux/open-compute/issues/48)。

## 当前结果

- 一个 typed registry 定义 Cloudflare 基线的 62 个候选扩展名。AI Search 公布并接收其中 59 个，
  `AI.toMarkdown().supported()` 公布 18 个富文档格式；`.xlsb`、`.et`、`.numbers` 因准入证据不足继续稳定拒绝。
- 41 个纯文本扩展名直接执行 BOM-aware UTF-8、control/NUL、NFC 和换行规范化；`.log.gz` 另有 16-member、
  64 MiB 展开量和 100:1 ratio 上限。`.env`、`.gitignore`、`.editorconfig` 按 exact basename 匹配。
- Xberg 精确固定为 `=1.1.5`，只启用 `tokio-runtime,pdf,office,excel,xml,ocr,svg`。已知会使
  `calamine 0.36.1` panic 的 XLSB 路径不进入生产调用。
- JPEG、PNG、WebP、SVG、GIF 首帧和 BMP 在隔离 parser child 中解码；SVG 禁止脚本、外部资源和非内部引用。
  图片执行固定 Tesseract OCR，并生成去 metadata、白底、Lanczos3、JPEG quality 90 的有界 VLM candidate。
- 无文字扫描 PDF 由 Xberg/Tesseract OCR。配置 VLM 时，OCR-only PDF 最多按配置渲染 16 个页面 candidate；
  born-digital PDF 不生成页面描述，避免重复文本与 provider fan-out。
- VLM 是 operator-only 的 OpenAI chat-completions-compatible mapping。parser child 无网络和 provider secret；parent
  执行 HTTPS/loopback policy、请求/响应限制、并发限制和固定 prompt，并把描述与 OCR 文本合并为 Markdown。
- 默认单文件限制保持 Cloudflare 的 4 MiB；operator 可通过现有 `document_parser.max_input_bytes` 提高到 64 MiB，
  batch hard cap 为 256 MiB。公开 Cloudflare API 没有新增字段。
- `chunk: false` 直接生成一个 ordinal 0、覆盖完整 normalized Markdown byte range 的 chunk。vector/hybrid 文档若超过
  frozen embedding input contract，以 `EMBEDDING_INPUT_TOO_LARGE` 失败且不激活部分 generation；keyword-only 不受 embedding
  input limit 约束。`chunk_size`／`chunk_overlap` 与 `chunk: false` 组合继续 fail closed。
- 每个 instance 有一个独立的 `parse-cache.sqlite`，以 source SHA-256/size、logical filename、canonical MIME、parser contract、
  固定 conversion options 和 OCR/VLM contract 为 key，持久复用完整 normalized parse result。上限为 512 entries／128 MiB，
  使用 deterministic LRU；损坏、miss、eviction 或 cache DB 不可用都安全重算。Xberg Tesseract 自带的全局 OCR result cache
  明确关闭，不读取或写入用户 cache directory，也不形成第二个未按 account/instance 隔离的持久化面。

## 权威合同

`crates/document-parser/src/admission.rs` 是 filename、MIME、magic、处理类别和 advertised surface 的唯一格式
authority。AI Search upload、Markdown Conversion、parser frame 和文档清单均消费该 registry，不保留 P5.1 列表或
旧 Xberg 分支。纯文本保持 `PlainText` content kind，富文档和图片输出 normalized Markdown。

parser contract 固定格式 registry、Xberg feature、OCR engine/languages、tessdata revision、解压与 decoder limits、
normalizer 和 raster revision。AI Search 另外把 output kind、description language、VLM enabled/disabled、secret-free
backend/model contract、prompt/preprocessing revision及所有 VLM semantic limits纳入 digest；secret value、timeout、并发和
host path 不进入索引语义。

空 OCR 结果可由 Markdown Conversion 返回空内容；AI Search 以 `DOCUMENT_NO_EXTRACTABLE_TEXT` 结束该 item，
不创建空 chunk/vector。已配置 VLM 的临时失败不会静默缓存 OCR-only 结果。

## Parse cache 与恢复

cache value 保存 normalized Markdown 及其 SHA-256、detected format/MIME、content kind、page/sheet metadata、受限 document
metadata、warning codes 和完整 semantic contract。raw source bytes、provider credential、endpoint、cache key 和内部路径均不进入
value、用户响应或日志。相同 source/contract 的 retry、replacement 和 full reindex 在 single-flight 后复用一次成功 parse；source、
filename/MIME、parser/options 或 OCR/VLM contract 任一变化都 miss。失败 parse 与 VLM transient response 不写成功 cache。

`parse-cache.sqlite` 是独立于 instance `data.sqlite` authority 的 disposable acceleration：重启保留有效 entry，row corruption 会删除
该 entry 并重算，cache 整体不可用不阻止 authoritative indexing。snapshot/restore 只携带 authority 和 immutable source reference，
不携带 parse cache；restore 后按需重建。删除 instance 时 cache 随其已隔离的 instance directory 一起删除。固定 cardinality metric
`ai_search_parse_cache_total{outcome="hit|miss|store|reject|evict"}` 不包含 account、instance、source 或 key label。

parser child 的 exit/signal 与 stdout/stderr bound 失败以 `DOCUMENT_PROCESS_FAILED` 立即结束索引；timeout、临时 admission、provider
或对象存储不可用最多执行五次持久 claim attempt，之后以原始稳定 code 进入 `error`。重启不重置 attempt。operator log 只记录
失败类别、exit code/Unix signal、bounded stdout/stderr byte count 和 stderr digest，不记录 stderr 正文、文档内容、secret 或内部路径。

## OCR 供应链和单文件分发

`crates/document-parser/tessdata.lock.json` 固定 `tessdata_fast` revision
`87416418657359cb625c412a48b6e1d6d41c29bd` 的 `eng`、`chi_sim`、`chi_tra` bytes；不包含 OSD。
构建同时验证 `crates/document-parser/tesseract-source.lock.json` 中的 Tesseract、Leptonica 和 Xberg 静态构建输入。
所有正式 bytes 位于 `share/` 并由 Git LFS 管理；普通 build、启动和请求路径均不下载或搜索系统安装。

语言资产确定性压缩进 `ocd`。服务在独占 data-dir 下按 lock digest 以 0700/0600、fsync、原子 rename 物化并逐项复验；
Xberg 的语言 validator alias 是同目录内经过相同 digest 校验的 hard link。parser child 只收到 parent 生成的绝对路径。
许可证由 `ocd licenses` 内嵌 Tesseract Apache-2.0、Leptonica BSD-2-Clause 和 tessdata notice。

## 安全与限制

- 源文件、gzip/ZIP 展开、页数、表格、XML、图片尺寸/像素、candidate bytes、private frame、Markdown、provider body 和
  provider response 是独立预算；提高源文件限制不会放宽其它预算。
- 单个 candidate 不超过 1280×720、921,600 pixels 和 1 MiB JPEG；每文档最多 16 个。模型 mapping 只能进一步收紧
  width、height、pixels、encoded bytes 和 output tokens。
- VLM 只接受 `en|it|de|es|fr|pt`，使用 non-stream request、temperature 0、固定 system/user prompt 和 base64 data URL；
  tenant 不能选择 endpoint、model、header 或 prompt。
- macOS parser child 的 RSS hard limit 仍是已接受的后续项；CPU、wall time、输入/输出、并发、process-group 回收和
  `RLIMIT_FSIZE=0`、无网络边界继续生效。JPEG/PNG/WebP/SVG/GIF/BMP 与扫描 PDF 的 OCR 不需要 regular-file write。

## 尚待资格化

本地 deterministic Gate 不调用 DeepSeek。`OPEN_COMPUTE_RUN_DEEPSEEK_VLM=1 bun run test/deepseek-vlm-live.ts`
是明确授权后单独执行的真实 wire qualification，只读取 root `/.env` 中的 `DEEPSEEK_API_KEY`，且不输出 secret、请求图片或
provider body；该检查固定当前官方视觉模型 `deepseek-v4-flash-vision-exp`。正式平台静态链接、release binary 增量和
hosted Markdown Conversion differential 同样留在验收清单，没有用本机结果代替。
