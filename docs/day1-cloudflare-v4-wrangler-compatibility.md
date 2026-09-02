# Day 1 Cloudflare v4 API 与 Wrangler 子集兼容设计

状态：设计完成，尚未实施与验收

日期：2026-09-03

本文定义 open-compute Day 1 唯一的在线管理协议和 Worker 项目配置契约。实施完成后：

- `/client/v4` 是唯一在线管理 API 根；
- `wrangler.jsonc` 是唯一 Worker 项目配置；
- 上游 Wrangler 是唯一部署协议客户端；
- Dashboard、自动化程序和 Lynx deployment broker 都调用同一组 v4 API；
- open-compute 特有能力仍使用 v4 的认证、包络、分页和错误语义，但位于明确的
  `open-compute` vendor namespace；
- 彻底移除 `/operator/api/v1`、`open-compute.json`、Operator SDK 和自定义 deployment upload 协议；
- 旧 API URL 只表现为未注册路由的 `404 Not Found`，不保留 alias、redirect、`410 Gone` shim、双写或
  feature flag。

这是 greenfield Day 1 replacement，不是兼容迁移。实现不以旧 Operator API 的请求、响应、错误、状态机或客户端
行为为约束；若它们与本文或固定 Cloudflare 合同冲突，直接删除旧逻辑，以新合同为准。

本文取代 [`Operator API 与可选 Dashboard Day1 方案`](implemented/operator-api-dashboard.md) 中关于 API
根路径、Operator SDK 和项目配置的目标设计。旧文档及其结果仍是当前实现的历史证据，不代表本文已经实施。
本文也不改变 [`Cloudflare Workers 兼容矩阵`](references/cloudflare-compatibility.md) 已记录的 runtime 证据；
但按本文实施时，必须把其中“tenant 不得选择 compatibility date 或 flags”的当前限制改成以 Worker Version
为单位的兼容日期和 flag 合同，并重新取得对应 Gate 证据。

Workers Logs、固定 Wrangler 的 realtime tail 和 Observability Telemetry 子集由
[`Day 1 Workers Logs 与 realtime tail 兼容设计`](day1-workers-logs-realtime-tail.md) 细化。该专项是本文 official
subset 的组成部分，不是 vendor logs API；其中未完成的实现与 Gate 也属于本文最终验收条件。

Workers Standard 的 structural/runtime limits 由
[`Day 1 Workers Standard limits 设计`](day1-workers-standard-limits.md) 细化；公开的 `worker_loaders` binding、
`WorkerLoader.load/get` 和 nested stock-workerd Gate 由
[`Day 1 Dynamic Workers / Worker Loader 设计`](day1-dynamic-workers-worker-loader.md) 细化。后者只覆盖
Dynamic Workers，不包含 Workers for Platforms 或 dispatch namespaces。

## 1. 结论与边界

Day 1 采用“一套协议、两个 namespace”的结构：

```text
Wrangler / Dashboard / automation / Lynx deployment broker
                         |
                         | Bearer token
                         v
                 https://<origin>/client/v4
                         |
             +-----------+----------------+
             |                            |
             v                            v
  Cloudflare-compatible subset     open-compute extensions
  /accounts/{id}/workers/...       /open-compute/...
  /accounts/{id}/storage/kv/...    /accounts/{id}/open-compute/...
  /accounts/{id}/d1/...                         |
             |                                  |
             +----------------+-----------------+
                              v
                domain services / SQLite / S3 / workerd
```

这不是两套 API。两类路由共享同一个 listener、鉴权、请求 ID、错误模型、分页规则实现和客户端 transport。
namespace 只回答一个问题：这个资源名和行为是否属于 Cloudflare 的公开合同。

路由归属遵循以下规则：

1. Cloudflare 已有对应资源模型时，只实现官方路径，不在 vendor namespace 重复一份 CRUD。
2. Cloudflare 没有对应概念时，才进入 `open-compute` namespace。
3. Lynx 的团队、成员、文件、桌面和应用目录是产品业务 API，不属于基础设施管理面，不进入
   `open-compute` namespace；它们应由普通 Worker 服务提供。
4. `/health/live`、`/health/ready` 和 `/metrics` 是进程运维端点，不伪装成 Cloudflare API。

## 2. 兼容承诺

“兼容 Cloudflare”不表示实现 Cloudflare 全部产品。本文只承诺一个显式、可验证的子集，并把兼容拆成四层：

| 层次 | Day 1 承诺 |
| --- | --- |
| 路径兼容 | 支持范围内使用 Cloudflare `/client/v4` 的 method 与 path，不另造等价资源路径 |
| wire 兼容 | 请求字段、multipart 结构、响应包络、资源 ID、时间、分页和错误尽量逐端点匹配官方 schema |
| 客户端兼容 | 固定版本的上游 Wrangler 和 Cloudflare SDK 可以通过自定义 API base URL 调用，不维护 fork |
| runtime 兼容 | 上传的 compatibility date、flags、modules 和 bindings 进入 stock workerd；本地拓扑差异单独登记 |

每个端点只允许三种状态：

- `supported`：请求、响应和可观察语义均在声明范围内兼容；
- `supported_with_deviation`：wire 合同兼容，但单机或内网拓扑导致已登记的语义差异；
- `unsupported`：不注册端点，或返回明确的 CF-style 错误；绝不静默忽略。

机器可读能力清单仍以 `share/cloudflare-capabilities.json` 为唯一真值。实施时应在现有 manifest 中增加
`managementApi` 和 `wrangler` 两部分，不建立第二份互相漂移的 capability 文件。人类可读矩阵由该 manifest
和 conformance catalog 生成或校验。

**固定上游版本。**

当前设计基线是仓库 lockfile 中的：

- `wrangler@4.127.1`；
- `cloudflare@7.1.0`；
- `workerd v1.20260830.1`；
- `@cloudflare/workers-types@5.20260830.1`。

Day 1 只认证精确的 Wrangler 和 Cloudflare SDK 版本，不声明宽泛的 semver 范围。Wrangler 的内部请求序列不是
稳定公开 API；Cloudflare 官方 SDK 又由 OpenAPI 自动生成，其 minor release 也可能调整 method、structure 或 type
名称。升级任一客户端必须先记录请求 trace、比较 config/schema 与 multipart metadata，再运行本文定义的兼容
Gate。

## 3. API 根、认证与 listener

Wrangler 通过环境变量选择 open-compute：

```bash
export CLOUDFLARE_API_BASE_URL="https://compute.example.internal/client/v4"
export CLOUDFLARE_API_TOKEN="<token>"
export CLOUDFLARE_ACCOUNT_ID="<account-id>"
```

`CLOUDFLARE_API_BASE_URL` 已由固定版本 Wrangler 读取，但它不是允许无验证升级 Wrangler 的承诺。目标地址和
token 不写入 `wrangler.jsonc`，因此同一份项目配置可以通过切换环境变量部署到 Cloudflare 或 open-compute。

所有 `/client/v4/**` 请求使用：

```http
Authorization: Bearer <token>
```

open-compute Day 1 选择一个 installation、一个 account，以及三类 token；这是平台 scope，不由 LynxOS 的团队
规模或某个安装的用户数推导：

| token | 用途 |
| --- | --- |
| admin | 所有官方子集和 vendor 运维扩展 |
| deployer | Worker 与授权资源的读写，不含整机维护和恢复 |
| read-only | Dashboard catalog、状态和资源读取 |

内部可以将权限压缩成上述角色；对外仍返回稳定的 scope 名称。Agent、用户生成的应用和浏览器页面不得持有
admin token。它们通过显式 Worker binding 或 Lynx 的部署 broker 获得有限能力。

为了让固定 Wrangler 的账号发现和 `whoami` 工作，Day 1 实现以下最小标准端点：

```text
GET /client/v4/user
GET /client/v4/user/tokens/verify
GET /client/v4/accounts
GET /client/v4/accounts/{account_id}
GET /client/v4/memberships
```

这些端点只返回当前 token 可见的本地 account 和最小成员信息，不引入 Cloudflare organization、billing、plan
或 OAuth login。非交互部署仍推荐显式设置 `CLOUDFLARE_ACCOUNT_ID`。

