# P6：Cloudflare v4 API 与 Wrangler 子集兼容实现记录

> 状态：**核心实现与 P6 scoped Gate 已完成；canonical Clippy、hosted differential、workspace Gate 与 coverage 尚未闭环**，2026-09-03。
> 本记录冻结已经进入 production path 的 P6 范围、正式 `cf-compatibility-check` 与本机验收证据；未执行或失败的
> 项目继续明确保留，不从 focused PASS 外推整仓验收。

## 1. 结论与范围

P6 已把 `/client/v4`、`wrangler.jsonc`、固定上游 Wrangler 和固定官方 Cloudflare SDK 接入 Day 1 唯一管理面。
标准资源使用 Cloudflare 路径、包络、分页和错误边界；open-compute 独有能力只保留在同一 transport 下的 vendor
namespace。旧 `/operator/api/v1`、`open-compute.json`、Operator SDK、自定义 upload transport 和生产 consumer
已经直接删除，没有 alias、redirect、双写或历史 schema 兼容路径。

实现范围包括 protocol core、Workers Scripts/Versions/Deployments/Settings/Secrets、Static Assets、KV、D1、R2、
Vectorize、AI Search、Queues、Workflows、vendor extension 与原 Dashboard/测试/工具 consumer 的迁移。既有 runtime
authority、SQLite、S3、immutable Version 和 stock workerd 仍是唯一数据与执行路径；v4 handler 只负责官方 wire
validation 和 domain 调用。

P6 没有提前实现后续阶段：P7 的 Workers Logs/realtime tail/Observability、P8 的 Workers Standard limits 和 P9 的
public `worker_loaders`/Worker Loader 仍分别标为 `planned` 或 `unsupported`。相关 route、配置字段、binding 与命令在
对应阶段完成前 fail closed。

## 2. 冻结输入

| 输入 | 冻结值 |
| --- | --- |
| Wrangler | `4.127.1`；package SHA-256 `f076e0a2cbff001c064a584f1abbde0dd2a58002ab38db5f57abaa2224bab043`；config schema SHA-256 `e42dc556dcb039aa1103d4811b1f58497e2676aceee20488d9ceb1d8ab712018` |
| Cloudflare TypeScript SDK | `7.1.0`；package SHA-256 `57cca8de9f72799d0c23cc1f2e289ac16d150a23352e9e7676ef22aee676139a` |
| Cloudflare OpenAPI | `cloudflare/api-schemas@b8687f42e28fbfcb296a350f7dbf16349ea900af`；`openapi.json` SHA-256 `2ffedbbf8b25361a3be2062b7793946e7b9efc0e48b462da68f3195f12ab052b` |
| P6 generated contract | subset SHA-256 `486e41018352aa664ef1035c75e13b69ee3bbe17bf3cf9c1b5b1267e738484e3`；capability SHA-256 `ea4230ac2c065a0e17f433d7bf7ebcdcae0e9a1c2a3e0c51b21dc4b9d351948b` |
| workerd | `v1.20260830.1` / revision `e9dda5963aba7ee4323960db795690ec78fec118`；effective compatibility date `2026-08-30` |
| workers-types / workers-sdk | `5.20260830.1` / revision `f8085545bcaa2c639f171c25e4424685036a0e10` |
| 部署画像 | self-hosted、单机、SMB；不声称 Cloudflare 全球 fleet、multi-region control plane、商业 plan 或托管服务内部语义 |

上表 generated hash 是本记录创建时的实现输入；若最终源码冻结前 contract generation 发生变化，最终验收必须先
更新 lock 和本节，不能沿用旧 hash。

## 3. 已实现结果

### 3.1 Protocol、Workers 与 Assets

- `/client/v4` 共享 Bearer 鉴权、account scope、请求 ID、Cloudflare success/error envelope、分页和稳定错误码；
  official subset、vendor extension、route inventory 与 capability manifest 由锁定的 OpenAPI/生成流程校验。
- Workers Scripts、Versions、Deployments、rollback、settings 和 secrets 均映射到 immutable Worker/Version authority；
  compatibility date/flags 和 binding descriptor 在 Version 创建时冻结，未知或尚未支持的字段拒绝而非忽略。
- Wrangler multipart upload、official SDK 7.1 typed upload 和 Static Assets 三段 direct-upload 都汇入同一个有界 admission
  path。SDK 7.1 的错误 `Content-Type` 与 bracket-form fields 只在精确 pin 下按 closed schema 归一；无法唯一重建、
  重复或未知的 shape fail closed，没有第二 transport。
