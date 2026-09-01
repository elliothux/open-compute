# Operator API 与可选 Dashboard Day1 方案

状态：方案完成，尚未实施

日期：2026-09-01

实施目标：把在线管理能力收敛为一个始终可用、强制管理员鉴权的 `/operator` surface，提供可被普通
JavaScript 程序直接调用的 Operator SDK，并提供一个默认关闭、以普通静态 Worker 项目构建和运行的
React Dashboard。

## 1. 决策

Day1 固定以下契约：

- `/operator/api/v1/**` 是唯一在线管理 API；Dashboard、`oc` 和其他自动化客户端都使用它，不直接访问
  SQLite、S3、workerd internal listener 或 binding backend；
- `packages/operator-sdk/` 提供唯一、端到端 typesafe 的 JavaScript client
  `@open-compute/operator-sdk`；Zod schema 是请求、响应和稳定错误的唯一 TypeScript 协议定义，Dashboard
  不手写第二套 URL、DTO、错误解析或鉴权 transport；
- Operator API 不受 Dashboard 开关影响。只要 `ocd` 的有效 admin listener 已启动，API router 就存在；
- `server.admin_auth` 始终必填，loopback listener 也不允许匿名调用 Operator API；
- Dashboard 使用固定入口 `/operator/`，由 `[dashboard].enabled` 控制，默认 `false`；
- Dashboard 位于 `packages/dashboard/`，技术栈固定为 React、TypeScript 7、Vite、Tailwind CSS、TanStack
  Router、TanStack Query 与 Cloudflare Kumo；产物是普通 assets-only Worker SPA，不引入另一套 Rust 静态
  文件服务器或 Node.js 生产进程；
- Dashboard 只通过浏览器发起同源 Operator API 请求。Dashboard Worker 不持有管理员 token，不获得控制面
  service binding，也不成为代用户调用 API 的 confused deputy；
- Durable Objects 页面只展示 namespace 和已经登记的 Object 实例元数据，不读取或修改 Object 内存、
  SQLite、KV、alarm 或 WebSocket attachment 状态。

现有管理路径会直接切换到新前缀，不保留 `/v1/accounts/**`、`/v1/operator/**` 或 `/v1/scheduler/**`
alias、redirect、双注册和旧客户端兼容分支。项目仍处于 Day1 开发期，只有一个权威管理协议。

**当前基础与需要补齐的边界。**

当前 [HTTP composition](../crates/service/src/http.rs) 已经区分 public、admin 和 merged listener；Workers、
KV、D1、R2、Durable Objects、Queues、Workflows、scheduler、Cache 与 Images 也已有 authenticated control
handler。问题在于路径分散在 `/v1/accounts/**`、`/v1/operator/**`、`/v1/scheduler/**`、`/health/status`
和 `/metrics`，且 loopback 模式允许不配置管理员凭据。

现有 [project parser](../packages/toolchain/src/project.ts) 和 [Static Assets](implemented/p3-1-static-assets.md)
已经支持 assets-only deployment 与 `single-page-application` fallback。Dashboard 不需要新的 Web 托管模型，
只需要把普通 Worker 产物作为 release-owned system deployment 挂到 admin listener 的保留路径。

Operator API 是 open-compute 自有的单机管理协议，不模拟 Cloudflare `/client/v4` response envelope，也不改变
[Cloudflare runtime compatibility](implemented/cloudflare-runtime-compatibility.md) 的 tenant API 范围。

## 2. Listener 与路由拓扑

```text
browser / TanStack Query --\
external JavaScript -------+--> @open-compute/operator-sdk --\
oc online commands --------/                                   \
curl ------------------------------------------------------------+-->
ocd effective admin listener
        |-- /operator/api/v1/** -- mandatory admin auth --> service/control authority
        |-- /operator/metrics  -- mandatory admin auth --> Prometheus output
        |-- /operator/**       -- dashboard.enabled? ---> dashboard Worker static assets
        `-- /health/live, /health/ready -------------> unauthenticated probes

public listener
        `-- tenant Worker ingress
```

`server.admin_bind` 继续决定是否使用独立 admin listener：

- 配置独立地址时，public listener 不暴露 `/operator/**`；
- 未配置时，Operator 与 public ingress 共用 `server.public_bind`，但 `/operator/**` 是平台保留路径，必须先于
  tenant route/fallback 匹配，租户不能注册或接管该前缀；