listener 边界保持不变：`ocd` 是唯一公网入口，control API 只在配置的 admin listener 暴露。即使 admin
listener 绑定 loopback，v4 API 也必须鉴权。tenant route 永远不能覆盖 `/client/v4`、`/health`、`/metrics`
或 Dashboard 保留路径。

## 4. 通用 v4 wire contract

**JSON 包络。**

官方兼容端点逐个采用对应 Cloudflare OpenAPI schema，不用一个自定义 DTO 覆盖所有产品。典型成功响应为：

```json
{
  "success": true,
  "result": {},
  "errors": [],
  "messages": []
}
```

分页端点按对应资源返回 `result_info`。不得把现有 Operator API 的统一 cursor 强加给所有资源：KV key list、
Workers Versions 和 account catalog 使用哪一种 cursor 或 page/per_page，都以各自的官方端点为准。

Cloudflare 的不同端点和版本对 `errors`、`messages` 的空值存在细节差异。open-compute 以固定的官方 schema
snapshot 和固定 Wrangler 的实际解析行为为准，不擅自做“全局统一”。vendor endpoint 默认返回数组。

以下响应不包 JSON envelope：

- Worker script/content 的 multipart 或原始内容下载；
- KV value 的原始字节；
- R2 object 的原始字节和 HTTP metadata；
- 未来明确支持的流式日志或导出下载。

**命名、ID 和时间。**

- JSON 使用 Cloudflare 字段的 `snake_case`，不沿用 Operator SDK 的 camelCase DTO。
- 外部 Worker 身份是 `script_name`，不能暴露内部 `worker_id` 作为官方路径主键。
- account、KV namespace、D1、Queue、Version 和 Deployment 分别遵守对应官方资源的 ID 语法。
- 内部仍可使用 UUID 或其他主键，但需要稳定映射；内部 ID 不泄漏到兼容响应。
- 所有 API 时间为带时区的 ISO 8601 字符串；内部毫秒整数不得直接出现在 v4 响应。
- opaque cursor、bookmark、ETag 和 upload token 不允许客户端解释或构造。

项目处于 Day 1，实施时直接修改 authoritative schema 和 ID 生成规则，不为旧开发数据库增加映射表、双读或
backfill。已有本地 `.data` 不能未经授权删除；新 binary 应对旧的不兼容 schema 明确拒绝启动。

**错误。**

典型错误响应为：

```json
{
  "success": false,
  "result": null,
  "errors": [
    {
      "code": 9101004,
      "message": "Binding type worker_loader is not supported by this open-compute release",
      "source": {
        "pointer": "/metadata/bindings/3/type"
      }
    }
  ],
  "messages": []
}
```

错误规则如下：

1. 已知的官方失败条件使用对应 Cloudflare code、HTTP status、message shape 和字段 pointer。
2. open-compute 独有错误使用整数保留段 `9,100,000..9,199,999`，不冒用含义不一致的官方 code。
3. unsupported、quota、conflict、not found、invalid input、authentication 和 transient failure 必须可区分。
4. retryable 错误返回合适的 `Retry-After`；客户端断开不能被解释为服务端已经回滚。
5. response 不包含路径、SQL、module source、signed URL、internal listener、token 或上游异常文本。
6. 不伪造 `cf-ray` 或 `server: cloudflare`。本地追踪使用 `X-Open-Compute-Request-Id`。

建议的 vendor code 一级分段为：

| 范围 | 类别 |
| --- | --- |
| `9100000..9100999` | 路由、协议和 capability |
| `9101000..9101999` | Worker upload、binding 和 compatibility metadata |
| `9102000..9102999` | 本地资源与容量限制 |
| `9103000..9103999` | 备份、恢复和维护 |
| `9104000..9104999` | scheduler、reconcile 和 repair |

具体 code 必须在一个 Rust authority 中定义，并由 OpenAPI、SDK 和测试读取；不得在各 handler 内散落数字。

## 5. Worker 的官方资源模型

当前 open-compute 的“deployment”实际同时承担了不可变代码版本和激活指针两个概念。新模型直接对齐
Cloudflare：

```text
Script
  ├── Version A  immutable modules + bindings + compatibility metadata
  ├── Version B  immutable modules + bindings + compatibility metadata
  └── Deployment traffic assignment
          └── Version B: 100%
```

Day 1 数据模型固定为：

- **Script**：以 `script_name` 标识的逻辑 Worker；
- **Version**：不可变代码、modules、vars、secret references、bindings、assets、compatibility date/flags 和
  runtime source snapshot；
- **Deployment**：Version 的流量分配记录；创建新 Deployment 才改变 active routing；
- **Route/endpoint**：指向 Script 的入口，不拥有代码版本。

现有 immutable deployment 数据结构在实施时直接改名并成为 Version authority；现有 promotion/rollback active
pointer 变成 Deployment authority。不得继续保留一套 `promotion`/`rollback` public endpoint。

Day 1 只接受一个 Version、`percentage: 100` 的 Deployment。多 Version percentage rollout 使用官方请求
shape，但返回明确 unsupported 错误；不能悄悄取第一个 Version。Rollback 的实现是创建一个指向历史 Version
的新 Deployment，而不是修改历史 Deployment 或 Version。

**Workers API 子集。**

以下路径是 Day 1 必须支持的官方子集：

| 领域 | Method 与 path | Day 1 语义 |
| --- | --- | --- |
| Scripts | `GET /accounts/{account_id}/workers/scripts` | catalog、官方分页和 Script summary |
| Scripts | `GET /accounts/{account_id}/workers/scripts/{script_name}` | 下载当前 Script 内容/metadata，按官方 content negotiation |
| Scripts | `PUT /accounts/{account_id}/workers/scripts/{script_name}` | 标准 multipart；内部原子创建 Version 和 100% Deployment |
| Scripts | `DELETE /accounts/{account_id}/workers/scripts/{script_name}` | 经过引用检查后 tombstone Script，不删除历史 artifact |
| Versions | `POST /accounts/{account_id}/workers/scripts/{script_name}/versions` | 标准 multipart；只创建不可变 Version |
| Versions | `GET /accounts/{account_id}/workers/scripts/{script_name}/versions` | list，支持固定 Wrangler 使用的 query |
| Versions | `GET /accounts/{account_id}/workers/scripts/{script_name}/versions/{version_id}` | 返回 immutable Version detail/resources |
| Deployments | `GET /accounts/{account_id}/workers/scripts/{script_name}/deployments` | deployment history |
| Deployments | `POST /accounts/{account_id}/workers/scripts/{script_name}/deployments` | 只允许单 Version 100% |
| Deployments | `GET/DELETE /accounts/{account_id}/workers/scripts/{script_name}/deployments/{deployment_id}` | 查询或删除非当前 Deployment；当前指针不可悬空 |
| Settings | `GET/PATCH /accounts/{account_id}/workers/scripts/{script_name}/script-settings` | 仅接收已支持字段和明确的 disabled/empty 值 |
| Tails | `GET/POST /accounts/{account_id}/workers/scripts/{script_name}/tails`，`DELETE .../tails/{tail_id}` | 固定 Wrangler 的 `trace-v1` realtime tail；filter、session、overload 和 WebSocket 合同见专项设计 |
| Secrets | `GET/PUT /accounts/{account_id}/workers/scripts/{script_name}/secrets`，`GET/DELETE .../secrets/{secret_name}`，`PUT .../secrets-bulk` | 支持固定 Wrangler 的 list/get/put/delete/bulk 请求 |
| Cron | `GET/PUT /accounts/{account_id}/workers/scripts/{script_name}/schedules` | 以完整 schedule collection 更新映射现有 Cron authority |
| Subdomain | `GET/POST/DELETE /accounts/{account_id}/workers/scripts/{script_name}/subdomain` | 只表达 `enabled=false`、`previews_enabled=false`；启用请求明确拒绝 |

`PUT Script` 和 `POST Version` 都是 Cloudflare 标准 API，不代表内部存在两套部署模型。前者只是兼容型组合操作，
必须调用同一个 create-Version 和 create-Deployment domain service。

