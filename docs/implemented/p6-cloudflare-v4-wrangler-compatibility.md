# P6：Cloudflare v4 API 与 Wrangler 子集兼容设计

状态：P6 本地核心实现完成并归档；托管端 differential 见
[`docs/acceptance/`](../acceptance/p6-cloudflare-v4-differential-acceptance.md)

日期：2026-09-03

本文定义并记录 open-compute Day 1 唯一的在线管理协议和 Worker 项目配置契约。P6 本地核心已经按该合同实现：

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

本文取代 [`Operator API 与可选 Dashboard Day1 方案`](operator-api-dashboard.md) 中关于 API
根路径、Operator SDK 和项目配置的目标设计。旧文档及其结果只保留为历史证据；当前实现已经删除旧 Operator
API、Operator SDK 和 `open-compute.json` 路径，并以本文合同为准。
[`Cloudflare Workers 兼容矩阵`](../references/cloudflare-compatibility.md) 已同步记录 runtime 事实；tenant 选择的
compatibility date 与 flags 已成为 immutable Worker Version authority，不再由全局值替代。

P5 已经实现 Vectorize、AI Search namespace/instance/items/jobs/search/chat 和 `env.AI.toMarkdown()` 的本地
domain/runtime authority，见[完成记录](p5-vectorize-ai-search-results.md)。因此 P6 不再把
`vectorize`、`ai_search_namespaces`、`ai_search` 或 `ai` 一概列为 unsupported；P6 的工作是把这些现有能力接到
固定 Wrangler multipart 和官方 `/client/v4` 路径，并保持 `OC-VECTORIZE-001`、`OC-AI-SEARCH-001` 与
`OC-AI-MARKDOWN-001` 已声明的单机 deviation。P6 已把这些能力接到固定 Wrangler multipart 和官方
`/client/v4` 路径，并移除旧 `/operator/api/v1` 与按内部 resource ID 的项目配置。

Workers Logs、固定 Wrangler 的 realtime tail 和 Observability Telemetry 子集由
[`P7 Workers Logs 与 realtime tail 兼容设计`](p7-workers-logs-realtime-tail.md) 独立实施。P6 建立它复用的
v4 protocol core；三个 Script Tails operation 在 P6 capability manifest 中保持 `planned`，相关 mutation 在
P7 完成前 fail closed。P6 完成不冒充 P7 已完成；P7 也不另建 vendor logs API。

Local / S3 对象后端互斥配置、统一 object operation facade 与开发期 rclone 移除由
[`P8 Local / S3 对象后端设计`](p8-local-s3-object-backend.md) 细化；P6 仍记录其实施时必须保持的 R2、
snapshot、backup 与 immutable artifact 公开合同。

Workers Standard 的 structural/runtime limits 由
[`workerd P2 Workers Standard limits 设计`](../workerd/p2-workers-standard-limits.md) 细化；公开的 `worker_loaders` binding、
`WorkerLoader.load/get` 和 nested stock-workerd Gate 由
[`workerd P1 Dynamic Workers / Worker Loader 设计`](../workerd/p1-dynamic-workers-worker-loader.md) 细化。后者只覆盖
Dynamic Workers，不包含 Workers for Platforms 或 dispatch namespaces。Cloudflare Artifacts 的 v4/Worker/Git
contract 由 [`P11 Cloudflare Artifacts 兼容设计`](../p11-cloudflare-artifacts.md) 细化；Browser Run 的 binding、
Quick Actions、DevTools/CDP 与 operator-owned 外部 Browser Provider 由
[`P12 Cloudflare Browser Run 兼容设计`](../p12-browser-run.md) 细化。P6 只记录固定 schema/multipart inventory 已出现的
后续字段；固定输入中尚未登记的 binding 不因此成为 P6 支持项。在各专项通过前它们保持 fail closed，P6 与
P7—P12 各自具有独立的 Definition of Done。

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

每个端点只允许四种状态：

