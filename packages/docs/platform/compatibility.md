# 兼容性

以这台机器上的二进制为准，不要用产品名字推导 Cloudflare 全量行为。未出现在 `capabilities --json` 里的 Cloudflare 功能视为不支持。本页不是一份完整 Cloudflare 矩阵。

```sh
ocd --config /etc/open-compute/config.toml capabilities --json
```

`--config` 对 `capabilities` 是可选的。省略时，`limits` 来自内嵌默认配置；给出绝对路径配置时，`limits` 反映该文件。JSON 顶层字段：`schema_version`、`release`、`runtime`、`products`、`limits`。

生成的成员索引（不要在文档里手抄 2,097 个签名）见 [API 参考](/platform/reference/api/)。已登记差异见[偏差](/platform/deviations)。运行中的数字上限见[限制](/platform/limits)。

## schema_version

固定为 `1`。其它值不要当这份契约来读。

## release

精确的发行身份，不是营销版本号。包含 `platform_version`、`git_revision`、`rust_msrv`、`workerd_version`、`workerd_lock_sha256`、`runtime_assets_sha256`、`facade_capability_version`，以及 control / scheduler / KV / D1 的 schema 版本和 `snapshot_format_version`。恢复和替换二进制时，拿这里的身份去对快照与 schema，而不是看文件名。

## runtime

pinned workerd 的固定 baseline：

| 字段 | 含义 |
| --- | --- |
| `effective_compatibility_date` | 正式 runtime lock 的唯一生效 compatibility date |
| `workerd_lock_sha256` | 正式 runtime lock 字节的 SHA-256 |
| `workers_types_version` | 固定的 `@cloudflare/workers-types` 版本 |
| `workers_types_git_head` | 该 types 包对应的 upstream git revision |
| `workers_types_package_sha256` | types 包 digest |
| `workers_types_index_sha256` | 固定 stable `index.d.ts` 字节 SHA-256 |
| `workers_types_ast_sha256` | 固定 stable 声明的 canonical AST SHA-256 |

不要改二进制旁的 workerd、不要 PATH 搜索、不要另下一份 runtime。digest 对不上就停。租户不能选 `compatibilityDate` 或 flags。

## products

按稳定产品名索引。每条包含：

| 字段 | 含义 |
| --- | --- |
| `status` | `supported` / `supported_with_deviation` / `blocked` / `unsupported` |
| `kind` | `target`（上游 AST inventory）、`platform`（本平台产品）、`non_target`（明确非目标） |
| `capability_version` | 完整支持时的静态 facade 版本；`blocked` / `unsupported` 时省略 |
| `members` | 目标产品的逐成员/overload 记录；每条有 stable id、symbol、member、kind、overload、readonly/optional/static、signature 与 `signature_sha256`、状态和证据 case |
| `deviations` | 已登记的单机拓扑或 stock-runtime 容量偏差 ID |

## 状态语义

- `supported`：该产品全部目标成员都有 compile 和真实 runtime 证据。
- `supported_with_deviation`：API 完整，仅存在已登记的单机拓扑差异。
- `blocked`：属于目标，但实现或证据未完成；不得声称兼容。
- `unsupported`：明确非目标产品。目标缺口不能标成 `unsupported`。没有精确证据的目标成员保持 `blocked`，不能用产品 smoke test 或类型存在推导支持。

当前 2,097 个目标成员没有 `blocked` 项。签名与 Cloudflare [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/) 相同的，以那边为准；本平台的差异只通过 `OC-*` ID 声明。

## 产品表

| 产品 | 状态 | 成员 | deviation |
| --- | --- | ---: | --- |
| Workers | `supported_with_deviation` | 1,580 | `OC-WKR-TCP-001`、`OC-WKR-LIMIT-001` |
| KV | `supported_with_deviation` | 52 | `OC-KV-001` |
| R2 | `supported_with_deviation` | 110 | `OC-R2-001` |
| D1 | `supported_with_deviation` | 36 | `OC-D1-001` |
| Durable Objects | `supported_with_deviation` | 115 | `OC-DO-001`（connect 成员另见 TCP/limit） |
| Alarms | `supported` | 7 | — |
| Queues | `supported_with_deviation` | 63 | `OC-QUEUE-001` |
| Cron | `supported_with_deviation` | 26 | `OC-CRON-001` |
| Workflows | `supported_with_deviation` | 72 | `OC-WORKFLOW-001` |
| Cache API | `supported_with_deviation` | 14 | `OC-CACHE-001`、`OC-CACHE-002` |
| Version Metadata | `supported` | 3 | — |
| WebSocket hibernation | `supported` | 19 | — |

`deployments`、`static_assets`、`service_bindings`、`workers_cache` 和有界本机 Images 是平台产品（`kind=platform`），分别带 `OC-DEPLOY-001`、`OC-ASSETS-001`、`OC-SERVICE-001`、`OC-CACHE-001` / `OC-CACHE-002`、`OC-IMAGES-001`。Images 不宣称完整托管 Cloudflare Images。

D1 覆盖 database/session/prepared-statement/result/meta、错误与 bind 转换、原子 batch、opaque bookmark 及当前 hosted 非 alpha `dump()` 拒绝。raw TCP general outbound 使用唯一 public Network 和 stock workerd 的 `cloudflare:sockets`/Node socket；命名 Service/DO 的 `Fetcher.connect()` 走显式 capability tunnel。

明确非目标产品见[不支持](/platform/unsupported)。