固定 Wrangler 在 Version 上传后可能 PATCH `script-settings`。服务端按专项设计支持 logs-only
`observability` 字段；空 tail consumers、空 destinations、`logpush=false` 和 traces disabled 等值可以接受，因为
它们真实描述本地状态。启用 Tail Workers、streaming tails、traces、非空 destinations 或 Logpush 必须明确拒绝，
不能当作 no-op 返回成功。

**私有部署入口的明确 deviation。**

内网 open-compute 不能诚实提供 `<script>.<account>.workers.dev`，上游 Wrangler 又会固定拼接
`.workers.dev`。因此 Day 1：

- `wrangler.jsonc` 必须设置 `workers_dev: false`；
- `preview_urls: true` 不支持；
- `route`/`routes` 暂不支持，直到实现真实的 Zone/route API、内部 DNS 和 TLS 合同；
- 不返回虚假的 workers.dev hostname；
- open-compute 自动生成的本地入口通过 vendor endpoint 查询：

```text
GET /client/v4/accounts/{account_id}/open-compute/workers/{script_name}/endpoints
```

该 endpoint 返回当前 installation 实际可访问的 URL、listener 类型和是否启用，不创建第二个部署接口。
未来如果实现预配置内部 Zone，应增加 Cloudflare Zone/Workers Routes 官方子集，再允许标准 `route`/`routes`，
而不是重新解释 Wrangler 字段。

## 6. Multipart Worker upload

**请求合同。**

`PUT Script` 和 `POST Version` 接受 `multipart/form-data`。请求至少包含名为 `metadata` 的 JSON part；有
user Worker 时，再包含 `main_module` 或 `body_part` 指向的代码 part，以及 metadata/bindings 引用的其他
module part。

典型 metadata：

```json
{
  "main_module": "index.js",
  "compatibility_date": "2026-08-30",
  "compatibility_flags": ["nodejs_compat"],
  "bindings": [
    { "name": "MODE", "type": "plain_text", "text": "production" },
    { "name": "CACHE", "type": "kv_namespace", "namespace_id": "<kv-id>" },
    { "name": "DB", "type": "d1", "id": "<d1-id>" }
  ]
}
```

Day 1 接受固定 Wrangler 为现有 runtime 能力生成的 module 类型：ES module、service-worker script、text、data、
Wasm 和 source map part。Python、container、Cap'n Proto、package dependency 等未声明类型必须在 artifact
写入 authority 前拒绝。

解析器必须：

1. streaming 读取并执行 header、part count、单 part、metadata 和总大小上限；
2. 拒绝重复 part name、非法 UTF-8、控制字符、路径穿越、绝对路径和未引用的敏感 part；
3. 在落库前验证 `main_module`/`body_part`、MIME、module graph、binding name、资源归属、compatibility date/flags
   和所有引用；
4. 对每个 part 计算 digest，写入现有 content-addressed artifact authority；
5. 只有全部 artifact verified 后，才在一个 SQLite transaction 中创建 Version；
6. 请求失败或 transaction 失败时不产生可见 Version，未引用 artifact 由正常 GC 回收；
7. `secret_text` 进入现有加密 authority，明文不进入日志、错误、GET response 或普通 artifact。

本地上限可以低于 Cloudflare 托管端，但必须由 capability manifest 和文档公开，并以 `413` 或对应的稳定
CF-style validation error 失败。不能为了兼容而在内存中无界 buffer 整个上传。

**Wrangler 字段到 upload metadata。**

| `wrangler.jsonc` | multipart metadata/binding |
| --- | --- |
| `vars` string | `plain_text` |
| `vars` JSON | `json` |
| `kv_namespaces` | `kv_namespace` + `namespace_id` |
| `r2_buckets` | `r2_bucket` + `bucket_name` |
| `d1_databases` | `d1` + `id` |
| `durable_objects.bindings` | `durable_object_namespace` + class/script |
| `queues.producers` | `queue` + `queue_name`/`delivery_delay` |
| `workflows` | `workflow` + workflow/class/script |
| `services` | `service` + service/entrypoint |
| `images` | `images` |
| `version_metadata` | `version_metadata` |
| `assets.binding` | `assets`，并由 `metadata.assets` 引用 completion token |
| `rules` 发现的 Wasm/text/data | 相应 module binding 和 multipart part |
| `compatibility_date`/`compatibility_flags` | Version immutable metadata |
| `cache`/`exports`/DO `migrations` | 对应官方 metadata 字段 |

Queue consumers、Cron triggers 等不是 Worker env binding。固定 Wrangler 会在上传前后调用对应官方资源 API；
open-compute 按官方操作顺序执行，不增加私有“全量配置事务”。如果 Version 已创建而 trigger 更新失败，Version
仍存在，Wrangler 报告部署未完全成功；这是需要覆盖的恢复路径。

**compatibility date 与 flags。**

`compatibility_date` 在 Day 1 必填，并随 Version 持久化。服务端验证：

- 日期格式正确；
- 位于当前 pinned workerd 和 capability manifest 声明的范围内；
- 每个 flag 都被当前 pin 识别并允许；
- date/flag 组合能由 stock workerd 编译和加载；
- 同一 Version 重启后使用完全相同的 date/flags。

服务端不得把 tenant 指定的日期替换成全局 `effectiveCompatibilityDate`，也不得静默删除未知 flag。system
Workers 仍可使用 release lock 中独立固定的 system compatibility flags。

## 7. Static Assets upload

Static Assets 不能把现有 `deployment-uploads` 私有协议换个路径继续暴露。Day 1 实现 Wrangler 使用的官方三段
流程：

```text
1. POST /accounts/{account_id}/workers/scripts/{script_name}/assets-upload-session
   body: { manifest }
   result: { jwt, buckets }

2. POST /accounts/{account_id}/workers/assets/upload?base64=true
   Authorization: Bearer <upload-token>
   multipart asset parts
   result: completion token

3. PUT Script 或 POST Version
   multipart metadata.assets = { jwt: <completion-token>, config: ... }
```

实现要求：

- manifest path、hash、size、数量和总大小有界；路径必须 canonical 且不能逃逸；
- `buckets` 只返回缺失的 content-addressed objects；已存在且 digest/size 匹配的 object 可复用；
- upload token 与 account、script、manifest digest、缺失 object 集合、过期时间绑定，并使用独立签名上下文；
- asset 上传校验 base64、hash、size、MIME 和 token scope；
- completion token 只在 manifest 全部满足后签发，且只能完成匹配的 Version；
- completion 不直接激活 Worker；激活仍由 Deployment 完成；
- asset-only Worker 允许 metadata 没有 `main_module`，但必须有有效的 assets completion token；
- `html_handling`、`not_found_handling`、`run_worker_first` 和 assets binding 使用官方字段和值；
- restart/crash 后 upload session 可继续或明确过期，不能把半完成 manifest 当作 ready deployment。

## 8. 标准资源 provisioning API

资源必须先通过官方 API 创建，再由 multipart binding 引用。Worker upload 不隐式创建一个名称相近的资源，
也不调用 vendor provisioning endpoint。上游 Wrangler 的显式 resource command 和该版本支持的 automatic
provisioning，只要最终调用下列标准端点，就复用同一 authority。

**KV。**

```text
GET/POST  /accounts/{account_id}/storage/kv/namespaces
GET/PUT/DELETE
          /accounts/{account_id}/storage/kv/namespaces/{namespace_id}
GET       /accounts/{account_id}/storage/kv/namespaces/{namespace_id}/keys
GET/PUT/DELETE
          /accounts/{account_id}/storage/kv/namespaces/{namespace_id}/values/{key_name}
GET       /accounts/{account_id}/storage/kv/namespaces/{namespace_id}/metadata/{key_name}
PUT       /accounts/{account_id}/storage/kv/namespaces/{namespace_id}/bulk
POST      /accounts/{account_id}/storage/kv/namespaces/{namespace_id}/bulk/get
POST      /accounts/{account_id}/storage/kv/namespaces/{namespace_id}/bulk/delete
```

value GET 返回原始 bytes；metadata、expiration、cursor 和 URL encoding 按官方 endpoint schema。现有 KV
backup/restore 没有等价 CF API，移入 vendor namespace，不混入上述路径。

**D1。**