- Static Assets 继续使用 manifest、session、逐对象上传和 immutable deployment authority；未恢复旧 upload endpoint。

### 3.2 KV、D1、R2、Vectorize 与 AI Search

- KV namespace/key/bulk、D1 database/query/import/migrations/time-travel、R2 bucket/object、Vectorize index/vector/metadata
  index 以及 AI Search namespace/instance/stats/search/jobs/item 的 P6 子集均已接入标准 v4 路径。
- D1 time-travel 两个 route 明确为 `supported_with_deviation`：普通 Worker mutation 不再同步复制整库，只有显式
  management 操作建立最多 8 个 retained checkpoints；timestamp/restore 仅解析这些点，容量被 durable transfer/
  restore evidence 占满时在复制或 mutation 前拒绝，过期 terminal transfer authority/file 会精确回收并释放 pin；
  每库同时最多保留 8 个未过期 terminal transfer file，生成新 export/import file 前达到上限会拒绝；不宣称
  Cloudflare always-on 分钟级 7/30 天 PITR。authority 删除后的极低概率 checkpoint/transfer unlink orphan
  作为单机磁盘清理长尾保留，不增加启动扫描或日志型 GC 状态机。
- 固定 Wrangler 的 Vectorize multipart 被显式 bounded，超过上限按稳定请求错误分类；AI Search update 对显式非法
  indexing/retrieval 配置拒绝，不再把它们当作可丢弃的继承默认值。
- Vectorize catalog 会在读取产品 locator 前排除已删除资源的 tombstone；删除一个 index 后不会再使同 account 的
  后续 ready index 误报 404。live/transitional 行仍逐项严格校验，损坏 locator 不被静默跳过。
- 固定 SDK 的 typed Worker upload 对 D1 binding 发送 `database_id`，固定 Wrangler 发送 `id`。adapter 只在 binding
  `type=d1` 且字段可唯一归组时把前者归一到内部唯一 `id`；两字段并存或其它 binding 使用 `database_id` 均拒绝。

### 3.3 Queues、Service Binding 与 Workflows

- Queues 的 queue/consumer 管理面已使用官方 v4 shape，并复用原有 producer、consumer、retry、metrics 与持久化
  authority。固定 Wrangler 4.127.1 会警告 `delivery_delay` 已废弃且无效果，因此 P6 只接受并忽略该上传 metadata，
  不把它持久化为 Queue authority，也不宣称当前 Cloudflare 文档与该 pinned-client 行为完全一致。
- Service Binding `props` 作为最大 64 KiB、最大深度 32 的 canonical JSON object 冻结进 descriptor/digest，投影到
  `WorkerLoader.getEntrypoint(name, { props })`，tenant 从 `ctx.props` 读取；读取时重新验证 canonical bytes/digest，
  损坏状态 fail closed。
- 固定 Wrangler 的 Workflow 顺序是 Worker upload、account subdomain GET、Workflow PUT。upload 前置 reservation
  在 SQLite 中冻结 name/class、owner 和 monotonic fence；只有同 fence 的 immutable binding/version 可以完成，
  bound/terminal reservation 不被超时抢占，失败 release 和重启 integrity 都有明确状态。
- Workflow delete 先记录 one-way delete intent/fence，阻断新 reservation，再删除实例并完成 tombstone；并发旧 owner、
  retry 和 terminal history 不能重新发布被删除 definition。该实现优先保证单机 crash/restart 和数据完整性，没有
  引入分布式 lease 或外部协调服务。

### 3.4 Vendor extension 与 consumer cleanup

- open-compute capability/system/health 等独有能力留在 v4 vendor namespace。TypeScript extension 复用官方 SDK 的
  公开 `get/post/put/patch/delete` transport，不携带独立 base URL、鉴权或 raw-fetch client。
- official SDK live path 包含 account/membership、typed Worker upload/readback、D1 binding 投影和 vendor extension
  调用；同一 ready `ocd` 内的立即回读只证明当前 authority 投影，不外推为 SDK wrapper 的独立 restart 资格。
- Dashboard、framework/toolchain adapter、conformance fixture、process Gate 和 operator tests 已改用新合同；生产树不再
  保留旧 DTO、旧 route、旧 SDK 或旧 project config parser。

## 4. 显式 deviation 与固定客户端冲突

