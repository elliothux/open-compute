# Operator API 与可选 Dashboard 完成记录

日期：2026-09-03

## 结论

**Implementation GO。** [`operator-api-dashboard.md`](operator-api-dashboard.md) 声明的本地 Day1 Operator API、
JavaScript SDK、可选 Dashboard、真实运行和浏览器验收范围已经实现。根目录 [`CR.md`](../../CR.md) 的复审没有
遗留 P1/P2 finding。

本次结论来自当前 frozen source，而不是 2026-09-02 的旧报告：

- `./scripts/dev-test.sh` 启动真实 `ocd`、SQLite、S3 fixture 和 formally pinned stock `workerd`；
- Dashboard Playwright **31/31**，live Operator SDK contract **12/12**；
- instrumented workspace Gate **42/42 targets、835/835 cases**；
- Rust 行覆盖率 **90.14%**；
- 最终 uninstrumented workspace Gate 按用户要求只运行一轮，**42/42 targets、835/835 cases**。

本记录不声明正式发行、跨平台或 Cloudflare billing/plan parity。

## 已实现范围

### 统一 Operator API

- 唯一在线管理根为 `/operator/api/v1/**`；旧管理路径没有 alias、redirect 或双注册。
- Dashboard 开关默认关闭，只控制 `/operator/` SPA；Operator API 始终随有效 admin listener 存在。
- API 根统一执行 Bearer admin auth、request ID、header/body bounds、脱敏错误映射和 metrics。
- `/operator/api/v1/meta`、`account`、`system/status` 与 Workers、KV、D1、R2、Durable Objects、Queues、
  Workflows、Scheduler、Cache、Images 的管理面共用同一安全边界。
- workerd 停止时，SQLite authority 与系统诊断类只读 API 仍可用；真正依赖 runtime 的动作返回稳定 503。

Catalog 的服务端契约统一为有界查询：

- `search` 由 SQL authority 执行，ID 输入走 typed exact match，名称走 bound parameter；
- 资源状态或 Worker `deployed` filter；
- `sort=name|createdAt|updatedAt` 与 `direction=asc|desc`；
- opaque cursor 绑定 sort、direction、last value 与 ID，错误类型或跨排序复用 fail closed；
- `limit` 归一到 1–1000，Dashboard 不通过无界全量读取伪造排序。

Workers、KV、D1、R2 handler 已加入实际 pagination/filter/sort/error matrix；Queue、Workflow 和 Durable Object
沿用各自已存在的有界 catalog/inspection 契约。

### Operator SDK

`packages/operator-sdk/` 是 Dashboard 与普通 JavaScript 客户端的唯一在线协议入口：

- TypeScript 7 strict、Zod strict schemas、typed operation registry、branded IDs 和 closed stable error union；
- 浏览器、Bun、Node.js 共享标准 `fetch` transport；每个公开调用支持 `AbortSignal`；
- 成功和失败 body 均先经过 bytes/content-type bounds，再由对应 schema 验证；
- mutation 不自动重试，idempotency key 由调用方显式提供；R2 upload/download 保持标准 stream；
- 没有 public raw request、unchecked generic response、旧 API 探测或第二套 DTO。

除基本 CRUD 外，SDK 已覆盖 Queue config、DO object get/delete、Workflow version/instance/step/action/event、
Worker cache inspect/purge、Platform workflow reconcile/cache GC、KV metadata/TTL、D1 query/table 与 R2 object stream。

### System-owned Dashboard

Dashboard 由 `packages/dashboard/` 构建为普通 assets-only Worker SPA。`crates/service/build.rs` 在 Cargo 消费前验证
产物清单与 SHA-256，并生成只读 embedded asset table；生产启动不需要 Bun、Node.js 或 TypeScript compiler。

bootstrap 直接建立一个 system-owned immutable Worker/deployment：

- authority 仍在 `control.sqlite`；artifact 仍由正式 S3/cache 路径管理；
- system-owned Worker 不允许 tenant/operator Worker API 修改、删除、上传或 promotion；
- 首次安装、ready deployment 幂等复用、缺失 artifact 恢复和错误 digest 拒绝均有真实 Gate；
- Dashboard Worker 没有 admin token、SQLite/S3/internal fetcher 或 control service binding。