```text
GET/POST  /accounts/{account_id}/d1/database
GET/PUT/PATCH/DELETE
          /accounts/{account_id}/d1/database/{database_id}
POST      /accounts/{account_id}/d1/database/{database_id}/query
POST      /accounts/{account_id}/d1/database/{database_id}/raw
POST      /accounts/{account_id}/d1/database/{database_id}/export
POST      /accounts/{account_id}/d1/database/{database_id}/import
GET       /accounts/{account_id}/d1/database/{database_id}/time_travel/bookmark
POST      /accounts/{account_id}/d1/database/{database_id}/time_travel/restore
```

`wrangler d1 migrations` 使用标准 query/import 能力和数据库内的 migration ledger。现有
`/migrations/apply` 私有控制面路径不保留。export/import/time-travel 可以复用现有 snapshot/backup primitives，
但 response、bookmark 和恢复原子性必须按官方端点重新建模并取得差分证据，不能只改 URL。

**R2。**

Day 1 支持固定 Wrangler 的 bucket catalog 和 object 操作：

```text
GET/POST  /accounts/{account_id}/r2/buckets
GET/PUT/DELETE
          /accounts/{account_id}/r2/buckets/{bucket_name}
GET/PUT/DELETE
          /accounts/{account_id}/r2/buckets/{bucket_name}/objects/{object_name}
```

R2 jurisdiction、custom domain、managed domain、Sippy、event notification、lifecycle、lock 和 catalog 不在 Day 1
子集。bucket/object 请求如果出现这些字段或子路径必须明确失败。Worker binding 内已经实现的 R2 multipart
upload 不等于控制面必须新增私有 multipart endpoint。

**Queues。**

```text
GET/POST  /accounts/{account_id}/queues
GET/PUT/DELETE
          /accounts/{account_id}/queues/{queue_id}
GET/POST  /accounts/{account_id}/queues/{queue_id}/consumers
GET/PUT/DELETE
          /accounts/{account_id}/queues/{queue_id}/consumers/{consumer_id}
```

Queue 名称与 ID、producer binding、consumer Worker、batch、timeout、retry、delay、dead-letter queue 和最大并发
映射到现有 Queue/Scheduler authority。pull consumer 和 `visibility_timeout_ms` 不在 Day 1 子集。

**Workflows。**

```text
GET       /accounts/{account_id}/workflows
GET/PUT/DELETE
          /accounts/{account_id}/workflows/{workflow_name}
GET       /accounts/{account_id}/workflows/{workflow_name}/versions
GET       /accounts/{account_id}/workflows/{workflow_name}/versions/{version_id}
GET/POST  /accounts/{account_id}/workflows/{workflow_name}/instances
POST      /accounts/{account_id}/workflows/{workflow_name}/instances/batch
GET
          /accounts/{account_id}/workflows/{workflow_name}/instances/{instance_id}
PATCH     /accounts/{account_id}/workflows/{workflow_name}/instances/{instance_id}/status
POST      /accounts/{account_id}/workflows/{workflow_name}/instances/{instance_id}/events/{event_type}
```

具体 method、batch/terminate 子路径和 result shape 以固定 Wrangler trace 和官方 schema snapshot 为准。
Day 1 config 只支持本文第 10 节列出的 Workflow 字段；hosted retention、fleet concurrency 等未实现字段不得
接受后丢弃。

**Durable Objects、Cron、Service Binding、Cache 与 Images。**

- Durable Object namespace 由 Worker 的 `exports` 或 `migrations` 生命周期声明创建、重命名和 tombstone；不保留
  手工 namespace CRUD API。object registry 的只读诊断属于 vendor extension。
- Cron 使用 Workers Script Schedules 官方 API；scheduler pause/repair 是 installation 运维能力，属于 vendor
  extension。
- Service Binding 完全由 Version multipart metadata 声明，不需要单独 provisioning API。
- Cache policy 使用固定 Wrangler 的 `cache`/`exports` metadata；平台 cache GC 是 vendor 运维能力。
- Images binding 使用 `images` metadata；本地 capacity/queue 状态属于 vendor 运维能力。

## 9. open-compute vendor namespace

扩展仍位于 `/client/v4`，但路径必须显式包含 `open-compute`：

```text
installation scope
GET  /client/v4/open-compute/capabilities
GET  /client/v4/open-compute/system/status
GET  /client/v4/open-compute/scheduler
POST /client/v4/open-compute/scheduler/pause
POST /client/v4/open-compute/scheduler/resume
POST /client/v4/open-compute/scheduler/repair
GET  /client/v4/open-compute/cache
POST /client/v4/open-compute/cache/garbage-collection
GET  /client/v4/open-compute/images/capacity

account/resource scope
GET  /client/v4/accounts/{account_id}/open-compute/workers/{script_name}/endpoints
GET  /client/v4/accounts/{account_id}/open-compute/durable-objects
GET  /client/v4/accounts/{account_id}/open-compute/durable-objects/{namespace_id}/objects
POST /client/v4/accounts/{account_id}/open-compute/kv/namespaces/{namespace_id}/backups
GET  /client/v4/accounts/{account_id}/open-compute/kv/namespaces/{namespace_id}/backups
POST /client/v4/accounts/{account_id}/open-compute/kv/backups/{backup_id}/restore
POST /client/v4/accounts/{account_id}/open-compute/d1/databases/{database_id}/backups
GET  /client/v4/accounts/{account_id}/open-compute/d1/databases/{database_id}/backups
POST /client/v4/accounts/{account_id}/open-compute/d1/backups/{backup_id}/restore
```

该清单只覆盖当前实现确有基础能力的扩展。整机备份、node/cluster 管理或 license 等未来能力不能因为预留了
namespace 就提前增加空 endpoint。

extension 规则：

- 与官方端点共用 Bearer token、scope enforcement、envelope、pagination、request ID 和审计日志；
- JSON 仍使用 `snake_case`；
- 使用 vendor error code 保留段；
- 不在路径中再嵌 `/v1`，因为 `/client/v4` 已经是协议版本；
- additive 字段可以在 v4 内增加，breaking change 必须进入未来整体 API version，不能加隐式 mode switch；
- `capabilities` 返回 release、精确认证的 Wrangler 版本、compatibility date 范围、flags、endpoint 状态、deviation
  IDs、Standard contract 与部署方显式 capacity，但不暴露 secret、内部 URL 或 filesystem path。

`master-key rotate`、控制库任意 SQL、强制清除 data-dir、手工修改 generation fence 等高风险操作不开放 HTTP，
只允许本机离线 CLI/runbook。内网部署不是放弃安全边界的理由。

## 10. `wrangler.jsonc` 子集

### 10.1 语法 authority

`wrangler@4.127.1/config-schema.json` 是配置语法 authority。open-compute 不复制一份改名 schema，也不再解析
`open-compute.json`。兼容范围按字段是否只影响本地构建、是否进入 multipart、是否触发已支持 API 三类判断。

字段通过 Wrangler schema 不代表 open-compute 实现了其远端能力。最终 authority 是：

```text
Wrangler schema validation
  -> Wrangler environment resolution / local build
  -> v4 endpoint request validation
  -> multipart metadata + binding validation
  -> stock workerd compile/load validation
```

任一层不支持都必须失败，不允许 warning 后继续部署一个语义不同的 Worker。

### 10.2 支持字段