| 项目 | 当前合同 |
| --- | --- |
| `OC-ACCOUNT-SUBDOMAIN-001` | `GET /accounts/{account_id}/workers/subdomain` 只为固定 Wrangler Workflow deploy prerequisite 返回 account-stable、以 `_` 开头的不可路由 label；不创建 DNS、listener、route 或 mutation authority，`PUT/DELETE` unsupported。 |
| `OC-SERVICE-001` | 固定 Wrangler 的静态 Service Binding `props` 只支持最大 64 KiB、最大深度 32 的 canonical JSON object；不声称动态 `ctx.exports` 可接受的任意顶层 JSON、Cloudflare fleet placement 或全局 discovery。 |
| `OC-D1-001` | D1 Session 保证同一 local primary 的顺序可见性；只有显式 management 操作创建最多 8 个 retained checkpoints，timestamp/restore 只解析这些点，不声称 Cloudflare always-on 分钟级 7/30 天 Time Travel。 |
| `OC-AI-SEARCH-TOKEN-001` | `GET /accounts/{account_id}/ai-search/tokens` 只返回一个 account-scoped、稳定、无 secret 的 installation-managed metadata 供固定 Wrangler create preflight 使用；token mutation、按 ID 管理和未知 ID 均 unsupported/not found。 |
| Queue `delivery_delay` | Wrangler 4.127.1 明确将字段视为 deprecated/no-effect，而当前 Cloudflare producer 文档仍描述该字段。P6 锁定 fixed-client 行为：wire 可接受、authority 不持久化；升级 Wrangler 时重新取 trace。 |
| SDK 7.1 multipart | typed `workers.scripts.update()` 的 header/bracket encoding 是精确 pin 的 wire exception；归一化范围由 bounded closed schema 和 regression 固定，SDK 升级时必须重取 trace，并在上游修复后删除例外。 |

这些 deviation 的收益是让 SMB 单机部署的公共 Wrangler/SDK 主路径可用，同时不伪造 Cloudflare 托管 DNS、credential
store 或 fleet 行为。它们不允许 secret 泄露、损坏状态静默修复或 unsupported capability 降级成功。

## 5. SMB 单机复杂度预算

本阶段 review 以单机 SMB 的常见可观察路径为目标：优先鉴权、secret、immutable authority、SQLite 完整性、
crash/restart、bounded input 和 fail-closed behavior；不为低概率、低影响长尾引入分布式状态机、兼容层或大规模
测试基础设施。缩小支持范围时必须在 capability/deviation 中明示，不能用宽松解析或 fallback 假装兼容。

当前接受一个 test-only 长尾：process Gate 先获取 ephemeral loopback port、释放后再由 child bind，理论上存在低概率
TOCTOU。它不位于 production listener 选择或 authority 路径，不影响数据和 secret；为完全消除此 race 引入跨平台
socket inheritance/复杂 harness 的收益不足，暂不实现。若它在支持平台成为可复现失败，则不再按长尾处理，应修复
根因或收窄 Gate 设计。

## 6. `cf-compatibility-check` 结论

正式 review 分别检查 committed branch、working tree、删除的旧 consumer 与相关 authority。P6 的 `/client/v4`
管理面按 skill 合同不进入 tenant runtime denominator，另由生成合同、官方 SDK 与固定 Wrangler Gate 验证；tenant
runtime 侧覆盖 D1、Queues、Workflows、Service Binding props、single-latest compatibility date/flags、pinned stock
workerd 和固定 upstream types。

review 最初发现三组问题并在冻结前修复：Service Binding 的中央 deviation 文档缺少静态 `props` 子集；D1 普通
mutation 同步整库 checkpoint 会造成无界成本；D1 expired transfer/checkpoint pin、terminal transfer file 与 authority
完成后 unlink error 会造成永久占用或把已完成操作误报 unknown。修复后未保留 actionable finding。

| changed surface | verdict | 冻结证据 |
| --- | --- | --- |
| Service Binding `ctx.props` | aligned with declared deviation | canonical object、64 KiB/depth 32、descriptor digest、runtime projection、restart/failure coverage；`OC-SERVICE-001` 已同步。 |
| Queues / Workflows | aligned | durable reservation/fence/delete、retry 与 capability lifetime 经 source/restart review 和真实 runtime Gate 覆盖。 |
| D1 Worker/Session | aligned with declared single-primary deviation | 普通 mutation 不复制整库；opaque bookmark 保持 local sequential visibility；显式 checkpoint/transfer 有界并由 `OC-D1-001` 声明。 |
| compatibility date/flags/types | aligned | workerd `v1.20260830.1`、effective date `2026-08-30`、workers-types `5.20260830.1`；没有 tenant-selectable historical branch。 |
| `/client/v4`、Wrangler、SDK | excluded from skill denominator | 由 P6 generated contract、`p6-cloudflare-sdk` 与 `p6-wrangler-resources` 单独闭环。 |
| global placement/replication/fleet limits | excluded self-host scope | 单机 SMB 画像明确，不移除 stable tenant API，也不弱化 local durability、security 或 restart contract。 |

