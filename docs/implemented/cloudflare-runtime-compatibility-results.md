# Cloudflare Runtime Day1 兼容改造完成报告

状态：**本地实现完成；Cloudflare 托管端资格为 Conditional Go**，2026-09-01。

本轮按 Day1 直接替换当前模型，没有保留旧 compatibility date/flags、旧自有 Cloudflare types、旧
outbound gateway、旧 wire/schema 或子集 facade 的兼容路径。`cf-compatibility-check` 复核未发现阻塞性
实现问题；唯一未完成项是 Cloudflare 托管端 Workflow differential，其账号权限条件由独立的
[剩余验收计划](../acceptance/cloudflare-runtime-compatibility-acceptance.md)跟踪。

## 冻结输入与能力结论

- Git 基线 revision：`188bbd833c666857cb87755ba343b469ca0f2729`；最终 workspace Gate 的 dirty
  source SHA-256：`e97209cd75eff01d32c8ea20025a22813ea47299cce556aeba50aabd64dd9021`。
- baseline `openComputeRevision`：`93dd12363d68cb70d2f57cf0b9e2837a925af690eddbe7393f3b9fd350761554`；
  baseline 文件 SHA-256：`e3602c6fd9ca7b04da62aa4c65ac0454c2d37ef12defb8a644265098c06005d5`。
- formal pin：`workerd v1.20260830.1`，revision
  `e9dda5963aba7ee4323960db795690ec78fec118`，`effectiveCompatibilityDate=2026-08-30`；
  `workerd.lock.json` SHA-256 为
  `4ccb7814a1ec72f50a3862cfac7475be6a4cade0ca646e8d16a2cb9aac42cb0f`。
- stable types：`@cloudflare/workers-types@5.20260830.1`；catalog SHA-256 为
  `302672ba7ee879b06e9fada90bb962d25741807c7fe575cbbbf2d4443c3df0db`，capability SHA-256 为
  `30b824b0841acf23d65700589941530a9785080ac38c50114bec1ac8edbfde18`。

目标 inventory 共 2,097 个 stable members/overloads：1,585 个 `supported`，512 个
`supported_with_deviation`，`blocked=0`。catalog 有 2,097 条 `memberEvidence`，没有 blocked contract 或
blocked member。

| 产品 | 成员数 | 结论 |
| --- | ---: | --- |
| Workers runtime | 1,580 | 1,556 `supported`；24 个 raw-TCP members 使用受控 self-host deviation |
| KV / R2 / D1 | 52 / 110 / 36 | 完整 Worker binding surface，保留单机 authority/placement deviation |
| Durable Objects / Alarms | 115 / 7 | namespace、ID、RPC/facet、storage/SQL、alarm、hibernation 与 connect 闭环 |
| Queues / Cron / Workflows | 63 / 26 / 72 | producer/consumer、事件调度、durable execution 与 recovery 闭环 |
| Cache API / Version Metadata / WebSocket hibernation | 14 / 3 / 19 | 完整成员 evidence |

## 关键实现

- tenant `fetch()`、`cloudflare:sockets.connect()`、`node:net` 与 `node:tls` 共用唯一
  `Network(allow=["public"])` general-outbound authority；Service/DO `Fetcher.connect()` 只走 deployment
  显式声明的 capability tunnel。旧 `OutboundGateway`、policy-version 字段和 HTTP-only 双路径已删除。
- Durable Objects 使用同一 stub 的 fetch/RPC/connect start ordering；facet abort 保留原始 reason 并拒绝
  后续旧 stub 调用。pinned workerd nested-facet 缺陷由稳定 hashed physical facet name 直接实现逻辑 path，
  没有版本选择分支或 legacy facade。
- KV、R2、D1、Queue 与 Workflow 的完整 stable surface、限制、错误、structured clone、transaction、
  output gate、restart/crash recovery 和资源 lifecycle 已进入各自 authority；Worker tombstone 会原子释放
  generic、Queue 和 Workflow referrer。
- runtime crash recovery 会安全回收“staging 已完整但尚未取得 child lease”的进程前状态，不再把可恢复
  的 macOS 启动中断当成永久损坏。
- tenant compatibility date/flags selector、自有 Cloudflare API declarations、canonical-JSON Workflow
  payload、旧 scheduler endpoints 和其它半截实现均已删除。