- `[dashboard].enabled = true` 不创建第二个 listener，也不改变 bind address；
- Dashboard 关闭时，`/operator/` 返回 `404`，`/operator/api/v1/**` 和 `/operator/metrics` 保持可用。

`/health/live` 和 `/health/ready` 是进程探针，不属于管理 API，继续保持无鉴权和稳定路径。原
`/health/status` 改为 `/operator/api/v1/system/status`；Prometheus 输出改为 `/operator/metrics`。

“API 始终可用”指 router、鉴权与有界诊断不依赖 Dashboard 或 workerd readiness。某个操作确实需要
workerd、S3 或其他当前不可用组件时，API 返回稳定、脱敏的 `503`，不能用缓存结果伪装成功。Dashboard
本身运行在 workerd 中，workerd 不可用时页面可能无法加载；CLI/curl 仍可通过 Operator API 诊断和恢复。

## 3. Operator API

**权威路径。**

JSON API 的固定根为 `/operator/api/v1`。典型资源路径为：

```text
GET    /operator/api/v1/meta
GET    /operator/api/v1/account
GET    /operator/api/v1/system/status

GET    /operator/api/v1/accounts/{account_id}/workers
GET    /operator/api/v1/accounts/{account_id}/kv/namespaces
GET    /operator/api/v1/accounts/{account_id}/d1/databases
GET    /operator/api/v1/accounts/{account_id}/r2/buckets
GET    /operator/api/v1/accounts/{account_id}/durable-objects/namespaces
GET    /operator/api/v1/accounts/{account_id}/queues
GET    /operator/api/v1/accounts/{account_id}/workflows

GET    /operator/api/v1/scheduler
GET    /operator/api/v1/queue-consumers
GET    /operator/api/v1/cron-activations
GET    /operator/api/v1/cache
GET    /operator/api/v1/images/capacity
```

上述是命名规则而非完整 endpoint 清单。现有 handler 的资源层级、HTTP method、idempotency、generation
fence、`force` 和不可变部署语义保持不变，只原子更换根路径。`/operator/api/v1/meta` 返回当前 release
identity、唯一 API version 和 capability 摘要，用于页面展示，不提供旧协议协商或 version fallback。

**统一约束。**

- API 根统一应用 auth、request ID、header/body bounds、日志脱敏、错误映射和 metrics middleware；各产品
  handler 不再各自决定是否鉴权；
- list API 使用有界 `limit` 与 opaque cursor。页面搜索不能先无界读取全部数据再在浏览器过滤；
- mutation 沿用稳定 idempotency key 与资源 generation/immutable identity，UI 二次确认不能替代服务端
  fence；
- API 返回产品对象和稳定错误，不复制 Cloudflare account API envelope；
- online CLI、Dashboard 和自动化没有私有快捷入口。离线 doctor、backup/restore 和 data-dir recovery
  仍是显式 break-glass 工具，必须遵守 data-dir lock，不能与运行中的 `ocd` 并发修改 authority；
- API availability 不授权自动修复损坏状态。持久化 authority 不一致时继续 fail closed。

**JavaScript SDK。**

新增 `packages/operator-sdk/`，package name 固定为 `@open-compute/operator-sdk`。它是一等 API client，
Dashboard 只是其中一个消费者；浏览器、Bun、Node.js 或其他提供标准 `fetch` 的 JavaScript runtime 都可以
直接调用。package 使用 TypeScript 7 strict mode，发布 ESM JavaScript、`.d.ts` 和显式 `exports`，不依赖
React、TanStack、Kumo、DOM 全局状态或 Node.js-only module。public API 不出现 `any`、未收窄的 `unknown`、
stringly typed resource kind 或 `Record<string, unknown>` response。

最小调用形式为：

```ts
import { createOperatorClient } from "@open-compute/operator-sdk";

const client = createOperatorClient({
  baseUrl: new URL("/operator/api/v1/", "https://compute.example.com"),
  getAccessToken: () => adminToken,
});

const page = await client.durableObjects.listObjects({
  accountId,
  namespaceId,
  limit: 100,
  signal: abortController.signal,
});
```

client 按资源暴露 `system`、`workers`、`kv`、`d1`、`r2`、`durableObjects`、`queues`、`workflows`、
`scheduler`、`cache` 和 `images` namespaces。公开方法与一个 HTTP operation 一一对应；SDK 不组合 promotion、
restore 或 delete 等高层 workflow，也不在客户端重写服务端 authority。