| 类别 | Day 1 字段 | 说明 |
| --- | --- | --- |
| 项目标识 | `$schema`, `name`, `account_id`, `main` | `main` 对 assets-only 可省略；account 也可由环境变量提供 |
| runtime 版本 | `compatibility_date`, `compatibility_flags` | date 必填；按 Version 持久化和验证 |
| 环境 | `env` | 使用 Wrangler 官方继承规则；bindings/vars 等 non-inheritable 字段不另造继承 |
| 本地构建 | `base_dir`, `build`, `define`, `find_additional_modules`, `jsx_factory`, `jsx_fragment`, `keep_names`, `minify`, `no_bundle`, `preserve_file_names`, `rules`, `tsconfig` | 由固定 Wrangler 或项目 build adapter 处理；服务端只验证产出的 multipart |
| 变量 | `vars` | string 与 JSON，分别成为 `plain_text` 和 `json` |
| KV | `kv_namespaces[].binding`, `id` | `preview_id`/`remote` 只属于 Wrangler local dev，不构成 remote dev 承诺 |
| R2 | `r2_buckets[].binding`, `bucket_name` | `jurisdiction`、`local_dev` 不进入 server 子集 |
| D1 | `d1_databases[].binding`, `database_name`, `database_id`, `migrations_dir`, `migrations_table`, `migrations_pattern` | migration path/table 由 Wrangler 本地命令消费 |
| Durable Objects | `durable_objects.bindings[].name`, `class_name`, 可选 `script_name` | 同 account；`environment` Day 1 不支持 |
| DO lifecycle | `migrations` 的 `tag`, `new_sqlite_classes`, `renamed_classes`, `deleted_classes` | `new_classes` 非 SQLite storage 不支持 |
| Declarative exports | `exports` 的 Worker entrypoint 和 SQLite DO `created`/`renamed`/`deleted` | container、cross-script transfer 不支持；与 `migrations` 的互斥沿用 Wrangler schema |
| Queues producer | `queues.producers[].binding`, `queue`, `delivery_delay` | queue 必须已存在或由标准 provisioning 创建 |
| Queues consumer | `queue`, `max_batch_size`, `max_batch_timeout`, `max_retries`, `dead_letter_queue`, `max_concurrency`, `retry_delay` | `visibility_timeout_ms` 不支持 |
| Workflows | `binding`, `name`, `class_name`, `schedules` | `script_name`、limits、retention、hosted concurrency 需另行实现后才加入 |
| Service Binding | `binding`, `service`, `entrypoint` | `props` 和 `remote` 不属于 server 子集 |
| Static Assets | `directory`, `binding`, `html_handling`, `not_found_handling`, `run_worker_first` | 使用官方 upload session 和 multipart token |
| Cron | `triggers.crons` | `triggers.events` 不支持 |
| Cache | `cache.enabled`, `cache.cross_version_cache` 和受支持的 Worker export cache override | 映射现有 Cache authority |
| Images | `images.binding` | `remote` 只是 local dev 字段，不改变 server binding |
| Version Metadata | `version_metadata.binding` | 不保留 `open-compute.json` 曾有的非标准 `tag` 字段 |
| Standard limits | `usage_model: "standard"`, `limits.cpu_ms`, `limits.subrequests` | immutable Version state；只有 stock workerd 真实执行后才能开放，完整矩阵与 fail-closed Gate 见 [limits 专项](day1-workers-standard-limits.md) |
| Worker Loader | `worker_loaders[].binding` | Day 1 Dynamic Workers 目标；当前受 upstream stock workerd nested-loader/limits/cache G0 阻断，见 [Worker Loader 专项](day1-dynamic-workers-worker-loader.md) |
| Observability logs | `observability.enabled`, `head_sampling_rate`, `logs.enabled`, `logs.head_sampling_rate`, `logs.invocation_logs`, `logs.persist` | Script-level non-Version setting；Workers Logs 与 realtime tail 的独立语义见专项设计；`destinations` 只接受空数组 |
| Secrets declaration | `secrets.required` | 只影响本地 type/dev validation；值由 `wrangler secret` 管理，不写入配置 |

`wrangler dev` 的纯本地模式由上游 Wrangler/Miniflare 提供，不是 open-compute server conformance 的证据。
`wrangler dev --remote`、edge preview 和 preview URL 在 Day 1 不支持。需要在真实 open-compute runtime 验证时，
使用单独的 dev environment 上传 Version/Deployment。

仓库维护的框架 adapter 可以继续使用 Rolldown 和 TypeScript 7，但部署 transport 必须交给上游 Wrangler。若
adapter 需要生成构建后配置，应使用 Wrangler 官方 `.wrangler/deploy/config.json` 跳转机制，生成的目标仍是
标准 `wrangler.jsonc`；不得生成 `open-compute.json` 或直接调用私有 upload API。

### 10.3 明确不支持的字段

Day 1 不支持的常见远端能力包括：

```text
workers_dev: true
preview_urls: true
route / routes
observability.traces
observability.logs.destinations（非空）/ observability.traces.destinations（非空）
logpush / tail_consumers / streaming_tail_consumers
placement
ai / ai_search / ai_search_namespaces / websearch / agent_memory
vectorize / hyperdrive / browser
analytics_engine_datasets
dispatch_namespaces
containers / cloudchamber
mtls_certificates / ratelimits
pipelines / stream / media
secrets_store_secrets / artifacts / flagship
vpc_services / vpc_networks
send_email
unsafe
```

该列表不是 future Wrangler schema 的自动 denylist。实施时从固定 schema 和固定 multipart generator 提取完整字段
inventory，每个字段都有 capability 状态。升级新增字段默认 unsupported，经过实现和 Gate 后才能转为 supported。

### 10.4 最小示例

```jsonc
{
  "$schema": "./node_modules/wrangler/config-schema.json",
  "name": "invoice-app",
  "main": "src/index.ts",
  "compatibility_date": "2026-08-30",
  "compatibility_flags": ["nodejs_compat"],
  "workers_dev": false,

  "vars": {
    "APP_ENV": "production"
  },

  "kv_namespaces": [
    { "binding": "CACHE", "id": "<kv-namespace-id>" }
  ],

  "d1_databases": [
    {
      "binding": "DB",
      "database_name": "invoice-db",
      "database_id": "<d1-database-id>",
      "migrations_dir": "migrations"
    }
  ],

  "r2_buckets": [
    { "binding": "FILES", "bucket_name": "invoice-files" }
  ],

  "services": [
    { "binding": "TEAM_FILES", "service": "lynx-files" }
  ],

  "queues": {
    "producers": [
      { "binding": "EVENTS", "queue": "invoice-events" }
    ]
  },

  "assets": {
    "directory": "dist/client",
    "binding": "ASSETS",
    "not_found_handling": "single-page-application",
    "run_worker_first": ["/api/*"]
  },

  "triggers": {
    "crons": ["0 2 * * *"]
  },

  "version_metadata": {
    "binding": "CF_VERSION_METADATA"
  }
}
```

endpoint、account 和 token 不需要提交到项目：

```bash
CLOUDFLARE_API_BASE_URL=https://compute.example.internal/client/v4 \
CLOUDFLARE_API_TOKEN="$OPEN_COMPUTE_DEPLOY_TOKEN" \
CLOUDFLARE_ACCOUNT_ID="$OPEN_COMPUTE_ACCOUNT_ID" \
npx wrangler@4.127.1 deploy
```

## 11. Secrets

`wrangler.jsonc` 不保存远端 secret value。Day 1 支持固定 Wrangler 的：

```text
wrangler secret put
wrangler secret list
wrangler secret delete
wrangler secret bulk
wrangler versions secret ...
```

服务端遵守 immutable Version 模型：

- 修改 Script 当前 secret 时，内部创建继承当前代码和非 secret bindings 的新 Version，并创建 100%
  Deployment；
- Version-specific secret 操作只创建或更新目标 Version 所允许的 secret binding，不原地修改已经 ready 的
  Version；若官方 endpoint 语义要求新 Version，则返回新 Version；
- `keep_bindings`、`keep_secrets` 和 `bindings_inherit=strict` 按固定 Wrangler 请求处理；
- GET/list 只返回 secret 名称和类型，绝不返回值；
- secret plaintext 只在请求处理和加密边界短暂存在。

这里必须以固定 Wrangler trace 和官方 endpoint schema 为最终准绳。不能为了复用当前 `open-compute.json` 的
env-reference secret 设计而改变官方 request shape。

## 12. Dashboard、SDK 与工具链

`operator` 保留为 UI/角色概念，不再是协议概念：

- Dashboard 可继续由 `/operator/` 提供，但所有数据请求都发往 `/client/v4`；
- 官方资源页面调用官方兼容路径；Platform/maintenance 页面调用 `open-compute` extension；
- Dashboard 不直接访问 SQLite、S3、binding backend 或 workerd internal listener；
- Dashboard 不手写第二套 URL、DTO、分页和错误解析。

现有 `@open-compute/operator-sdk` 在实施时删除。Day 1 不生成一套覆盖标准资源的
“open-compute Cloudflare SDK”，因为那会重复 Cloudflare 官方 SDK 已经完成的 OpenAPI generation、分页、上传、
错误和 retry 逻辑。调用面按用途固定为：