## 本地验收

| 检查 | 结果 |
| --- | --- |
| `bun run build` | PASS |
| `bun run test:js` | PASS，193/193 tests |
| `cargo fmt --all --check`、`git diff --check` | PASS |
| canonical Clippy（workspace/all-targets/all-features/keep-going/`-D warnings`） | PASS |
| no-default-features、Rust 1.98 MSRV all-targets、metadata、dependency boundaries | PASS |
| unchecked TS escape review | `@ts-ignore`、`@ts-nocheck`、`as unknown as` 均无命中 |
| 最终 workspace Gate | PASS，1 round、40/40 processes、802/802 cases、706.83 秒 |

最终 Gate 报告是 `.temp/gate-run/20260901T145516-c7ab80b8/report.json`，实际执行一个完整 round。
2026-09-03 起仓库最终与发行 Gate 统一采用单轮政策，因此该轮数满足当前 Gate 要求；这不改变报告记录的
源码、case 数、外部 qualification 状态或未执行的发行操作。

Rust line coverage 为 **90.17%**（68,383 / 75,839），报告位于
`target/llvm-cov/{html/index.html,lcov.info,summary.json}`。完整 instrumented workspace 的 40 个 Rust/runtime
targets 全部通过；当次 `coverage.sh` 进程仅因源码修改后的静态 `p3-contract` baseline digest 漂移而退出失败，
报告为 `.temp/gate-run/failed/20260901T143647-735f82aa/report.json`。baseline 修正后，`p3-contract` 在
`.temp/gate-run/20260901T144849-30604ba0/report.json` 通过，并用该完整 profile set 生成覆盖率报告及执行
`--fail-under-lines 90.00` 通过。这里不把失败的 wrapper 进程改写成一次完整 coverage PASS。

最终 workspace contract report 的 `localVerdict/platformVerdict` 仍为 `incomplete`，原因是最终本地 Gate
没有重跑 `p3-cf-diff`，其中 remote case 均记为 `not_run`；这与 `blocked=0` 的本地 member inventory 是
两个不同维度，不能把前者隐去。

## Cloudflare differential 与清理

同源 portable fixtures 已在真实 Cloudflare 与 open-compute 对照 Workers、Cache API、KV、D1、R2、
Durable Objects 和 Queues，公开 status/JSON 在允许归一化后逐字段一致。每轮只创建唯一 `oc-p34-*`
Worker 及 fixture 自有 binding，按精确 name/ID 删除并复查 absent；没有修改账号中已有服务。

Workflow fixture 已通过本地真实 `platformd`/stock workerd 路径，但 Wrangler OAuth 对 Cloudflare Workflow
inventory API 返回 `Authentication error [code: 10000]`，在只读 preflight 即停止，没有创建 Workflow、
Worker 或其它资源。随后冻结源码的合并复查在 D1 inventory preflight 收到同一错误；该次只完成并清理
Cache API Worker，D1 及后续资源未创建。刷新 OAuth 或更换 credential 会修改外部账号状态，未擅自执行。

退出复查未发现本轮遗留的 `platformd`、`workerd`、Node test 进程或 `open-compute-cf-diff-*` 容器。

## `cf-compatibility-check` 结论

- API/types：固定 upstream stable declarations、generated `Env`、capability/catalog 与 2,097 条成员 evidence
  双向一致；非目标 binding 在 authority 边界 fail closed。
- runtime wiring：tenant date/flags 不可选；仅一个 public Network；`PUBLIC_NETWORK` 只存在于 system Worker
  env，不进入 tenant binding、descriptor、capability 或公开错误。
- 安全与资源：private/loopback/link-local/metadata/Unix/DNS-to-private 由地址层拒绝；内部 listeners 保持
  loopback；secret、token、provider key 与拓扑不进入 tenant 响应或报告。
- 状态与恢复：SQLite/S3 authority、immutable deployment、DO/Queue/Workflow output gate、restart/crash、
  tombstone/referrer cleanup 均有真实进程回归。
- Day1：没有发现需要保留的旧 API/schema/runtime、dual read/write、fallback、alias 或兼容 shim。

结论是 Day1 本地 runtime 兼容实现完成且 `blocked=0`；在 Workflow hosted differential 和正式发行、
跨平台资格完成前，整体 Cloudflare 托管端/发行结论保持 Conditional Go。