- `supported`：请求、响应和可观察语义均在声明范围内兼容；
- `supported_with_deviation`：wire 合同兼容，但单机或内网拓扑导致已登记的语义差异；
- `planned`：只冻结后续阶段的 route/schema inventory，当前不注册或明确 fail closed；
- `unsupported`：不注册端点，或返回明确的 CF-style 错误；绝不静默忽略。

机器可读能力清单以 `share/cloudflare-capabilities.json` 为唯一真值，已经包含 `managementApi` 和 `wrangler`
两部分，不建立第二份互相漂移的 capability 文件。当前 P6 management route inventory 共 155 项：149 项
`supported`、2 项 `supported_with_deviation`、3 项 P7 Tails `planned`、1 项 R2 object collection GET
`unsupported`。人类可读矩阵由该 manifest 和 conformance catalog 生成或校验。

**SMB 单机部署的复杂度预算。**

open-compute 面向 SMB 的单机自托管部署，不追求 Cloudflare 全球 fleet 的所有长尾行为。实现优先级依次是协议
主路径、数据完整性、安全边界、重启恢复和运维可诊断性；只有在真实 SMB 场景中有足够收益时，才增加高复杂度的
低概率边界覆盖。被延期的长尾行为必须在 capability/deviation/acceptance 中显式记录并 fail closed，不能用复杂度
预算静默吞字段、放宽 account/secret 边界或伪造托管语义。

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

固定 `cloudflare@7.1.0` 的 `resources/workers/scripts/scripts` 与 `internal/uploads` 会在
`workers.scripts.update()` 把 typed body 转为 `FormData` 后仍保留生成代码写入的
`Content-Type: application/javascript`，并把 metadata object 编码为 `metadata[...]` bracket fields、数组编码为
`[]` fields、模块编码为带 filename 的 `files[]`。生产上传边界仅对这个精确 pin 的真实 wire shape 从首个有界、
合法 multipart delimiter 恢复 boundary；只按 P6 的 closed metadata schema 重建 `compatibility_flags`、annotations、
bindings 和已支持的嵌套字段，未知、重复、无法无损重建的 shape 直接失败，再进入同一个 multipart/Version
authority；不增加第二 transport、raw fetch 或旧协议。`workers_http::v4::sdk_multipart` 的 wire regression 与
`p6-cloudflare-sdk` 的真实 `ocd`/stock workerd Gate 共同锁定这一例外；升级 SDK 时必须重新取 trace，若上游已修复
header/field shape 就直接删除该归一化路径。

同一组固定客户端对 D1 binding 使用不同的官方 upload 字段：Wrangler 4.127.1 把配置里的 `database_id`
转换为 multipart metadata 的 `id`，而 `cloudflare@7.1.0` typed `workers.scripts.update()` 直接发送
`database_id`。SDK bracket adapter 只在 binding `type` 为 `d1` 时把后者归一为内部唯一的 `id`；二者并存、
字段顺序无法唯一分组或任何其它 binding 携带 `database_id` 都失败。该规则是两个固定官方客户端的当前 wire
合同，不是旧 open-compute alias，也不进入标准 JSON metadata path。

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

项目处于 Day 1，P6 直接修改 authoritative schema 和 ID 生成规则，没有为旧开发数据库增加映射表、双读或
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