| 调用方 | Day 1 客户端 |
| --- | --- |
| `wrangler deploy` 与资源命令 | 固定版本的上游 Wrangler；不经过任何本地 SDK |
| Node/Bun 自动化与 Lynx deployment broker | 固定版本的官方 `cloudflare` TypeScript SDK，设置 open-compute `baseURL` |
| Dashboard | 官方 SDK 的 documented tree-shakable entrypoint，只打包支持的资源；复用同一 extension binding |
| open-compute vendor endpoint | 从 extension OpenAPI 生成 TypeScript types，薄 wrapper 调用官方 SDK 已公开的 `get/post/put/patch/delete` transport |
| 其他语言或直接 HTTP 用户 | 使用发布的 bundled OpenAPI 或直接 HTTP；Day 1 不维护其他语言的 open-compute SDK |

标准资源的 TypeScript 调用保持官方写法：

```ts
import Cloudflare from "cloudflare";

const client = new Cloudflare({
  apiToken: token,
  baseURL: "https://compute.example.internal/client/v4",
  maxRetries: 0,
});

const accounts = await client.accounts.list();
const namespaces = await client.kv.namespaces.list({ account_id: accountId });
```

`maxRetries` 是调用方策略，不是 wire compatibility 差异。Dashboard 默认关闭自动重试，避免对缺少幂等保证的交互式
mutation 做隐式重放；只读请求或具有明确幂等语义的操作可以单独开启。外部用户仍可使用官方 SDK 的默认策略，服务端
必须正确处理重复、条件和冲突语义。

Wrangler 和官方 SDK 的 base URL 配置名不同：固定 Wrangler 使用 `CLOUDFLARE_API_BASE_URL`；当前官方
TypeScript SDK 使用构造参数 `baseURL`，也读取 `CLOUDFLARE_BASE_URL`。open-compute 的示例和进程配置可以从同一
installation endpoint 派生这两个值，但不能假设上游工具共享一个环境变量。

vendor extension 不进入 Cloudflare SDK namespace，也不 fork 官方 package。建议只提供
`@open-compute/cloudflare-extension`：它接收已经创建的官方 `Cloudflare` client，导出 extension methods，例如：

```ts
const extension = createOpenComputeExtension(client);

await extension.system.status();
await extension.backups.create({ account_id: accountId, resource_id: resourceId });
```

这个 package 没有自己的 auth、fetch、retry、pagination 或 error transport。request/response types 由 extension
OpenAPI 生成；少量 operation wrapper 可以手写，但 path、method、operation ID 和类型必须由 contract test 对照
OpenAPI。不要用手写 Zod/TypeScript interface 再复制一份 schema authority。

OpenAPI 的职责是合同、文档、类型生成和 conformance，不是再造 transport。建议目录为：

```text
openapi/
  upstream/cloudflare-openapi.lock.json  # 固定 revision/hash 的官方 snapshot
  cloudflare-subset-manifest.json        # 选择支持的 operation
  cloudflare-v4-subset.yaml              # 从 snapshot 生成，不手改
  open-compute-extension.yaml            # open-compute 自有 source schema
  open-compute-v4.yaml                    # bundled 发布物，生成
```

`cloudflare-v4-subset.yaml` 中的 operation、字段和 response 必须从固定的官方 schema snapshot 提取并记录来源
revision/hash；不能凭记忆手写一个“类似 Cloudflare”的接口。extension schema 可以自有，但必须复用相同的
common envelope、error 和 pagination components。

Rust 服务端不从这份大 schema 生成 router 或 domain model。handler request/response 使用清晰的 `serde` 类型和显式
validation，domain service 保持唯一 authority；OpenAPI schema test、官方 SDK contract test、真实 Wrangler trace
和 Cloudflare differential 共同检查 wire 结果。multipart、raw bytes 和 streaming endpoint 优先复用 Wrangler 或
官方 SDK 已有编码，不增加本地通用 upload abstraction。

现有 TypeScript toolchain 只保留本地 build、type generation、framework output adapter 等 Wrangler 没有替代的
职责；它不再拥有项目配置解析、认证、resource CRUD 或 deployment transport。`oc deploy`/`oc run` 若仍存在，
只能是调用固定上游 Wrangler 的薄入口，不能直接请求另一套 API。

## 13. 旧 Operator API 的彻底删除

这里的“删除”不是把 `/operator/api/v1` 重定向到 `/client/v4`，也不是留下一个返回新版错误包络的兼容层。
完成切换后，Operator API 不再是一项可运行、可链接、可导入或可测试的协议。`operator` 只允许继续作为
Dashboard UI 和管理员角色的名称。

实施不安排 deprecation window、兼容版本或先迁移消费者再保留旧服务的过渡发布。v4 handler、官方 SDK
调用、extension binding、Wrangler config、Dashboard 和自动化消费者在同一个 Day 1 变更中落地，旧实现也在该
变更中删除。旧测试不能成为新实现的兼容性要求；测试应根据固定 Cloudflare schema、Wrangler trace、本文 vendor
contract 和 domain invariant 重新编写。

**必须删除的实现面。**

| 实现面 | 删除要求 |
| --- | --- |
| HTTP route | 删除 `operator_api_router`、`/operator/api/v1/**` 以及历史 `/v1/accounts/**`、`/v1/operator/**`、`/v1/scheduler/**` 的注册、嵌套、not-found handler 和旧鉴权 middleware |
| wire contract | 删除旧 request/response DTO、Zod/schema、错误码映射、cursor、upload/finalize 协议和只为旧 API 存在的 header |
| SDK | 删除 `packages/operator-sdk/`、`@open-compute/operator-sdk` workspace dependency、所有 import、type test 和生成/打包入口 |
| toolchain | 删除 `createOperatorClient` deployment transport、旧 base URL 校验和 `open-compute.json` 驱动的 deploy/run 路径；上游 Wrangler 负责部署 transport |
| Dashboard | 全部 query/mutation 改用官方 SDK 与 extension binding；源码、测试、mock 和构建产物中不得再出现旧 URL 或 Operator SDK |
| service plumbing | 删除只服务旧路径的 body limit 判定、route label、metrics classifier、allowlist、path parser、sanitizer 和内部 helper 命名 |
| tests/fixtures | 有效能力测试迁到 v4；旧合同测试、mock server、fixture 和 differential probe 删除，不能把旧请求的成功响应保留成“回归测试” |
| operations | dev-test、runbook、example、部署脚本和告警查询改用 v4 或独立 health/metrics endpoint；不再探测旧 status URL |
| generated output | 重新生成并检查 lockfile、bundle、OpenAPI/client artifacts；仓库不得因 `dist` 或缓存继续发布旧 client 与 URL |

底层 domain services、SQLite/S3 authority 和与 wire protocol 无关的 domain invariant 可以复用，但需要逐项确认
符合 Day 1 合同。旧 DTO、旧错误、旧状态名、旧分页、旧 upload/finalize 状态机和旧 handler flow 不因“复用”而
保留。新的 v4 handler 应直接调用唯一的 domain services；不能让 v4 handler 在进程内请求、包装或适配旧
Operator router。

**旧 URL 的唯一行为。**

- `/operator/api/**` 必须在 public、admin 和 merged listener 上返回普通未匹配路由的 HTTP 404；
- 响应不得是 2xx、3xx、410，不得返回旧错误 DTO，也不承诺 Cloudflare v4 envelope；
- `/operator/{*rest}` Dashboard SPA fallback 必须明确排除 `/operator/api/**`，旧 API URL 不能返回
  `text/html` shell；
- merged listener 必须保留该前缀，避免旧 API 请求落入 tenant Worker 的 public ingress；这个保留只用于返回
  中性的 404，不解析认证、不调用 domain service，也不记录成兼容 endpoint；
- 不发送 deprecation、successor、rewrite 或 redirect header，不做 method/path/content-type 协商。

因此，404 断言只是证明旧 surface 已消失，不是一项继续维护的 legacy API contract。删除完成后可以保留一组
集中式 negative route inventory；不能把每个旧 handler、DTO 和业务 case 连同旧协议一起留在测试代码里。