### Dashboard 功能闭环

Dashboard 使用 React 19、Vite、TanStack Router/Query、Tailwind 与 Cloudflare Kumo。主要能力包括：

- 登录、同 tab `sessionStorage`、401 全局清理、Sign out、主题；
- 账号上下文、分组/折叠导航、recent、breadcrumb、文档入口和 `⌘K` command palette；
- Workers、KV、D1、R2、DO、Queues、Workflows catalog 的服务端搜索、过滤、排序、刷新、分页和 row actions；
- Worker create/upload/route/cache，KV value metadata/TTL，D1 query/table/migration/backup/restore，
  R2 object list/upload/download/delete，DO object inspection/delete，Queue configuration，Workflow instance actions，
  Platform scheduler/consumer/cache/images maintenance；
- mutation pending 防重复、成功/失败 Toast、authority query invalidation，以及危险操作的 Kumo 确认 Dialog；
- 390 px 响应式表格、键盘 focus、唯一页面 `h1`、section `h2` 与 `Primary navigation` landmark。

Kumo production CSS 的 Tailwind `@source` 指向实际 dashboard workspace dependencies。生产 CSS 约 153 KiB；
修复前缺失的 modal positioning、overlay、responsive utilities 和组件状态已由 Playwright 锁定。

### 本地开发入口

`./scripts/dev-test.sh` 管理隔离的 `.temp/dev-test/` 状态、S3 fixture、`ocd` 日志/PID 和端口，使用 formally
pinned workerd 输入，不在启动时下载 runtime。默认页面为 `http://127.0.0.1:8787/operator/`，开发 token 为
`dev-admin-token`。修复完成后连续运行两次 `./scripts/dev-test.sh smoke`：两次 live、ready、Dashboard 和鉴权
meta 均为 200，第二次复用了相同 SQLite authority 并验证重启/重新上传。原 `.data/` 未被删除或重置，持久开发
状态继续只由 `scripts/dev.sh` 管理。

## 与真实 Cloudflare Dashboard 的对比

2026-09-03 在同一 Chrome 会话中只读查看真实 Cloudflare Workers & Pages，再打开真实本地 Dashboard：

| 观察维度 | Cloudflare | open-compute 当前实现 |
| --- | --- | --- |
| 上下文与导航 | account、recent、分组产品、quick search、docs | account、recent、分组/折叠导航、`⌘K`、docs |
| Catalog controls | create、search/filter/sort/refresh | create、服务端 search/filter/sort、refresh、cursor pagination |
| Worker 摘要 | route/source/requests/latency/updated | route/source/requests/latency/updated；本机 process-lifetime 指标 |
| Usage | plan/billing 与托管 usage | `Usage since startup`，不冒充 billing |
| 主展示 | 卡片 | 更适合单机 operator 的 dense Kumo table |
| 产品操作 | Cloudflare hosted product workflows | open-compute 当前受支持的本地资源和维护动作 |

截图证据：

- `.temp/operator-dashboard-review/cloudflare-workers-final.png`
- `.temp/operator-dashboard-review/open-compute-workers-final.png`
- `.temp/operator-dashboard-review/open-compute-overview-after.png`
- `.temp/operator-dashboard-review/open-compute-platform-after.png`
- `.temp/operator-dashboard-review/open-compute-workers-mobile.png`
- `.temp/operator-dashboard-review/open-compute-create-dialog-after.png`

差异是明确的产品边界：open-compute 没有 Cloudflare plan/billing、Git provider 或全球网络计费数据；本地页面不为
追求像素复制而虚构这些 authority。

## 功能验证

### 浏览器与 SDK

| 检查 | 结果 |
| --- | --- |
| `bun run build` | 通过；SDK、Dashboard、toolchain、runtime 与所有 TS strict checks |
| `bun run test:js` | **226 pass / 0 fail / 1 live-only skip** |
| live `contract-ocd.test.mjs` | **12/12 pass** |
| `bun run test:dashboard:e2e` | **31/31 pass**，真实 `dev-test.sh`/`ocd` |
| `dashboard_gate` | **1/1 pass**，真实 stock workerd 与 system deployment 恢复 |