原 immutable deployment 数据结构已直接成为 Version authority，原 promotion/rollback active pointer 已成为
Deployment authority；没有继续保留一套 `promotion`/`rollback` public endpoint。

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
| Tails（P7 planned） | `GET/POST /accounts/{account_id}/workers/scripts/{script_name}/tails`，`DELETE .../tails/{tail_id}` | P6 只冻结 route inventory 并 fail closed；固定 Wrangler 的 `trace-v1` realtime tail、filter、session、overload 和 WebSocket 合同由 P7 实现 |
| Secrets | `GET/PUT /accounts/{account_id}/workers/scripts/{script_name}/secrets`，`GET/DELETE .../secrets/{secret_name}`，`PUT .../secrets-bulk` | 支持固定 Wrangler 的 list/get/put/delete/bulk 请求 |
| Cron | `GET/PUT /accounts/{account_id}/workers/scripts/{script_name}/schedules` | 以完整 schedule collection 更新映射现有 Cron authority |
| Account subdomain | `GET /accounts/{account_id}/workers/subdomain` | 固定 Wrangler Workflow deploy 的只读 prerequisite；返回 account-scoped 稳定、不可路由 label，不创建 DNS/listener/route；`PUT/DELETE` 不支持 |
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
- 固定 Wrangler 4.127.1 在 `workers_dev:false` 且同次 deploy 声明 Workflow 时，仍会无条件读取 account
  subdomain 后丢弃其值；`GET /accounts/{account_id}/workers/subdomain` 因此返回以 `_` 开头的稳定 account-scoped
  prerequisite label。该 label 不是合法 DNS hostname，不对应 DNS、listener、route 或可访问 URL；注册/修改/删除
  account subdomain 仍不支持；
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
| `vectorize` | `vectorize` + `index_name` |
| `ai_search_namespaces` | `ai_search_namespace` + `namespace` |
| `ai_search` | `ai_search` + `instance_name` |
| `ai` | `ai`；只注入已声明的 Markdown Conversion 子集 |
| `durable_objects.bindings` | `durable_object_namespace` + class/script |
| `queues.producers` | `queue` + `queue_name`；固定 Wrangler 4.127.1 的 `delivery_delay` deprecated/no-effect 字段只接受后忽略 |
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

固定 Wrangler 4.127.1 的 schema 仍识别 producer `delivery_delay`，但同一 pin 的 CLI 明确警告该字段已弃用且
不生效，并要求通过 `wrangler queues update` 配置 Queue-level settings。P6 因此接受该字段以兼容固定客户端，
但不把它写入 binding descriptor、Version digest 或 Queue authority；这不是可配置延迟的成功承诺。未来升级
Wrangler 时重新取 trace，并以新 pin 的实际命令合同为准。

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

P6 本地实现已经完成本节声明的 D1 route surface；两个 time-travel route 标记为
`supported_with_deviation`。固定 `cloudflare@7.1.0` 的
`resources/d1/database/database.mjs` 与 `time-travel.mjs` 所列 export/import/time-travel endpoint 已接入
`/client/v4`；SQL
export 支持 `dump_options`，import 使用只存 capability fingerprint 的持久 init/upload/ingest/poll session，
导入提交在 durable snapshot/history 之前不会返回成功，重启会先 reconcile fenced ingest。time-travel 的
bookmark/timestamp 只解析 retained completed checkpoint，restore 保留 database identity 并产生单调递增的新
session version。

Cloudflare Time Travel 是 always-on、分钟级、7/30 天 PITR；单机 SMB 实现不为每次普通 Worker D1 mutation
同步复制整库，也不把普通 D1 Session bookmark 自动升级为可恢复快照。只有显式 export/import/time-travel
管理操作会建立当前 checkpoint；每个数据库最多保留 8 个 checkpoint，仍被 transfer/restore intent 引用的点
不能回收。terminal transfer 的 URL capability 过期后会先删除其 authority 和 exact transfer file，释放所 pin 的
history；每库同时最多保留 8 个未过期 terminal transfer file。若尚未过期的 durable evidence 已占满 checkpoint
上限，或 transfer file 达到上限，新的显式操作在复制或 mutation 前以稳定容量错误拒绝；普通
Worker 读写继续只依赖 live SQLite durability，不因管理面 checkpoint 创建、验证或回收失败返回 result unknown。
timestamp 仅解析仍保留的显式点，restore bookmark 也必须精确对应其中一个点；authority-row 删除后的极低概率
checkpoint 或 expired transfer file unlink 失败可能留下不再可达的 orphan file，当前不为此引入启动扫描/日志型
GC 状态机。
`openapi/p6-capability-source.json` 是 route 状态 authority；固定 Wrangler/SDK 的最终本地
Gate 需要在 P6 source freeze 后按第 15 节单轮执行，托管端 differential 仍单独待验收。

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

**Vectorize。**

P6 把现有 per-index SQLite/exact engine 接到当前官方 V2 路径；外部主键是 `index_name`，不能把内部
`ResourceId` 暴露为 URL 主键：

