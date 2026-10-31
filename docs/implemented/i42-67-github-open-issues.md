# I42–67：GitHub open issues 剩余实施批次

当前 CLI/应用配置与构建入口由 [P20](p20-cf-cli-migration.md) 拥有：`ocd cf`、`cloudflare.config.ts`、官方 Vite 插件 v2 与 Build Output。
本文旧客户端命令、版本、fixture 与 PASS 保留为当时证据，不要求恢复旧入口，也不证明当前 cf 的资格。

状态：**implemented**（2026-09-16）。原始批次覆盖 7 个 issues；其中
[#51](https://github.com/elliothux/open-compute/issues/51) 与
[#67](https://github.com/elliothux/open-compute/issues/67) 已完成；`#51` 归档为
[`P15`](p15-sqlite-refinery-migrations.md)，`#67` 的完整 Standard limits、公开 API 与恢复证据归档为
[`W2`](w2-standard-limits.md)。本实施记录
只保留其余 5 个 issues：
[#42](https://github.com/elliothux/open-compute/issues/42)、
[#58](https://github.com/elliothux/open-compute/issues/58)、
[#61](https://github.com/elliothux/open-compute/issues/61)、
[#62](https://github.com/elliothux/open-compute/issues/62)、
[#66](https://github.com/elliothux/open-compute/issues/66)。

2026-09-15 再次读取 GitHub 时共有 31 个 issues（25 closed、6 open）；新增的
[#68](https://github.com/elliothux/open-compute/issues/68) 是 capability-scoped TypeScript SDK，已由
[`P16`](p16-capability-scoped-typescript-sdk.md) 单独承接，晚于本批次上界且不依赖本批次实现，因此明确排除。
本文消费 P15 和 W2 已验证合同，不重复数据库 migration 方案，也不再实现第二套 CPU limiter、isolate
recovery、runtime liveness probe、workerd supervisor 或 limits API。

## 1. GitHub inventory 与范围

2026-09-13 读取 `elliothux/open-compute` 的全部 30 个 issues，当时为 23 closed、7 open。2026-09-14 完成
`#51` 和 `#67` 后，本批次剩余 5 项。2026-09-15 实现冻结时，新增 `#68` 已明确排除；本文列出的五项在
实现与验收完成后，于 2026-09-16 写入提交和验收证据并以 completed 关闭。

已关闭的是 `#1–#4`、`#17–#20`、`#36`、`#37`、`#41`、`#44–#48`、`#52–#54`、`#56`、`#57`、
`#59` 和 `#60`。它们不重新进入实现范围，不保留旧方案或兼容分支；最终 workspace Gate 继续覆盖其当前
产品回归。若现行行为再次失败，按新事实重新打开 issue，而不是在本计划复制历史实现。

剩余开放项分为三条责任线：

| 责任线                       | Issues              | Day1 结果                                                                                                |
| ---------------------------- | ------------------- | -------------------------------------------------------------------------------------------------------- |
| Worker 部署与 runtime 可用性 | `#66`、`#61`、`#62` | 标准 Wrangler 可部署；Cron drain 有界；deployment 只有验证可 dispatch 后才激活；runtime 故障可诊断和恢复 |
| operator outbound            | `#42`               | AI、target、release 与远程 S3 使用一个代理选择与安全合同，tenant egress 不变                             |
| AI Search 扩展               | `#58`               | namespaced manual source、exact revision、零完整源文件副本、复用现有 indexing pipeline                   |

## 4. `#66`：对齐 Wrangler multipart metadata

### 4.1 合同

问题只在 Worker upload multipart 的 `metadata` JSON：Wrangler 会发送 `package_dependencies`，当前
`WorkerUploadMetadata` 因 `deny_unknown_fields` 且未声明该字段而拒绝整个请求。multipart framing、module
parts、bundle 编译和 workerd 均不在本 issue 的修改范围。

保持 `WorkerUploadMetadata` closed decoder，新增可选的 `package_dependencies`，其元素使用 Wrangler 实际
wire shape 的严格结构：

```text
name
packageJsonVersion
installedVersion
```

真实 Wrangler fixture 决定 exact spelling 和 optionality。该字段只是客户端 build provenance；解析成功后
丢弃，不参与 runtime config、Worker code hash、bindings 或 limits，也不持久化或进入 tenant env/log。

不为这个字段增加独立 count、字符串或 aggregate budget；multipart metadata 已有统一的 1 MiB admission
上限。元素保留 `deny_unknown_fields` 和字符串类型校验，顶层也继续 `deny_unknown_fields`，避免把本次修复
扩大为接受任意 Wrangler metadata。

## 5. `#61`：Cron unknown outcome 必须收敛

### 5.1 durable attempt model

直接修改当前模型：`attempt` 表示一条逻辑 Cron run 已开始的 delivery 次数，而不是只统计收到明确失败响应的
次数。claim transaction 在任何外部 dispatch side effect 前将它从 0 增加到 1；transport unknown、process
restart 或 response loss 都已消耗本次 delivery。

Cloudflare 当前明确说明 Scheduled handler 最多运行 15 分钟，见
[Scheduled Handler](https://developers.cloudflare.com/workers/runtime-apis/handlers/scheduled/)。向 scheduler
Refinery lineage 追加下一条 migration，给 `cron_runs` 增加：

- `first_dispatched_at_ms`：首次成功 claim 的固定时间；
- `dispatch_deadline_at_ms`：由首次 claim 加 Standard 15-minute bound 物化，之后不延长；
- `last_unknown_reason`：closed low-cardinality token，只保存 transport-timeout、connection-loss、
  malformed-response 或 runtime-generation-lost；
- terminal reason 使用现有 history authority；若现有列不能表达，再在同一 migration 添加一个字段。

unknown lease recovery 只在 `attempt < 1 + cron_max_retries`、deadline 未到、activation 仍 `accepting` 时回到
ready。否则原子转为 terminal failure。known exception 继续服从 `controller.noRetry()` 和相同 delivery budget；
一次瞬时 unknown 可在剩余 budget 内成功，但不能通过 restart 重置。

### 5.2 draining

activation 转为 `draining` 后：

- 不再 claim 该 activation 的 ready runs；已有 ready runs 原子 terminalize 为 `activation_drained`；
- 已 claimed run 可完成当前 delivery，但 unknown/lease expiry 直接 terminalize，不再 requeue；
- stale late completion 由 claim token + activation generation 拒绝；
- `cron_activation_in_flight()` 只统计仍可能合法完成的 claim，因此必然在 current lease/deadline 内归零。

promotion 保留有界等待。如果当前 claim 在 drain timeout 内没有结束，返回专门的
`CronProjectionPending` operator diagnostic，包含 activation id 的非秘密 hash、剩余 bound 和 terminal reason
分类；Cloudflare-facing envelope 保持固定 shape。调用方重试会在上述确定上界后成功，绝不会永久循环。

## 6. `#62`：deployment admission、quarantine 与 crash diagnostics

### 6.1 active 不再等同 stored-ready

Version 的 `ready` 只表示 immutable artifact、bindings 和静态验证完成；Deployment 只有通过当前 formal workerd
generation 的 runtime admission 后才能成为 `workers.active_deployment_id`。

复用 `RuntimeValidator` 作为单一 deployment admission workflow，并在 storage 追加 mutable runtime-assessment
表；Deployment 和 Version 内容仍不可变。持久状态只有：

```text
dispatchable → quarantined
```

candidate 只是请求内的临时值，不写入数据库，也不计作 deployment。trusted gateway 通过 W1 Loader 加载 exact Version，执行 module compile/
initialization、entrypoint/binding resolution 和最小 non-handler probe；W2 startup/CPU/memory limits 全程生效，
不主动调用 tenant fetch/scheduled/queue handler。service 在 probe 前后读取同一 supervisor `StartupId`，Version id 与
source digest 继续由既有 authenticated loader envelope 校验。

只有 admission 成功、supervisor 仍是同一 Running generation、product promotion 已完成且 active pointer/route
generation CAS 仍匹配时，storage transaction 才创建带 exact `StartupId` 的 `dispatchable` assessment，并切换 100%
active pointer。generation 在验证和 commit 之间变化时，本次请求 fail closed；若 pointer 已提交则立即 quarantine
该新 Deployment 并回退，调用方的新请求必须重新验证，旧 deployment 保持 active。

### 6.2 runtime crash quarantine

W2 的 `RuntimeIncident` 由 service 关联到 generation-scoped in-flight deployment registry：

- candidate admission 期间的 crash 使未持久化 candidate 失败，旧 active 不动；
- active dispatch 期间若 incident snapshot 只有一个 tenant deployment，原子 quarantine 它，并把 active pointer
  CAS 回最近一个仍 dispatchable 的 Deployment；没有前任时只让该 Worker 变为无 active deployment；
- 多 deployment 同时在途时不猜测元凶，也不批量 quarantine。W2 先恢复 service，incident 标记为
  `attribution_ambiguous`，由 operator 获取诊断。该并发歧义是 single-process profile 的明确限制，不能用错误
  quarantine 健康 tenant 来伪造精确隔离；
- supervisor functional restart、operator drain、正常 ResourceLimit 和 client disconnect 不计作 deployment
  crash strike。

quarantined Version/Deployment 不删除，不能再次激活；修复必须创建新的 immutable Version。自动 rollback 只改
active pointer/route generation并写 audit，不修改旧 Version、Durable Object storage 或 product data。

### 6.3 诊断与可观察状态

runtime crate 继续生成 bounded redacted `ProcessDiagnostics`；service-owned recorder 将每次 generation incident
原子写入一个有界、secret-scanned 的 `data/diagnostics/workerd/last-exit.json`，内容只包括 timestamp、startup
id、restart reason、exit code/signal、reader status、截断后的已 redacted stdout/stderr tail、内容 digest 和
deployment attribution class。新记录替换旧记录，不建立无界 crash log。

namespaced system status 与 support bundle 暴露：

- supervisor state/startup id、最近 incident summary、是否已恢复；
- dispatchable/quarantined counts 和 sanitized quarantine reason；
- active deployment 的 `runtime_dispatchable`，不能在 supervisor unavailable 时暗示正在 serving。

Cloudflare Deployment API 仍返回 immutable deployment resources；open-compute-only assessment 只出现在
namespaced status/capability surface。deployment create 在 candidate 未通过时返回失败，不先返回 active success。

## 7. `#42`：operator-owned HTTP proxy

### 7.1 一个冻结的单代理策略

本批次不实现 OS proxy discovery 或 per-scheme proxy。`ocd` 启动时按以下固定顺序取第一个非空值，得到唯一
operator proxy，并冻结到本次进程生命周期：

```text
HTTPS_PROXY -> https_proxy -> ALL_PROXY -> all_proxy -> HTTP_PROXY -> http_proxy -> direct
```

选中的 proxy 同时用于外部 operator-owned HTTP 和 HTTPS 请求；这是 open-compute 的单代理合同，
不声称完整复制其他 HTTP client 的 per-scheme 环境变量语义。

request 选择顺序是：

1. loopback、当前平台 listeners、runtime internal endpoints 和 `#58` manual provider endpoint 永远强制
   direct，不能被环境变量覆盖；
2. `NO_PROXY` 优先于 `no_proxy`，第一个非空值按 host/domain suffix/IP/CIDR/`*` 匹配，命中则 direct；
3. 其余外部请求使用冻结的单一 proxy；没有 proxy 时 direct。

第一版只接受无 userinfo 的 canonical `http://host:port` proxy URL；非空但无效、带 credential、使用 HTTPS、
SOCKS、PAC 或其他 scheme 时启动 fail closed，不回退 direct。HTTP 目标使用 absolute-form，HTTPS 目标使用
`CONNECT` tunnel；CONNECT 后仍使用既有 webpki roots，不增加 interception CA 信任。

在 `crates/core` 放一个 transport-neutral `OperatorProxyPolicy`，只负责解析、验证和保存选中的 proxy origin、
来源变量及 `NO_PROXY` 规则。在 `crates/service/src/operator_http.rs` 建立关闭自动 redirect/环境代理的 direct/proxy
双 `reqwest` client；
`ai_provider`、`target_http` 和 `release_http` 继续拥有各自 request/response、redirect、timeout 和 size policy，
不重复解析环境变量或创建不同 proxy 规则。

远程 S3-compatible artifact storage 同样属于 operator-owned outbound，必须消费相同 policy。现有
`aws-smithy-http-client` 原生 `ProxyConfig` 负责 HTTP proxy 和 HTTPS `CONNECT`，同时保留当前 AWS-LC、webpki
roots、SigV4、timeout、retry 和 connection-pool 合同。loopback S3 endpoint 仍强制 direct。

public Git import 保持当前 DNS/address pinning 和 `.no_proxy()`，不能把解析权交给 proxy；runtime bridge、
supervisor probe、Wrangler subprocess 和 tenant workerd outbound 也不接入该 policy。显式 proxy unreachable
必须 fail closed，不能 direct fallback。release redirect 每一跳继续执行既有 downgrade/redirect bound 并重新
应用同一个 policy。

operator 文档明确说明：macOS System Settings 中启用 Surge/Whistle 本身不会影响 `ocd`；必须在实际启动
`ocd` 的 shell、launchd 或 service environment 中设置上述变量。本批次不实现 SystemConfiguration、PAC、
WPAD、SOCKS、proxy authentication、custom CA 或单独的 open-compute proxy config。

## 8. `#58`：namespaced manual external source

### 8.1 public boundary

新增唯一 source type：`open-compute:manual`。Cloudflare 当前
[Workers Items binding](https://developers.cloudflare.com/ai-search/api/items/workers-binding/) 的 upload 明确写入
built-in storage；本扩展不重新解释该方法，也不改变 `builtin`、`r2`、official `PUT /items` 或 sync contract。
instance 创建时指定 operator provider id；不做 initial scan、scheduled scan、full reconcile 或 delete diff。

`#58` 只能增加 open-compute namespaced API，不能修改、替换或放宽任何已经声明兼容的 Cloudflare API。官方
management routes、Worker binding methods、request/response fields、source enum、错误、默认行为、类型声明和
conformance member count 在启用或未启用 manual source 时都保持不变；尤其不能让 official `PUT /items`、upload、sync
或 `source` 字段接受 manual provider。扩展名称、配置和类型必须带 `open-compute` namespace，Cloudflare client 不会
发现或调用它；无法隔离时本功能保持 unsupported。

bound instance 增加明确 namespaced 方法：

```ts
await env.SEARCH.items.openComputeUpsert({
  key: "files/blob-123",
  revision: "immutable-revision",
  contentType: "application/pdf",
  metadata: { team_id: "team-1" },
  waitForCompletion: false,
});
```

类型只由 `@open-compute/workers-types` 的 `open-compute:ai-search` module 导出；不修改或冒充
`@cloudflare/workers-types` 的官方声明。runtime facade 对非 manual instance 调用该方法返回稳定 extension
error。已有 `items.delete/list/get/download` 和 search response shape 继续复用。

### 8.2 provider authority 与 wire

`AiConfig` 增加 closed `source_providers` map。Day1 只实现 `loopback_http` provider：固定
`http://127.0.0.1:<port>`/`[::1]` endpoint、允许的 account ids、provider source namespace、credential
env/file reference，以及现有 AI Search source-byte bound。tenant 只能引用 provider id，不能提交 URL、header、
credential 或代码。

provider private protocol 只有两个 authenticated POST operations：

- `resolve {source, key, revision}` → exact revision、content type、size、SHA-256；
- `read {source, key, revision}` → bounded byte stream，并在 headers 重复 exact revision、size、SHA-256。

禁止 redirect、compressed transfer、chunk count/size overflow 和非 loopback resolution。read 前后 metadata 必须
一致，实际 bytes 重算 SHA-256/size；missing、revision drift 或 mismatch 使本 generation terminal failure，旧
active generation 保持可搜索。provider credential 和 endpoint不写入 instance database或公共 metadata。

### 8.3 persistence 与 indexing

把当前 `AiSearchSourceReference` 收敛为一个 tagged immutable locator：builtin object、R2 revision、manual
provider revision。manual locator 只持久化 provider id、source、key、revision、observed size/digest/content type；
不持久化完整源 bytes。

若 AI Search V1 不能直接表达 tagged locator，向 `ai_search/` lineage 追加下一条 per-instance migration，把当前
builtin/R2 rows 一次转换为新权威形态，然后删除代码中的双读/双写和 R2-specific fallback。control database 如需
provider locator，则独立向 `control/` lineage 追加下一条 migration；两个文件按各自 history 恢复，不增加组合版本。

coordinator 复用现有 parse、OCR、chunk、embedding、FTS/vector staging、parse cache、generation fence、retry、
activation 和 GC。只新增 `ManualSourceReader`；它把 provider stream写入现有 bounded disposable parser staging，
不进入 AI Search object storage。normalized text、parse cache、chunks、embeddings、job/history 是允许的 derived
state。

相同 `(provider, source, key, revision)` upsert 幂等，不重新 parse/embed；revision 改变创建新 item generation，
旧 generation 在新 generation 完整成功前 active。delete 只移除 locator和 derived state，从不调用 provider
delete。snapshot 保存 locator和 derived database，不包含源文件；restore/reindex只有在同 provider exact
revision仍可读时继续，否则 fail closed。

## 13. 最终验收证据

实现前已将本地 `main` fast-forward 到上游 `3f4ed33ab3609fdcb95bb92f80eb2f92b0815dc0`，随后按 Day1
模型解决工作区冲突并重新生成 OpenAPI/SDK；未保留旧生成物或兼容读取路径。

- `bun run build`、`bun run check:frontend`、`bun run test:js`（273/273）、P6 contract/Vinext（11/11）、
  conformance（14/14）和 Gate harness（25/25）通过；
- `cargo fmt --all --check`、`./test/check-rust-clippy.sh`、no-default-features、Rust 1.98 MSRV、Cargo metadata、
  dependency boundaries 和 `git diff --check` 通过；
- `./test/coverage.sh` 单次执行通过，workspace 行覆盖率为 **90.03%**（134604/149514，最低 90.00%）；
  内嵌 Gate 为 52/52 processes、1532 cases，报告为
  `.temp/gate-run/20260916T001218-265df85b/report.json`；
- source freeze 后唯一一轮 `./test/gate.py --workspace` 通过：52/52 processes、1532 cases，耗时 1248.62 秒，
  报告为 `.temp/gate-run/20260916T003641-704fc4a4/report.json`；
- 覆盖率报告位于 `target/llvm-cov/html/index.html`、`target/llvm-cov/lcov.info` 和
  `target/llvm-cov/summary.json`。失败诊断完整保留在 `.temp/gate-run/failed/`；对应的 promotion validator、
  conformance digest、非交互 Git 与 Node compile-cache 根因均已在最终冻结源码中修复。