SDK 内部维护一个 typed operation registry。每项 operation 绑定固定 method/path template、严格 input schema、
严格 success schema 和允许的 stable error-code union；resource method 只能引用已登记 operation。公开调用均使用
单个对象参数，required/optional field、query、body、stream 和 idempotency requirement 由方法签名表达，不能
退化为 `request<T>(path, body)` 让调用方自行声称返回类型。

所有 ID、digest、opaque cursor 和 epoch-millisecond 字段使用 Zod branded types，例如 `AccountId`、
`WorkerId`、`DeploymentId`、`ResourceId`、`DurableObjectId`、`Sha256Digest` 和 `PageCursor`。由一个 API response
取得的 ID 可以直接传给下一个 method；外部字符串必须先经过 SDK 导出的 parser。lifecycle、content kind、
binding kind 和 stable error code 使用 closed enum/discriminated union，新增成员必须更新 schema 和 exhaustive
switch，不用宽泛 `string` 吞掉协议变化。

Zod schema 是 SDK 的 runtime contract 和 TypeScript 类型唯一来源。每个 operation 分别声明 params、query、
body、success response 与 error response schema；对象 schema 默认使用 `z.strictObject(...)`，协议没有明确允许
的未知字段一律拒绝。公开类型只通过 `z.input<typeof Schema>`、`z.output<typeof Schema>` 或 schema parser
导出，不再平行维护 interface、手写 DTO 或 `as` assertion。schema 的 transform 只用于协议明确规定的 canonical
表示，不做旧格式兼容、静默默认或错误数据修复。

transport 固定以下行为：

- `baseUrl` 必须是绝对 URL，并以 `/operator/api/v1/` 为 API root；不接受旧路径或自动探测 API version；
- `getAccessToken` 在每次请求前取值，SDK 不读取环境变量、localStorage、cookie 或全局 singleton，也不持久化
  token；
- 默认使用 `globalThis.fetch`，测试或特殊 runtime 可以显式注入兼容的 `fetch`；所有方法接受
  `AbortSignal`，取消会传到真实 HTTP request；
- SDK 在编码 URL/body 前用对应 Zod input schema 校验公开方法参数；收到 HTTP response 后先执行 content-type
  与 bytes/depth/count bounds，再把 JSON 作为 `unknown` 交给对应 success/error schema 校验。只有校验成功的
  `z.output` 能返回调用方；malformed、oversize、未知字段或 schema mismatch 抛出脱敏的
  `OperatorProtocolError`，不能退回 unchecked assertion 或原始 body；
- list 统一返回 `CursorPage<T>`；时间、ID、generation、digest 和 enum 保留服务端 canonical 表示，不在 SDK
  中猜测、修复或宽化；
- 非 2xx body 只有通过 operation 对应的 Zod error schema 后才抛出 `OperatorApiError`；该错误只携带 HTTP
  status、稳定 error code、sanitized message、request ID 和可选 `Retry-After`。无法通过 schema 的错误响应改为
  `OperatorProtocolError`；两者都不携带 raw response、Authorization header 或未验证 body；
- SDK 不自动重试。调用方只能对明确的只读请求做有界 retry；mutation 必须显式复用同一个
  idempotency key，不能因组件重渲染生成新 key；
- R2 download 返回 `ReadableStream`/`Response` 风格的有界 stream，upload 接受标准 body/stream；SDK 不把
  大对象强制聚合为 `ArrayBuffer`。普通 JSON response 继续受服务端 bounds 限制。

schema、operation registry、client 和 resource methods 按产品拆分，公共 transport/error/page primitives
保持小而稳定。SDK 不导出无类型 `request(path, init)` escape hatch；新增 Operator endpoint 时必须同时增加
SDK method、input/output/error schemas 和成功/失败 contract coverage。SDK 的 Zod schemas 是 JavaScript wire
contract 的 canonical machine-readable form；Rust handler 必须用同一组 canonical JSON fixtures 验证 request
和 response，并由真实 `ocd` integration test 运行 SDK 证明协议。这样消费端得到完整 compile-time inference，
HTTP trust boundary 又有 runtime validation；不把“Rust 与 TypeScript 各自能编译”误报为端到端 typesafe。