```text
GET/POST  /accounts/{account_id}/vectorize/v2/indexes
GET/DELETE
          /accounts/{account_id}/vectorize/v2/indexes/{index_name}
POST      /accounts/{account_id}/vectorize/v2/indexes/{index_name}/insert
POST      /accounts/{account_id}/vectorize/v2/indexes/{index_name}/upsert
POST      /accounts/{account_id}/vectorize/v2/indexes/{index_name}/query
POST      /accounts/{account_id}/vectorize/v2/indexes/{index_name}/get_by_ids
POST      /accounts/{account_id}/vectorize/v2/indexes/{index_name}/delete_by_ids
GET       /accounts/{account_id}/vectorize/v2/indexes/{index_name}/info
GET       /accounts/{account_id}/vectorize/v2/indexes/{index_name}/list
POST      /accounts/{account_id}/vectorize/v2/indexes/{index_name}/metadata_index/create
GET       /accounts/{account_id}/vectorize/v2/indexes/{index_name}/metadata_index/list
POST      /accounts/{account_id}/vectorize/v2/indexes/{index_name}/metadata_index/delete
```

固定 Wrangler 的 `vectorize create/list/get/delete/insert/upsert/query/get-vectors/delete-vectors/info` 与 metadata
index 子命令都必须复用这些端点和现有唯一 domain authority。V1/`--deprecated-v1` 路径、旧
`VectorizeIndex` binding、托管 ANN/placement/billing 不支持。upload metadata 中的 `index_name` 在创建 Version
前解析为同 account 的 live resource，并把内部 ID、spec generation 与权限冻结进 immutable descriptor；runtime
仍只看到经过验证的 capability。

**AI Search 与 `ai`。**

P6 只暴露 P5 已实现的 built-in storage 子集，不借 v4 adapter 扩张到 website/R2 connector、crawler、public
endpoint、MCP、credential mutation、AI Gateway 或完整 Workers AI inference。固定 Wrangler 4.127.1 的
`ai-search create` 会先列出 stored credentials，因此只额外开放一个只读 preflight：

```text
GET       /accounts/{account_id}/ai-search/tokens

GET/POST  /accounts/{account_id}/ai-search/namespaces
GET/PUT/DELETE
          /accounts/{account_id}/ai-search/namespaces/{namespace_name}
POST      /accounts/{account_id}/ai-search/namespaces/{namespace_name}/search
POST      /accounts/{account_id}/ai-search/namespaces/{namespace_name}/chat/completions

GET/POST  /accounts/{account_id}/ai-search/namespaces/{namespace_name}/instances
GET/PUT/DELETE
          /accounts/{account_id}/ai-search/namespaces/{namespace_name}/instances/{instance_id}
GET       /accounts/{account_id}/ai-search/namespaces/{namespace_name}/instances/{instance_id}/stats
POST      /accounts/{account_id}/ai-search/namespaces/{namespace_name}/instances/{instance_id}/search
POST      /accounts/{account_id}/ai-search/namespaces/{namespace_name}/instances/{instance_id}/chat/completions

GET/POST  /accounts/{account_id}/ai-search/namespaces/{namespace_name}/instances/{instance_id}/jobs
GET/PATCH /accounts/{account_id}/ai-search/namespaces/{namespace_name}/instances/{instance_id}/jobs/{job_id}
GET       /accounts/{account_id}/ai-search/namespaces/{namespace_name}/instances/{instance_id}/jobs/{job_id}/logs

GET/POST/PUT
          /accounts/{account_id}/ai-search/namespaces/{namespace_name}/instances/{instance_id}/items
GET/PATCH/DELETE
          /accounts/{account_id}/ai-search/namespaces/{namespace_name}/instances/{instance_id}/items/{item_id}
GET       /accounts/{account_id}/ai-search/namespaces/{namespace_name}/instances/{instance_id}/items/{item_id}/download
GET       /accounts/{account_id}/ai-search/namespaces/{namespace_name}/instances/{instance_id}/items/{item_id}/logs
GET       /accounts/{account_id}/ai-search/namespaces/{namespace_name}/instances/{instance_id}/items/{item_id}/chunks
```