**当前能力的迁移归属。**

| 当前能力 | Day 1 去向 |
| --- | --- |
| `/operator/api/v1/meta` | `/client/v4/open-compute/capabilities` |
| `/operator/api/v1/account` | `/client/v4/accounts`、`/user/tokens/verify`、`/memberships` |
| `/operator/api/v1/system/status` | `/client/v4/open-compute/system/status` |
| Workers CRUD | `/client/v4/accounts/{id}/workers/scripts...` |
| deployment upload/finalize | 删除；改为官方 Worker multipart 和 Assets upload session |
| promotion/rollback | 删除；改为创建官方 Deployment |
| platform path route | `open-compute/workers/{script}/endpoints`；不冒充 Zone route |
| KV CRUD/value | `/client/v4/accounts/{id}/storage/kv...` |
| KV backup/restore | account-scoped `open-compute/kv...` extension |
| D1 CRUD/query | `/client/v4/accounts/{id}/d1...` |
| D1 migrations | Wrangler migration ledger + standard D1 query/import |
| D1 backup/restore | 标准 export/import/time-travel；本地额外快照进入 extension |
| R2 bucket/object | `/client/v4/accounts/{id}/r2...` |
| DO namespace 手工 CRUD | 删除；由 Worker exports/migrations 管理 |
| DO object registry | read-only `open-compute/durable-objects...` extension |
| Queues | `/client/v4/accounts/{id}/queues...` |
| Workflows | `/client/v4/accounts/{id}/workflows...` |
| Scheduler pause/resume/repair | installation-scoped `open-compute/scheduler...` |
| Cache GC / Images capacity | installation-scoped extension |
| `/operator/metrics` | admin listener 的 `/metrics`，不是 v4 JSON API |

Day 1 replacement 必须一次完成：同时落地所有 producer、consumer、Dashboard、tests、fixtures、active docs 和
package imports，并在同一变更删除旧 router、旧 SDK、`open-compute.json` parser 和私有协议。`docs/implemented/`
中已经归档的旧 Operator API 文档保留为历史证据，并明确由本文取代；它不进入生产 artifact，也不能被 active
文档链接成现行合同。

## 14. 实施顺序

**M0：冻结合同输入。**

- 把 `wrangler@4.127.1` 变成直接、精确 pin，而不是偶然的 transitive dependency；
- 保存 config schema SHA-256、Wrangler package integrity、官方 OpenAPI snapshot revision/hash；
- 用一个最小支持项目记录 `whoami`、deploy、Versions、Deployments、Secrets、Assets、`tail`、Observability
  Telemetry 和资源命令的 HTTP/WebSocket trace；
- 扩展 capability manifest 与 conformance catalog，给每个 route、field、binding 和 command 一个状态。

**M1：v4 protocol core。**

- 实现 common envelope、error、pagination、request ID、content type 和 auth middleware；
- 实现最小 user/account/membership/token endpoints；
- 建立 Cloudflare ID 与内部 authority 的 Day 1 数据模型；
- 注册 `/client/v4/open-compute/capabilities`，先让 capability 可查询。

**M2：Workers、Version 与 Deployment。**

- 直接重构 Script/Version/Deployment schema；
- 实现 streaming multipart parser、module/binding/date/flag validation 和 artifact admission；
- 实现 Versions/Deployments/Secrets/Schedules，以及带 generation 的 logs-only observability script-settings；
- 实现 rollback 作为新 Deployment；
- 删除 promotion/rollback 和 custom deployment upload public model。

**M3：Static Assets。**

- 实现 manifest session、missing object buckets、upload/completion token；
- 将完成的 assets manifest 绑定到 immutable Version；
- 覆盖 assets-only、Worker-first、dedupe、过期、restart 和 crash recovery。

**M4：资源 API。**

- 按 KV、D1、R2、Queues、Workflows 顺序增加 official adapters；
- adapter 只负责 wire validation/translation，domain service 保持唯一 authority；
- 把备份、scheduler、capacity 等非官方能力迁入 vendor namespace；
- DO lifecycle 改由 exports/migrations 驱动，移除手工 namespace CRUD。

**M5：Wrangler config 与消费者切换。**

- 删除 `open-compute.json` parser 和文档；
- framework adapter 产出标准 `wrangler.jsonc`/`.wrangler/deploy/config.json`；
- Dashboard 和 Lynx deployment broker 切换到精确 pin 的官方 SDK 与无独立 transport 的 extension binding；
- Dashboard Logs 和固定 Wrangler tail 分别切换到 Telemetry API 与 Script Tails API，不增加 vendor logs transport；
- 删除 Operator SDK、workspace dependency、toolchain direct deploy transport 和 Dashboard 的旧 transport；
- 重新生成 lockfile 与已跟踪 bundle，确保发布物不再内嵌旧 client、URL、DTO 或 config 默认值。

**M6：删除旧 surface 与最终验收。**

- 删除 `/operator/api/v1`、`/v1/accounts`、`/v1/operator`、`/v1/scheduler` 等旧注册及其 router、middleware、
  DTO、path helper、metrics label 和 mock；
- 让 `/operator/api/**` 在三类 listener 上命中中性 404，且既不进入 Dashboard SPA，也不进入 tenant Worker；
- 用 dependency/source/route inventory Gate 证明旧 package、symbol 和路径不存在，新路径没有 duplicate handler；
- 更新兼容矩阵、active docs、runbooks、examples、README 和所有测试 fixture；
- 完成第 15 节 Gate 后再把本文归档到 `docs/implemented/`。

M0–M6 是同一个 Day 1 implementation 的工作分解，不表示允许生产环境依次暴露中间合同。最终变更不提供 legacy
mode，也不接受“先留旧 API，后续再删”作为完成状态。

## 15. 验收策略

**Schema 与协议 Gate。**

- official subset 的 OpenAPI operation、method、path、required/nullable、enum 和 response 与固定 snapshot 一致；
- vendor schema 只出现在 `open-compute` namespace；
- 所有 JSON endpoint 都经过共同 envelope/error contract；
- raw/multipart endpoint 不被错误包裹；
- 每个分页 endpoint 验证自己的 cursor/page 行为；
- unsupported field/binding/route 有稳定错误和 JSON pointer。

**真实 Wrangler Gate。**

使用仓库 pin 的真实 Wrangler 子进程、真实 `ocd`、SQLite、S3 fixture 和 stock workerd。至少覆盖：

```text
wrangler whoami
wrangler deploy
wrangler versions upload/list/view/deploy
wrangler deployments list/status
wrangler rollback
wrangler secret put/list/delete/bulk
wrangler kv namespace create/list/delete
wrangler kv key put/get/list/delete
wrangler d1 create/list/info/execute/migrations apply/delete
wrangler r2 bucket create/list/delete
wrangler r2 object put/get/delete
wrangler queues create/list/delete
wrangler workflows ...（本文声明的子命令）
wrangler tail <script> --format=json|pretty（含本文声明的 filter flags）
```

测试不能 mock Wrangler 的 HTTP transport。每条命令记录 method/path、query、关键 headers、request content type、
response schema 和退出码；secret、token、signed upload token 和对象内容必须清洗。

**官方 SDK Gate。**

- 使用精确 pin 的 `cloudflare` package 和真实 `ocd`，覆盖 account discovery、Workers、KV、D1、R2、Queues、
  Workflows、Observability Telemetry 的 logs 子集、分页、raw value/object、multipart upload 与错误类型；
- 同一组无破坏 fixture 分别指向 Cloudflare 和 open-compute，只替换 `baseURL`、token 与资源 ID；
- extension binding 必须接收同一个官方 client，并通过它公开的 HTTP verb methods 发出请求；测试禁止替换成第二个
  fetch mock；
- Dashboard production build 只引入声明子集所需的 tree-shakable resources，并以真实浏览器 network test 验证
  standard 与 extension 请求都落到 `/client/v4`；
- SDK upgrade 先比较生成 API surface 与 wire trace；method/type rename 不能在未验收时通过宽松 semver 进入 lockfile。

**Cloudflare differential。**

对不会破坏账号现有资源的临时唯一名称，在真实 Cloudflare 和 open-compute 执行同源 fixture：