package 从一开始保持可发布：不设置 `private: true`，使用仓库的 Apache-2.0 license，并明确 version、files
和 exports；workspace 内通过 `workspace:*` 消费，外部项目可通过正常 package dependency 使用。是否向
npm 发布仍是独立的 release 写操作，需要在发行计划中单独授权；Dashboard 实现不以已经公开发布 npm
package 为前提。`packages/operator-sdk/dist/` 是可复现且 untracked 的 package build output。

**Dashboard 所需产品能力。**

Dashboard 优先复用现有 control workflow。确实缺少 item-level 管理 API 时，应在 owning crate/service 中增加
typed operator operation，再由薄 HTTP handler 暴露；禁止复用 `/internal/bindings/**` 或直接打开存储文件。

| 页面 | Day1 管理范围 | 明确不做 |
| --- | --- | --- |
| Overview | readiness、component/runtime 状态、release/capability、资源汇总 | 自动修复、展示 secret 或内部 token |
| Workers | Worker、deployment、promotion、rollback、route 管理 | 浏览器内安装依赖、编译源码或替代 `oc` 构建流程 |
| KV | namespace、backup/restore；通过新 typed operator API 浏览和修改 key/value | 调用 private binding protocol、无界导出 |
| D1 | database、migration、backup/restore；有界 SQL 与表数据浏览 | 直接打开 SQLite/WAL、绕过 D1 authority |
| R2 | bucket 与 object list/head/upload/download/delete | 向浏览器暴露 S3 credential、signed internal URL |
| Durable Objects | namespace 管理；已登记 Object ID、generation、lifecycle 与时间元数据 | Object 内存、SQL/KV、alarm、WebSocket 状态或任意方法调用 |
| Queues | Queue 配置、consumer 状态、backlog/失败聚合 | message body 浏览、control-plane publish 或伪造 delivery |
| Workflows | definition/version、instance、step/status/event 与有界 history | 输出未授权 payload、直接修改 scheduler SQLite |
| Platform | scheduler、cache/Images capacity、受支持的维护操作 | 任意 shell、文件浏览器或未审计的数据库控制台 |

KV、D1 和 R2 的数据管理是 Operator API 的产品能力，不属于 Cloudflare runtime compatibility 分母。它们
必须分别复用当前 KV executor、D1 backend 和 R2/S3 authority，继承已有 quota、权限、stream、digest 与
错误边界。大对象上传和下载采用专用 streaming bounds，不能被当前普通 JSON body limit 意外截断，也不能
为了 Dashboard 全局放宽所有请求。

**Durable Objects 页面。**

DO 页面调用现有 namespace/object registry authority：

- namespace 列表显示资源 ID、名称、owner Worker、class 与 lifecycle；
- Object 列表显示平台已经实际登记的 Object ID、generation、lifecycle、创建与更新时间；
- 支持 cursor pagination 和 Object ID 精确搜索；
- 未创建、未访问且未进入 registry 的理论 Object 不可枚举，页面必须明确这一点；
- Day1 Object 详情只展示上述元数据，不提供 Data Studio、状态编辑、SQL、KV、alarm、WebSocket attachment、
  arbitrary fetch/RPC 或 debug injection。

Cloudflare 的公开 Object inventory API 同样以 Object ID 和 `hasStoredData` 为边界，而不是返回任意实例状态：
<https://developers.cloudflare.com/api/resources/durable_objects/>。open-compute 不需要为了 Dashboard 读取
workerd private localDisk。

## 4. 管理员鉴权

**服务端契约。**

`server.admin_auth` 从“non-loopback 时必填”改为“始终必填”。继续只接受现有 env/file secret reference、
0600 regular file、bounded UTF-8 token 和 constant-time Bearer comparison。缺失、空值、env/file 不一致或
文件权限错误都使 `ocd` 启动失败。

所有 `/operator/api/v1/**` 和 `/operator/metrics` 请求必须携带：

```http
Authorization: Bearer <admin-token>
```

没有“loopback 即管理员”、匿名只读接口、Dashboard 专用万能 token 或从 tenant Worker 转发管理员身份的
例外。non-loopback admin listener 必须由 operator 配置可信 HTTPS reverse proxy/TLS；`ocd` 不把明文 HTTP
上的 Bearer token 宣传为安全的远程管理方式。

**浏览器登录。**