method、multipart item upload、pagination、cancel/sync body 和 response shape 以固定 OpenAPI/SDK/Wrangler trace
为准，不能从现有 private binding frame 直接推导。`ai_search_namespaces` 使用 namespace name；固定 Wrangler
部署时若 namespace 不存在，会先 GET 再 POST 官方 namespace endpoint。`ai_search` 只绑定 `default` namespace
中已经存在的 `instance_name`，不得隐式创建 instance。两种 binding 都在 Version 创建前解析并冻结当前 live
resource generation；namespace runtime CRUD 仍受该 namespace capability 和现有 read/write permissions 约束。

`GET /ai-search/tokens` 遵循 SDK 7.1.0 `TokenListResponse` 和官方 v4 pagination，但本机返回的唯一记录是由
installation authority 管理、按 account 稳定派生且不含 secret 的 credential metadata。它只满足固定 Wrangler
创建 built-in instance 的只读 preflight；客户端提交的 Cloudflare `cf_api_key` 不被接受或保存，token
POST/PUT/DELETE 与按 ID 管理端点继续不支持：collection mutation 返回 method-not-allowed，未声明的按 ID 路径
保持中性 404。此单机语义登记为 `OC-AI-SEARCH-TOKEN-001`；官方来源是固定 OpenAPI 的
`ai-search-list-tokens` 与 SDK 7.1.0 `TokenListResponse`。`official_ai_search_routes_cover_the_frozen_30_operation_surface`
回归验证稳定 metadata、pagination 和不泄露 `cf_api_key`，`p6-wrangler-resources` Gate 验证固定 subprocess
create 成功且输出不含本机认证 token。

`ai: {binding}` 只生成 `{name,type:"ai"}` multipart binding，并注入 P5 已验证的
`aiGatewayLogId`、`toMarkdown()` 与 `supported()` 子集。其它 Workers AI 方法继续按 capability matrix 明确拒绝；
支持 `ai` 配置字段不等于宣称完整 Workers AI。

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

P6 本地实现已经完成上述十个 Queue endpoint，均接入 `/client/v4`，使用公开稳定 ID、官方
envelope 与固定 Wrangler 的 query/JSON shape；consumer 更新先持久化不可变 generation，再由唯一
Scheduler repair authority 执行旧 generation drain、fence、target switch 和重启 reconcile。Queue pause 是
持久 desired state，重启后会与 consumer projection 收敛。`openapi/p6-capability-source.json` 是 route 状态
authority；pull consumer 仍按 Day 1 范围明确拒绝。固定 Wrangler 4.127.1 的 producer `delivery_delay` 按第
6 节接受后忽略，Queue 级延迟只通过受支持的 Queue mutation 生效。

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

P6 已实现固定 Wrangler 4.127.1 的实际 `Worker upload -> account subdomain GET -> Workflow PUT` 顺序。Worker
upload 先在 account/name scope 建立持久 definition reservation，以 operation owner 与单调 fence 冻结
`class_name`；Workflow PUT 只能消费匹配的 owner/fence 并发布 current Workflow version，runtime 在 ready/current
version 形成前 fail closed。相同 class 的重试可恢复，不同 class 冲突明确拒绝；upload validation、artifact 或
Version 创建失败时只释放该 operation 尚未消费的 reservation，新建且无消费者的空 definition 会 tombstone，
已有 ready definition 则保留原 current version。Workflow delete 先 fence 新 reservation，再完成实例清理和
tombstone，避免失败上传留下永久占名。跨 Script 的 `script_name` binding 不在 P6 单机子集，出现时明确拒绝。

**Durable Objects、Cron、Service Binding、Cache 与 Images。**

- Durable Object namespace 由 Worker 的 `exports` 或 `migrations` 生命周期声明创建、重命名和 tombstone；不保留
  手工 namespace CRUD API。object registry 的只读诊断属于 vendor extension。