- 对 request 只归一化 origin、token、account/resource IDs 和不可避免的随机 boundary；
- 对 response 只归一化官方允许的 ID、timestamp、ETag、cursor、URL 和已登记 deviation；
- 字段缺失、null/empty 差异、错误 code、分页边界和 multipart metadata 都算合同差异；
- fixture 精确清理自己创建的资源，不读取或修改无关资源；
- 没有 credential 或产品权限时记录未验收，不能用本地测试替代远端证据。

**Runtime、恢复与安全 Gate。**

- 每个 Version 的 compatibility date/flags 在首次运行、restart 和 rollback 后一致；
- multipart/asset upload 在断流、重复 part、digest mismatch、token 过期和 crash 后 fail closed；
- Version ready 前 artifact 必须 verified；Deployment 不能指向 incomplete Version；
- binding 必须属于同 account 且是 live generation；
- secret 不出现在 artifact、SQLite plaintext、log、metrics、error、support bundle 或 GET response；
- API path 不能被 tenant route 覆盖，外部伪造 internal headers 被剥离；
- 旧 `/operator/api/v1`、历史 `/v1/...` 管理路径与 `open-compute.json` 不再被任何 production/test/example
  consumer 引用；`docs/implemented/` 的历史记录是唯一允许的文字证据例外。

**旧 surface 零保留 Gate。**

- 对旧 route manifest 中每个 prefix 和代表性深层路径分别请求 public、admin、merged listener，结果必须是
  HTTP 404，不能是 2xx、3xx、410、`text/html` 或 tenant Worker response；
- route inventory 中不存在旧 handler，Dashboard SPA matcher 不接受 `/operator/api/**`，public ingress 也不接管该
  reserved prefix；
- dependency graph、workspace package list、lockfile 与发布 artifact 中不存在
  `@open-compute/operator-sdk` 或 `createOperatorClient`；
- 标准资源调用只依赖精确 pin 的官方 Cloudflare SDK；extension package 不包含独立 transport，wrapper inventory 与
  extension OpenAPI 的 operation inventory 一致；
- production、test、fixture、example、script、Dashboard 和 toolchain 源码中不存在旧 URL、旧 DTO、旧 cursor 与旧
  deployment upload symbol；negative route inventory 可以集中保存旧 prefix 字面量；
- Dashboard browser/network test 和 Lynx broker integration test 只观察到 `/client/v4` 请求；
- 自动检查需要区分 active surface 与 `docs/implemented/` 历史证据，不能通过删除历史验收记录来制造零命中。

文档变更本身只运行 `git diff --check` 和链接/命令核对。真正实施属于 protocol、persistence、security 和 runtime
变更，必须按仓库 `AGENTS.md` 完成相关 focused tests、coverage 和最终 workspace Gate。

## 16. 已接受的 Day 1 deviation

| deviation | 决策 |
| --- | --- |
| 单机而非 Cloudflare 全球 fleet | API shape 保持，capacity/HA/latency 不伪装成 hosted semantics |
| 一个 installation、一个 account | account scope 仍保留；不实现 billing/org/plan |
| 无 `*.workers.dev` | 要求 `workers_dev:false`；真实入口通过 vendor endpoint 查询 |
| 无 Zone/DNS/TLS 管理 | `route`/`routes` 不支持；未来实现官方 Zone/Workers Routes 子集后再开放 |
| 无 remote dev/preview URL | `wrangler dev` 仅支持上游本地模式；真实验证使用 dev environment deployment |
| Deployment 只允许 100% 单 Version | 保留官方 request/response shape，拒绝多版本 rollout |
| 只支持部分 bindings/products | capability manifest 明确列出；upload fail closed |
| 不实现 Cloudflare commercial plan / billing quotas | Standard runtime 合同与部署方 capacity 分树报告；不写人数或单机默认值，不伪造 Free/Paid plan |
| Wrangler 内部 API base URL 行为非稳定承诺 | 精确 pin、trace Gate、逐版本升级 |

任何新 deviation 必须包含官方来源、可观察差异、影响范围、错误行为和回归 case。不能用“私有部署”作为静默
接受字段、放宽 account boundary 或暴露内部管理能力的理由。

## 17. Definition of Done

本文只有同时满足以下条件才可移入 `docs/implemented/`：

- 固定 Wrangler 的支持命令全部通过真实 subprocess Gate；
- official subset 和 vendor extension 都有 OpenAPI 与 route inventory；标准资源通过精确 pin 的官方 SDK，vendor
  资源通过 generated types 与无独立 transport 的 extension binding 调用；
- Dashboard、自动化与 Lynx deployment broker 不再调用 Operator API；
- repo 的 active surface 中没有 `open-compute.json` parser、Operator SDK、旧 DTO、旧 transport、旧管理 handler 或
  旧管理 path；集中式 negative route inventory 和 `docs/implemented/` 历史证据是仅有例外；
- 旧 URL 在 public、admin、merged listener 上均为中性 HTTP 404，未被 Dashboard SPA 或 tenant ingress 接管；
- multipart Worker upload 和 Static Assets 三段协议通过 success、failure、restart、crash 和 security tests；
- Worker compatibility date/flags 已成为 immutable Version authority；
- Standard structural/runtime limits 达到
  [`Workers Standard limits 专项设计`](day1-workers-standard-limits.md) 的 Definition of Done；
- `worker_loaders` 达到
  [`Dynamic Workers / Worker Loader 专项设计`](day1-dynamic-workers-worker-loader.md) 的 Definition of Done；
- KV、D1、R2、Queues、Workflows 和 Cron 的声明子集通过 API/runtime Gate；
- `wrangler tail`、Workers Logs、Telemetry query 与 Dashboard Live Tail 达到
  [`Workers Logs 专项设计`](day1-workers-logs-realtime-tail.md) 的 Definition of Done；
- Cloudflare differential 已完成，或剩余 credential 限制被拆成独立 active acceptance 文档；
- `docs/references/cloudflare-compatibility.md` 和 machine-readable capability authority 已同步；
- 最终 coverage、static checks 和单轮 workspace Gate 符合仓库 policy。

## 18. 官方基线

- [Cloudflare API Reference](https://developers.cloudflare.com/api/)
- [Cloudflare API schemas](https://github.com/cloudflare/api-schemas)
- [Cloudflare TypeScript SDK](https://github.com/cloudflare/cloudflare-typescript)
- [Cloudflare SDK support policy](https://developers.cloudflare.com/fundamentals/reference/sdk-ecosystem-support-policy/)
- [Workers Scripts API](https://developers.cloudflare.com/api/resources/workers/subresources/scripts/)
- [Workers API](https://developers.cloudflare.com/api/resources/workers/)
- [Workers Script Tail API](https://developers.cloudflare.com/api/resources/workers/subresources/scripts/subresources/tail/)
- [Workers Observability Telemetry API](https://developers.cloudflare.com/api/resources/workers/subresources/observability/subresources/telemetry/)
- [Workers Logs](https://developers.cloudflare.com/workers/observability/logs/workers-logs/)
- [Real-time logs](https://developers.cloudflare.com/workers/observability/logs/real-time-logs/)
- [Workers limits](https://developers.cloudflare.com/workers/platform/limits/)
- [Dynamic Workers](https://developers.cloudflare.com/dynamic-workers/)
- [Worker Loader API](https://developers.cloudflare.com/dynamic-workers/api-reference/)
- [Dynamic Workers limits](https://developers.cloudflare.com/dynamic-workers/platform/limits/)
- [Wrangler configuration](https://developers.cloudflare.com/workers/wrangler/configuration/)
- [Multipart upload metadata](https://developers.cloudflare.com/workers/configuration/multipart-upload-metadata/)
- [Static Assets direct uploads](https://developers.cloudflare.com/workers/static-assets/direct-upload/)
- [KV Namespaces API](https://developers.cloudflare.com/api/resources/kv/subresources/namespaces/)
- [D1 API](https://developers.cloudflare.com/api/resources/d1/)
- [Cloudflare workers-sdk](https://github.com/cloudflare/workers-sdk)

这些链接用于发现当前官方合同；实施和验收必须另外固定具体 schema/repository revision 与 SHA-256，不能把会变化
的网页当作可复现 build input。