静态 SPA shell 不包含管理数据，可以在未认证时加载并显示 token 输入页。用户提交 token 后，页面先请求
`GET /operator/api/v1/account` 验证，再把 token 仅保存在当前页面内存中，并为后续同源请求设置
`Authorization` header。Day1 不使用 query string、URL fragment、localStorage、构建期注入、Worker env、
cookie session、OAuth 或多角色 RBAC 保存管理员凭据；刷新页面需要重新输入 token。

收到 `401` 时，页面立即清空内存 token 与已加载的管理数据。前端不记录 request/response body，不接入外部
analytics、字体、CDN script 或错误上报。静态资源设置严格 CSP、`frame-ancestors 'none'`、`nosniff` 与
referrer policy，release 不发布 source map。

Dashboard 与 API 固定同源，因此 Day1 不开启 credentialed CORS。需要远程访问时由 operator 在同一 origin
前配置 TLS/reverse proxy，而不是允许任意网站读取管理 API。

## 5. Dashboard Worker、前端技术栈与配置

新增 `packages/dashboard/`，与任何用户 Worker 一样使用 TypeScript strict mode、根 Bun workspace、现有
toolchain 和 Static Assets contract。Worker 项目配置保持最小：

```json
{
  "name": "open-compute-dashboard",
  "assets": {
    "directory": "dist",
    "not_found_handling": "single-page-application"
  }
}
```

项目不配置 `main`、secret、var、resource binding 或 service binding。Cloudflare 对 assets-only SPA 的
正式配置也是 `assets.directory` 配合
`not_found_handling = "single-page-application"`：
<https://developers.cloudflare.com/workers/static-assets/routing/single-page-application/>。

**依赖选择。**

| 层 | 选择 | 固定职责 |
| --- | --- | --- |
| View | React 19 | component lifecycle、rendering 与 error boundary；不承担 server-state cache |
| Language | TypeScript 7 | `strict`/`noEmit` 类型检查；不放宽现有禁止 `any`、ignore 和 double assertion 的规则 |
| Dev/build | Vite | 本地开发、HMR、route generation 与 SPA build；production transform/bundle 使用仓库固定的 Rolldown 路径 |
| Styling | Tailwind CSS 4 | CSS-first theme、layout、responsive 与少量组合样式；不另建平行 token system |
| Components | `@cloudflare/kumo` | 表单、按钮、Dialog、Table、Tabs、Toast、navigation shell 和 semantic design tokens |
| Icons | `@phosphor-icons/react` | 满足 Kumo peer contract；不混用第二套 icon library |
| Routing | `@tanstack/react-router` | typed route/search params、nested layout、deep link 和 route-level code splitting |
| Server state | `@tanstack/react-query` | query lifecycle、cancellation、pagination、invalidation 与只读请求的 bounded retry |
| HTTP client | `@open-compute/operator-sdk` | branded ID、typed operation、URL、auth、DTO validation、error 与 stream；Dashboard 中禁止散落 raw `fetch` |
| Boundary schema | Zod 4 | 只由 Operator SDK 验证不可信 HTTP payload；页面直接消费已经验证的类型 |

所有第三方版本在根 `package.json` catalog 和唯一 `bun.lock` 中精确 pin；workspace package 只引用 catalog 或
`workspace:*`，不引入 npm/pnpm/Yarn lockfile。Vite 负责 dev/build orchestration，TypeScript 7 只执行
`noEmit` 类型检查；不能用 Babel-only、第二套 bundler 或 Vite 默认 fallback 绕过仓库的 Rolldown 构建要求。
Vite integration 明确使用与所选版本匹配的 React、Tailwind 和 TanStack Router plugins；这些 plugin 也由
根 catalog pin，不通过脚手架生成第二份 package manager 配置。

Kumo 作为设计系统依赖使用，不复制其源码，也不自行维护一套相似组件。页面优先使用 granular imports，
Tailwind 只补 layout 和 Kumo 尚未覆盖的组合样式；颜色、间距、surface、focus 与 dark mode 使用 Kumo semantic
tokens。Kumo 当前文档要求 Tailwind v4 显式扫描 package dist，并按顺序载入 theme：

```css
@source "../node_modules/@cloudflare/kumo/dist/**/*.{js,jsx,ts,tsx}";
@import "@cloudflare/kumo/styles/tailwind";
@import "tailwindcss";
```

实际 `@source` 相对路径必须由 `packages/dashboard/src/app.css` 的位置验证，不能照抄后留下缺失 Dialog/layout
样式。Kumo 推荐 granular component imports 以保持 tree-shaking；其 React peer、Phosphor icon peer、Tailwind
集成与 MIT license 纳入 dependency/NOTICE review。参考：
<https://github.com/cloudflare/kumo/blob/main/packages/kumo/README.md>。