- Cron 使用 Workers Script Schedules 官方 API；scheduler pause/repair 是 installation 运维能力，属于 vendor
  extension。
- Service Binding 完全由 Version multipart metadata 声明，不需要单独 provisioning API。P6 支持 `service`、
  `entrypoint` 与可选 `props`；`props` 必须是 canonical immutable JSON object，编码上限 64 KiB、深度上限 32，
  随 Version descriptor/digest 持久化并通过 `ctx.props` 传入目标 entrypoint。`constructor` 与 `__proto__` 只是普通
  JSON key，不触发原型语义；`remote` 不属于 server binding 子集。
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

### 10.2 P6 支持字段与显式后续阶段

下表未标阶段的行属于 P6 目标；标记 P7—P12 的行只冻结固定 Wrangler 语法和 fail-closed handoff，不能在
对应阶段完成前出现在成功 upload/settings response 或 capabilities 的 `supported` 集合中。

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
| Vectorize | `vectorize[].binding`, `index_name` | index 必须已存在；`remote` 只属于 Wrangler local dev；V1 不支持 |
| AI Search namespace | `ai_search_namespaces[].binding`, `namespace` | 固定 Wrangler 可在 deploy 前通过官方 API 创建缺失 namespace；`remote` 不进入 server binding |
| AI Search instance | `ai_search[].binding`, `instance_name` | 只解析 `default` namespace 中已存在的 instance；不隐式创建 |
| Workers AI | `ai.binding` | 只开放 P5 Markdown Conversion 子集；完整 inference、AutoRAG 与 AI Gateway 不支持 |
| Durable Objects | `durable_objects.bindings[].name`, `class_name`, 可选 `script_name` | 同 account；`environment` Day 1 不支持 |
| DO lifecycle | `migrations` 的 `tag`, `new_sqlite_classes`, `renamed_classes`, `deleted_classes` | `new_classes` 非 SQLite storage 不支持 |
| Declarative exports | `exports` 的 Worker entrypoint 和 SQLite DO `created`/`renamed`/`deleted` | container、cross-script transfer 不支持；与 `migrations` 的互斥沿用 Wrangler schema |
| Queues producer | `queues.producers[].binding`, `queue`, `delivery_delay` | queue 必须已存在或由标准 provisioning 创建；固定 Wrangler 4.127.1 将 `delivery_delay` 标为 deprecated/no-effect，P6 接受后忽略且不进入 immutable state |
| Queues consumer | `queue`, `max_batch_size`, `max_batch_timeout`, `max_retries`, `dead_letter_queue`, `max_concurrency`, `retry_delay` | `visibility_timeout_ms` 不支持 |
| Workflows | `binding`, `name`, `class_name`, `schedules` | owner/fence reservation 支持固定 Wrangler 的 upload-first 顺序；`script_name`、limits、retention、hosted concurrency 不支持 |
| Service Binding | `binding`, `service`, `entrypoint`, `props` | `props` 为 canonical immutable JSON object（64 KiB、深度 32）并投影到 `ctx.props`；`constructor`/`__proto__` 是普通 key；`remote` 不属于 server 子集 |
| Static Assets | `directory`, `binding`, `html_handling`, `not_found_handling`, `run_worker_first` | 使用官方 upload session 和 multipart token |
| Cron | `triggers.crons` | `triggers.events` 不支持 |
| Cache | `cache.enabled`, `cache.cross_version_cache` 和受支持的 Worker export cache override | 映射现有 Cache authority |
| Images | `images.binding` | `remote` 只是 local dev 字段，不改变 server binding |
| Version Metadata | `version_metadata.binding` | 不保留 `open-compute.json` 曾有的非标准 `tag` 字段 |
| Standard limits（workerd P1，原 P9） | `limits.cpu_ms`, `limits.subrequests` | 固定 Wrangler 4.127.1 schema 不含 `usage_model` property；`usage_model` 因 pinned-schema absence 保持 `unsupported`，`limits` 字段在 workerd P1 完成前同样 fail closed，见 [limits 专项](../workerd/p2-workers-standard-limits.md) |
| Worker Loader（workerd P2，原 P10） | `worker_loaders[].binding` | P6 识别字段但 fail closed；public Loader 仍未完成 native Gate；后续采用用户 fork 路线，见 [Worker Loader 专项](../workerd/p1-dynamic-workers-worker-loader.md) |
| Observability logs（P7） | `observability.enabled`, `head_sampling_rate`, `logs.enabled`, `logs.head_sampling_rate`, `logs.invocation_logs`, `logs.persist` | P6 只提供共用 v4 core；P7 完成前 settings mutation fail closed；`destinations` 只接受空数组 |
| Cloudflare Artifacts（P11） | `artifacts[].binding`, `artifacts[].namespace` | 固定 config schema 已在 P6 inventory 标为 `unsupported`；Artifacts multipart binding 与 P11 v4/Worker/Git 合同由 [Artifacts 专项](../p11-cloudflare-artifacts.md) 一起实现，P11 前 fail closed；`remote` 仅 local dev |
| Browser Run（P12） | `browser.binding` | P6 识别固定 config/multipart，但 P12 provider、binding、Quick Actions、DevTools/CDP 全部通过前 fail closed，见 [Browser Run 专项](../p12-browser-run.md)；`remote` 仅 local dev |
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
websearch / agent_memory
hyperdrive
analytics_engine_datasets
dispatch_namespaces
containers / cloudchamber
mtls_certificates / ratelimits
pipelines / stream / media
secrets_store_secrets / flagship
vpc_services / vpc_networks
send_email
unsafe
```

`artifacts` 与 `browser` 不在以上永久 unsupported 列表中：它们分别属于 P11/P12 的后续范围，但 P6 machine
inventory 中已出现的成员仍保持 `unsupported`，尚未登记的 multipart member 也不对外宣称存在。专项 DoD 完成前，
行为始终 fail closed，而不是 P6 提前标记 supported。

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

  "vectorize": [
    { "binding": "EMBEDDINGS", "index_name": "invoice-embeddings" }
  ],

  "ai_search_namespaces": [
    { "binding": "TEAM_SEARCH", "namespace": "team" }
  ],

  "ai_search": [
    { "binding": "INVOICE_SEARCH", "instance_name": "invoices" }
  ],

  "ai": {
    "binding": "AI"
  },

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

`@open-compute/operator-sdk` 已从 workspace、lockfile、Dashboard/toolchain imports 和发布面删除。Day 1 不生成一套覆盖标准资源的
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

vendor extension 不进入 Cloudflare SDK namespace，也不 fork 官方 package。P6 已实现
`@open-compute/cloudflare-extension`：它接收已经创建的官方 `Cloudflare` client，导出 extension methods，例如：

```ts
const extension = createOpenComputeExtension(client);