Playwright 覆盖登录/reload/401/sign-out、所有 catalog、search/filter/sort/pagination、Workers 与各 binding/product
detail、主要 mutation 的 success/validation/stable API error、pending 防重复、确认 Dialog、runtime warning、全局搜索
和 390 px 窄屏。它调用真实 Operator API，不用 mock server 替代产品路径。

### Rust 与仓库检查

| 检查 | 结果 |
| --- | --- |
| Service lib | 最终 workspace Gate 中 **239/239** |
| Storage catalog helper | cursor/token/typed search/SQL matrix **3/3** |
| `cargo fmt --all --check` | 通过 |
| canonical Clippy | 通过，all-targets/all-features/keep-going，warnings denied |
| no-default-features | 通过，warnings denied |
| Rust 1.98 MSRV | 通过，workspace all-targets |
| `cargo metadata --no-deps --format-version 1` | 通过 |
| `./test/check-boundaries.sh` | 通过 |
| `./test/check-production.py` | 通过 |
| Gate harness Python tests | **23/23** |
| `git diff --check` | 通过 |

### 覆盖率

`./test/coverage.sh` 的 instrumented Gate 在当前 Rust 源码上执行 **42/42 targets、835/835 cases**，用时
728.22 秒。报告位于：

- `target/llvm-cov/html/index.html`
- `target/llvm-cov/lcov.info`
- `target/llvm-cov/summary.json`

结果为 **72,186 / 80,085 lines，90.1367%**。

本轮同时修复 coverage 的证据污染：复用的 external-runner target dir 会保留旧 hash executable；此前
`cargo llvm-cov report` 会自动发现它们，产生 2,808 个 mismatched functions 并把覆盖率错误稀释到约 76%。现在
Gate build 明确 hard-link 本轮 compiler-artifact inventory，报告直接调用 active Rust toolchain 的
`llvm-profdata`/`llvm-cov` 并只传入这组 objects。当前 summary、LCOV 和 HTML 均来自同一轮 profile/object 集。

### 最终单轮 Gate

按用户明确要求，最终验收只执行：

```sh
OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE=/absolute/pinned/workerd-darwin-arm64.gz \
OPEN_COMPUTE_TEST_WORKERD=/absolute/pinned/workerd \
./test/gate.py --workspace
```

结果：

- rounds：**1**；
- targets：**42/42**；
- cases：**835/835**；
- processes：**42/42**；
- wall time：**647.76 秒**；
- report：`.temp/gate-run/20260903T040744-d3b1f88a/report.json`；
- source SHA-256：`9c163816f08f79842e614d7054b0e246635eb2d1615e665bf9a0a94643225da9`。

最终 Gate 使用仓库当前规定的单轮策略；没有额外重复同一冻结源码的 target 或 case。

## 修复过程中发现的回归

- Service binding lifecycle completion 在 product Gate 中出现一次 transient control-hop 未完成。runtime
  transport 现在用同一 operation identity 做三次有界重试；新增 JS regression，P3 services product/recovery
  随后通过。
- `test/conformance/baseline.json` 的 source identity 和 Workers types lock digest 已更新到当前权威输入；
  `p3-contract` 最终通过。
- production artifact hygiene 的单词扫描曾把 Kumo/Floating UI bundle 的 `onBeforeDispatch` 误认为 scheduler
  `BeforeDispatch` fault hook。检查改为要求完整 scheduler fault-marker 组，仍会拒绝真实 test-support enum 泄漏。
- Dashboard bootstrap Gate 增加 system artifact 丢失后的恢复和 ready deployment 重用，避免“首页能加载”掩盖
  重启/缓存失效问题。

## 接受的限制

- 没有运行正式 release packaging、签名、公证、npm 发布或其他目标平台验证。
- Cloudflare 远端只做 UI/信息架构只读对比，没有修改远端资源。
- Dashboard token 使用同 tab `sessionStorage` 以支持 reload；它不会跨 tab 或浏览器重启保留，也不进入
  `localStorage`、cookie、URL、日志或 Worker。
- 本机 usage 指标不等于 Cloudflare billing/plan 数据。

2026-09-02 的旧 Implementation Go/No-Go 段落和失效 source hash 已被本记录取代；历史 Gate 报告仍保留为实际运行
证据，但不再作为当前结论的依据。