**Router 与 Query ownership。**

Dashboard 使用 TanStack Router 的 file-based routes，Vite plugin 生成唯一 route tree。路由按产品拆到
`src/routes/`，根 layout 提供 navigation、token state、QueryClient 和 error boundary；`/login`、overview、
Workers、KV、D1、R2、Durable Objects、Queues、Workflows 与 platform 页面各自 lazy load。Router 的 basepath
固定为 `/operator`，筛选、cursor、tab 和选中资源使用经过验证的 typed search params，使深链接和浏览器
前进/后退不依赖隐藏 component state。`routeTree.gen.ts` 是提交到仓库的可复现 generated source，禁止
手改；CI 在 typecheck/build 前重新生成并要求 Git diff 为空。

TanStack Query 只管理服务端状态。每个产品拥有稳定 query-key factory 和 `queryOptions`，query function
只调用 `@open-compute/operator-sdk` 并把 Query 提供的 `signal` 传入 SDK。分页保留上一页可见数据；健康状态
等少量页面可按可见性有界 polling，普通 catalog 不做全局高频 refetch。只读 network/5xx 可配置 bounded
retry，`401`、`403`、其他 stable 4xx 与 mutation 不重试。mutation 成功后按资源 generation 精确 invalidate；
promotion、rollback、delete、restore、purge 等操作不做 optimistic update，以服务端返回的 authority 状态为准。

TanStack Query adapter、query keys 和 React hooks 属于 `packages/dashboard`，不能放进 Operator SDK。这样外部
JavaScript 调用者不需要安装 React/TanStack，Dashboard 也不会绕过统一 client。query、loader、mutation 和
route component 的数据类型全部从 SDK method return type 推导，不重新声明 Dashboard DTO 或使用 assertion
修正不匹配。

建议源码边界：

```text
packages/
├── operator-sdk/
│   └── src/{client,transport,error,schemas,resources}/
└── dashboard/
    └── src/{routes,queries,components,features,styles}/
```

构建与发行约束：

- `packages/dashboard/dist/` 是可复现生成物且保持 untracked；源码、lock、配置和测试进入仓库；
- 根 `bun run build` 先构建 Operator SDK、生成 route tree、执行 TypeScript 7 typecheck 和 Vite production
  build，再用现有 asset scanner、canonical manifest 和 immutable deployment format 生成 Dashboard Worker
  artifact；
- 正式单文件发行将该 release-owned artifact 嵌入 `ocd`，启动时不运行 Bun/Node、不下载 UI、不访问外部
  CDN；
- artifact 作为 system-owned reserved deployment 运行，复用 stock workerd 与现有 assets router，但不进入
  tenant Worker catalog，不能被 tenant API rename、replace、route 或 delete；
- `/operator/api/**` 和 `/operator/metrics` 始终由 `ocd` 在 SPA dispatch 前截获，Dashboard Worker 无法覆盖
  或代理这些路径；
- `index.html` 使用 revalidation/no-cache，带 digest 的 JS/CSS/font 使用 immutable cache policy；所有 asset
  URL 与 client-side route 都以 `/operator/` 为 base；Vite release build 固定 `sourcemap: false`。

这保留了“Dashboard 是普通 Worker 项目”的开发、构建和 runtime 约束，同时保留其作为平台自带管理 UI
所需的 system ownership。它不是第二个 daemon、sidecar 或特权控制 Worker。

**配置。**

新增一个静态、拒绝 unknown fields 的顶层配置：

```toml
[server]
public_bind = "127.0.0.1:8787"
admin_bind = "127.0.0.1:8788"
admin_auth = { env = "OPEN_COMPUTE_ADMIN_TOKEN" }

[dashboard]
enabled = false
```

`dashboard.enabled` 默认 `false`，只在启动时读取；Day1 不增加热重载、自定义 URL prefix、外部 Dashboard
URL、CDN origin、任意本地目录或开发代理配置。修改后通过受控重启生效。即使 `enabled = false`，
`server.admin_auth` 仍然必填，因为 Operator API 始终存在。

## 6. 实施归属