await extension.system.status();
await extension.backups.create({ account_id: accountId, resource_id: resourceId });
```

这个 package 没有自己的 auth、fetch、retry、pagination 或 error transport。request/response types 由 extension
OpenAPI 生成；少量 operation wrapper 可以手写，但 path、method、operation ID 和类型必须由 contract test 对照
OpenAPI。不要用手写 Zod/TypeScript interface 再复制一份 schema authority。

OpenAPI 的职责是合同、文档、类型生成和 conformance，不是再造 transport。P6 使用的目录为：

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

## 13. 管理面唯一入口

所有管理客户端、Dashboard 和工具链使用本合同的 `/client/v4` 与受控 vendor extension。
旧 Operator API／SDK 已移除，现有 domain service 与持久化 authority 直接复用；不存在双协议读写。

## 15. 验收依据

实际命令、固定客户端测试、源码身份和未验收项见[本地完成记录](p6-cloudflare-v4-wrangler-compatibility-results.md)。
测试清单以 [`test/gate_cases.py`](../../test/gate_cases.py) 和 conformance inventory 为准；
执行节奏见[测试手册](../references/testing.md)。
管理资源、SDK、Assets 的 hosted differential 见[独立验收计划](../acceptance/p6-cloudflare-v4-differential-acceptance.md)。

## 16. 已接受的 Day 1 deviation

| deviation | 决策 |
| --- | --- |
| 单机而非 Cloudflare 全球 fleet | API shape 保持，capacity/HA/latency 不伪装成 hosted semantics |
| 一个 installation、一个 account | account scope 仍保留；不实现 billing/org/plan |
| 无 `*.workers.dev` | 要求 `workers_dev:false`；固定 Wrangler Workflow deploy 的 account subdomain GET 只返回不可路由 prerequisite label，且 CLI 丢弃该值；不创建或声明 DNS、listener、route；真实入口通过 vendor endpoint 查询 |
| 无 Zone/DNS/TLS 管理 | `route`/`routes` 不支持；未来实现官方 Zone/Workers Routes 子集后再开放 |
| 无 remote dev/preview URL | `wrangler dev` 仅支持上游本地模式；真实验证使用 dev environment deployment |
| Deployment 只允许 100% 单 Version | 保留官方 request/response shape，拒绝多版本 rollout |
| 只支持部分 bindings/products | capability manifest 明确列出；upload fail closed |
| 不实现 Cloudflare commercial plan / billing quotas | Standard runtime 合同与部署方 capacity 分树报告；不写人数或单机默认值，不伪造 Free/Paid plan |
| `OC-AI-SEARCH-TOKEN-001` | 官方 list tokens 返回 account stored credential metadata；单机安装只返回一个 account-scoped、稳定、无 secret 的 installation-managed metadata 供固定 Wrangler create preflight 使用，所有 token mutation 与按 ID 管理继续 unsupported |
| SMB 单机复杂度预算 | 安全、完整性、重启恢复和主路径优先；低收益、高复杂度、低概率长尾可显式延期并 fail closed，不追求全球 fleet 的表面完美覆盖 |
| 测试端口选择 TOCTOU | 接受真实子进程 Gate 在释放 ephemeral probe socket 到 `ocd` bind 之间的低概率碰撞；碰撞明确失败并留证，不为测试引入 fd inheritance/socket activation，也不改变 production listener |
| Wrangler 内部 API base URL 行为非稳定承诺 | 精确 pin、trace Gate、逐版本升级 |

任何新 deviation 必须包含官方来源、可观察差异、影响范围、错误行为和回归 case。不能用“私有部署”作为静默
接受字段、放宽 account boundary 或暴露内部管理能力的理由。

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
- [Cloudflare Artifacts REST API](https://developers.cloudflare.com/artifacts/api/rest-api/)
- [Cloudflare Artifacts Workers binding](https://developers.cloudflare.com/artifacts/api/workers-binding/)
- [Cloudflare Artifacts Git protocol](https://developers.cloudflare.com/artifacts/api/git-protocol/)
- [Cloudflare Browser Run API](https://developers.cloudflare.com/api/resources/browser_rendering/)
- [Browser Run CDP](https://developers.cloudflare.com/browser-run/cdp/)
- [Browser Run Wrangler commands](https://developers.cloudflare.com/browser-run/reference/wrangler-commands/)
- [Wrangler configuration](https://developers.cloudflare.com/workers/wrangler/configuration/)
- [Multipart upload metadata](https://developers.cloudflare.com/workers/configuration/multipart-upload-metadata/)
- [Static Assets direct uploads](https://developers.cloudflare.com/workers/static-assets/direct-upload/)
- [KV Namespaces API](https://developers.cloudflare.com/api/resources/kv/subresources/namespaces/)
- [D1 API](https://developers.cloudflare.com/api/resources/d1/)
- [Vectorize API](https://developers.cloudflare.com/api/resources/vectorize/)
- [AI Search API](https://developers.cloudflare.com/api/resources/ai_search/)
- [AI Search Workers binding](https://developers.cloudflare.com/ai-search/api/search/workers-binding/)
- [Cloudflare workers-sdk](https://github.com/cloudflare/workers-sdk)

这些链接用于发现当前官方合同；实施和验收必须另外固定具体 schema/repository revision 与 SHA-256，不能把会变化
的网页当作可复现 build input。