本结论只覆盖 P6 changed surfaces，不声称整个平台已经达到 Cloudflare 全产品兼容。

## 7. 本机冻结验证

### 7.1 Scoped Gate

最终选择集为 `p0-2 p0-6 dashboard p3-services-product p6-cloudflare-sdk p6-wrangler-resources p3-contract`，
共 21 个 registry case，使用 pinned stock workerd 和正式 archive。Gate 遵守 stop-on-first-failure：第一次暴露 runtime
probe timeout 与错误 listener；第二次 20/21 通过并暴露 Wrangler fixture 使用 internal account UUID；第三次越过 account
discovery 后暴露 Vectorize tombstone 使后续 ready index 404。各次均保留 failure evidence，修复根因后才重跑。

- `20260903T173318-4116d954`：`p0-2` 2/2、`p0-6` 1/1、Dashboard 1/1、Service product 1/1、official SDK 1/1、
  P3 contract 14/14 通过；Wrangler 在 account ID fixture 错误处停止。
- `20260903T174344-c962bcd0`：最终受影响的 fixed Wrangler resource Gate 1/1 与 P3 contract 14/14 通过；报告
  `source_sha256=b4344f9911d091deaf541a967c55e8ec831151b077afaf9b4457820350161ad3`，15/15 PASS，90.15 秒。
- 最终 source baseline identity 为 `f80a995b3d2c971adfc3775ba629a1f16eb107059ce388d32bcf3e24fac44674`；
  generated subset/capability hash 仍分别为 `486e41018352aa664ef1035c75e13b69ee3bbe17bf3cf9c1b5b1267e738484e3`、
  `ea4230ac2c065a0e17f433d7bf7ebcdcae0e9a1c2a3e0c51b21dc4b9d351948b`。

后两次源码差异仅为 Wrangler/SDK fixture 纠正、Vectorize tombstone catalog root fix 与对应回归；未把失败 target
静默跳过，也未用重复轮次替代根因修复。由于 stop-on-failure policy，没有伪造一个不存在的全选择集单报告；上述
报告共同给出每个选择 target 在其相关最终实现上的通过证据。

### 7.2 Static 与 focused checks

| 检查 | 结果 |
| --- | --- |
| `bun run build` | PASS；extension type generation check、Dashboard build/typecheck、toolchain/runtime、examples/scripts/conformance 均完成。 |
| `cargo fmt --all --check` / `git diff --check` | PASS。 |
| `cargo metadata --no-deps --format-version 1` | PASS。 |
| `./test/check-boundaries.sh` | PASS。 |
| no-default-features | `OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE=... RUSTFLAGS='-D warnings' cargo check --workspace --no-default-features` PASS。 |
| MSRV | `OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE=... cargo +1.98.0 check --workspace --all-targets` PASS。 |
| focused regressions | D1 history/transfer/restart、Workflow reservation/delete、Service props、Vectorize tombstone、official SDK typed live path 均通过对应 focused tests。 |
| canonical Clippy | **FAIL，未声明 PASS**。完整 reachable run仍有 99 个 `service` lint diagnostics，主要为既有大函数/参数数、style 与 large error variant；继续批量机械改写的收益低于 P6 SMB 功能闭环，按复杂度预算保留为后续 debt。 |

按用户约定，最终 workspace Gate 与 Rust coverage 统一延期到 P9 完成后的单次全局验收，P6 不重复执行，也不把延期
写成 P6 PASS。Clippy 失败同样是明确未闭环项，不被 scoped Gate 覆盖。

## 8. 托管 Cloudflare differential 与保留边界

本机环境没有 `CLOUDFLARE_API_TOKEN` 和 `CLOUDFLARE_ACCOUNT_ID`，因此 P6 最终源码的 hosted differential 尚未
执行。独立 acceptance 文档规定了唯一资源前缀、inventory preflight、existing-resource collision、创建上限、
best-effort cleanup、二次 inventory 和 retained failure evidence；取得凭据前不得声称 Cloudflare-hosted PASS。

P7/P8/P9、remote dev/preview、`*.workers.dev`、Cloudflare billing/plan quota、multi-region placement/failover 和其它
未声明管理资源不属于 P6 完成范围。unsupported route/field/binding 保持中性 not-found 或 CF-style validation
failure，不增加本地近似实现。