- `crates/core`：增加 `DashboardConfig`，把 admin auth 改为无条件必填并完成配置验证；
- `crates/service`：建立单一 operator router/auth middleware、迁移全部管理路径、保留 health probes、增加
  Dashboard dispatch 和 release-owned artifact composition；
- 各 owning crate/service：补齐 Dashboard 实际需要的 KV/D1/R2 typed item operations，不把 authority
  搬到 HTTP handler；
- `packages/operator-sdk`：纯 Fetch/TypeScript client、Zod schemas、稳定错误、streaming 与 contract tests；
- `packages/dashboard`：React/Kumo 页面、TanStack Router/Query adapter、token state、可访问性与前端测试；
- `packages/toolchain`：`oc` 在线命令复用 Operator SDK；只在普通 project/artifact 路径确有缺口时扩展，
  不增加 Dashboard 专用 bundle format；
- release scripts：在 Cargo 消费前构建并校验 Dashboard artifact，把它与 workerd/runtime assets 一起纳入
  单文件 release identity；
- package docs：记录配置、认证、反向代理、页面能力和 API 路径。

## 7. 验收

实现期按仓库规则只跑一次相关单轮检查；实现、review 和修复完成后再执行一次最终验收链。至少证明：

1. 缺少 `server.admin_auth` 时，loopback 和 non-loopback 配置都 fail closed；错误不含 token；
2. Dashboard 关闭时 `/operator/` 为 `404`，授权 Operator API 正常，未授权请求统一为 `401`；
3. workerd 停止或未 ready 时，Operator API 的 system status 仍可访问，需要 runtime 的操作稳定返回 unavailable；
4. 独立与 merged listener 都不会把 `/operator/**` 交给 tenant route；旧管理路径均为 `404`，live source 中
   没有 alias；
5. Dashboard 开启时，根路径、hashed assets 和 client-side deep link 均由 stock workerd 的正常 assets-only
   deployment 返回；API path 不被 SPA fallback 吞掉；
6. 同一 `packages/dashboard` 项目可以经过普通 toolchain 构建，release 内 artifact 与 source/manifest pin
   一致，干净 checkout 不依赖已存在的 `dist/`；
7. Operator SDK 在 browser、Bun/Node fake-fetch 和真实 `ocd` contract case 中使用同一 methods/types；正向
   type tests 证明 branded ID 和 method inference，负向 `@ts-expect-error` cases 证明错 ID、缺 required field、
   非法 enum 和错误 response assumption 不能编译；runtime 覆盖 URL/auth、abort、cursor、malformed/oversize
   response、未知字段、success/error schema mismatch、stable error、stream 与 mutation idempotency，SDK 外
   没有 Dashboard raw `fetch`；
8. TanStack Router deep link/search params、Query cancellation/invalidation/retry 分类和 Kumo/Tailwind production
   CSS 均在 clean build 中生效；route tree、SDK dist 和 Dashboard dist 都由源码可复现生成；
9. token 不出现在 HTML、JS bundle、URL、日志、metrics、support bundle、错误响应或 source map；`401`
   会清空前端状态；
10. Workers/KV/D1/R2/Queue/Workflow 的管理操作继续经过现有 authority、quota、idempotency 和恢复路径；
11. DO 页面只列 registry metadata；测试使用 tenant canary 证明 SQL/KV/内存状态不会出现在 API 或页面；
12. UI disable/enable、SDK、`oc` 与 curl 使用同一 API contract，没有 Dashboard-only mutation。

文档-only 阶段只运行 `git diff --check` 和链接/路径核对。实现完成后按 `AGENTS.md` 执行格式、静态检查、
coverage 与最终三轮策略 Gate，不提前重复完整 Gate。

## 8. 非目标

- Cloudflare Dashboard、`/client/v4`、账号组织、billing、plan、RBAC 或 Wrangler remote API parity；
- Localflare 的 hosted dashboard、sidecar、浏览器连接任意本地端口或自动读取 Wrangler 配置；
- DO Data Studio、任意对象状态 introspection、远程方法调用或 debug injection；
- Queue message body inspector、control-plane producer、Worker 在线源码编辑器和生产时依赖安装；
- 独立 Dashboard server、Node.js production runtime、外部 CDN、动态插件系统或自定义主题平台；
- 把 React hooks、TanStack Query、Kumo component 或未经验证的通用 request escape hatch 塞进 Operator SDK；
- 为旧管理路径、旧无鉴权 loopback 行为或旧 Dashboard artifact 保留兼容分支。
