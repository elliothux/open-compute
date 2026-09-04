# P12：Cloudflare Browser Run 兼容设计

状态：Day 1 合同与架构设计完成；待 G0、外部 Browser Provider、实施与验收。

本文细化 [P6 Cloudflare v4 API 与 Wrangler 子集兼容设计](implemented/p6-cloudflare-v4-wrangler-compatibility.md)
中的 `browser` binding、Browser Run API、DevTools session 和固定 Wrangler commands。方案参考当前 AI Search 的
operator-owned provider 模型与固定 Miniflare Browser Rendering 实现，但不把 Miniflare 的开发期浏览器下载、内存状态或
硬编码容量复制到生产。

## 1. 范围与结论

Cloudflare 已把 Browser Rendering 产品名更新为 **Browser Run**，但固定 API/config/binding 中仍使用
`browser-rendering` / `browser`。open-compute 保留这些标准名字，不发明 `browser_run` 配置或 vendor route。

P12 Day 1 目标：

- `wrangler.jsonc` 标准 `browser: { binding }`；
- multipart metadata `{name,type:"browser"}`；
- stock workerd 中能被固定 `@cloudflare/puppeteer` / `@cloudflare/playwright` 使用的 Browser Fetcher binding；
- 固定 Workers types 的 `BrowserRun.fetch()` 与 `BrowserRun.quickAction()`；
- `wrangler browser create/list/view/close`；
- `/client/v4/accounts/{account_id}/browser-rendering/**` 的选定 Quick Actions；
- DevTools session HTTP 与 CDP WebSocket；
- operator-owned 外部 Browser Provider 的 session allocation、browser execution 与 cleanup。

结论：**Browser Run 可以、也应该调用外部执行服务**，其角色与 AI Search 的 embedding/generation provider 类似。但
必须区分“外部基础设施依赖”和“open-compute 自己的 sidecar”：

1. `ocd` 仍是唯一公开 listener、认证/授权/session-scope authority；
2. Browser Provider 由 operator 在 loopback 或受控内网预先部署，open-compute 不打包、不下载、不搜索 PATH、不启动或
   supervise 它；
3. 正式 open-compute release 仍只有一个原生 `ocd` executable 和既有单个 stock workerd child；
4. provider URL、credential、raw CDP endpoint 与 provider session ID 永不暴露给 tenant；
5. provider 未配置、能力不足或不可用时，browser upload/API fail closed，不回退到本机任意 Chrome；
6. tenant 不能在 `wrangler.jsonc`、API body 或 Worker call 中指定 endpoint、browser binary、launch flags 或 credential。

这保留了 on-prem 部署灵活性，同时不把 Chromium 生命周期、sandbox、平台差异和巨型 browser binary 塞进 `ocd`。

## 2. Compatibility authority

实施和 qualification 固定：

- [Cloudflare Browser Run API](https://developers.cloudflare.com/api/resources/browser_rendering/)；
- [Chrome DevTools Protocol](https://developers.cloudflare.com/browser-run/cdp/)；
- [Browser session management](https://developers.cloudflare.com/browser-run/cdp/session-management/)；
- [Wrangler browser commands](https://developers.cloudflare.com/browser-run/reference/wrangler-commands/)；
- [Browser Run limits](https://developers.cloudflare.com/browser-run/limits/)；
- [Cloudflare Puppeteer](https://developers.cloudflare.com/browser-run/puppeteer/)与固定 package；
- [Live View](https://developers.cloudflare.com/browser-run/features/live-view/)；
- [Browser Run rename changelog](https://developers.cloudflare.com/changelog/post/2026-04-15-br-rename/)；
- `wrangler@4.127.1` config schema、upload builder、`browser-rendering/**` commands/tests；
- 固定 `@cloudflare/puppeteer`、`@cloudflare/playwright`、Workers types 与 Miniflare source snapshot；
- 固定 Cloudflare HTTP/WebSocket trace 和 OpenAPI revision/hash。

网页和 upstream source 用于发现合同；进入 Gate 的 route、query、body、header、raw response、WebSocket frame、close code、
错误与 package call sequence 都必须固定为 fixture。Browser Run 当前仍在快速演进，未进入 inventory 的新功能默认
unsupported。

## 3. 三层协议，不混为一个 API

| 层 | 调用方 | 协议 | 是否公开 |
| --- | --- | --- | --- |
| Cloudflare public API | Wrangler、SDK、用户 HTTP client | `/client/v4/.../browser-rendering/**` | 是 |
| Worker Browser binding | `@cloudflare/puppeteer` / Worker | Fetcher + `/v1/**` HTTP/WebSocket | 只对已绑定 Worker 可见 |
| Browser Provider | `ocd` | operator-private adapter | 否 |

Public API 中 JSON route 是否使用 v4 envelope 必须逐 route 固定。尤其固定 Wrangler 的 DevTools helper 明确把
Browser Run DevTools 当作 **raw JSON**，不能由 P6 的通用 `fetchResult()`/v4 envelope middleware 包装。image/pdf/body 与
WebSocket 同样保持原始媒体类型/upgrade。

Worker binding 的 `/v1/**` 是固定 Cloudflare packages/Miniflare 可观察到的 service contract，但它不是 tenant 可直接
访问的 public management endpoint。Provider adapter 可以采用相同协议以减少转换，也不能因此绕过 `ocd` 的 account、
binding 和 session scope。

## 4. Wrangler 与 upload contract

### 4.1 `wrangler.jsonc`

标准配置：

```jsonc
{
  "$schema": "./node_modules/wrangler/config-schema.json",
  "name": "browser-app",
  "main": "src/index.ts",
  "compatibility_date": "2026-09-03",
  "browser": {
    "binding": "BROWSER"
  }
}
```

规则：

- `browser` 是 non-inheritable singleton，named environment 需要显式重复声明；
- server-side immutable state 只有 binding name；
- 固定 schema 中的 `remote` 只控制 local development，不上传；
- 不接受 endpoint、provider、browser、executable、args、headless、user_data_dir、team 或 user 等自定义 key；
- binding name 与所有其他 bindings 共用唯一性校验；
- provider 未配置或 P12 capability 未通过时，upload fail closed，不能删除 binding 后继续部署。

### 4.2 Multipart metadata

固定 Wrangler 生成：

```json
{
  "bindings": [
    { "name": "BROWSER", "type": "browser" }
  ]
}
```

descriptor 是 immutable Version state。provider name、base URL、auth、Chrome revision 和 session limits 是 operator
runtime authority，不写进 tenant Version；但每次 session 会记录 secret-free provider contract digest，用于配置变更
后的 fencing/reconciliation。

## 5. Worker binding contract

Miniflare 把 `browser` binding 组装成 service binding，固定 Cloudflare packages 把它当 Fetcher 使用。open-compute
沿用相同边界：

```text
tenant Worker + fixed Workers types / @cloudflare/puppeteer
  -> env.BROWSER.fetch() / env.BROWSER.quickAction() / WebSocket
  -> packages/runtime BrowserTransport
  -> ocd BrowserService
  -> external Browser Provider
```

不需要修改 stock workerd，也不在 tenant isolate 注入 Node/Chrome process handle。runtime facade 精确提供
`BrowserRun.fetch(input, init)` 和九个固定 `quickAction(action, options)` overload；后者转换为对应 Browser Run action
route 并原样返回标准 `Response`，不返回自定义 object。底层 `BrowserTransport` 只携带：

```text
account_id, script_id, version_id, deployment_id,
binding_name, descriptor_sha256, capability_version
```

它不携带 provider URL/secret/raw session ID。每次 fetch 都重新验证 immutable deployment snapshot 与 binding identity，
沿用现有 KV/D1/R2/Vectorize/AI Search 的 scoped transport pattern。

### 5.1 固定 binding route inventory

G0 先从固定 Puppeteer/Playwright 与 Miniflare source 抽取实际 call graph，目标包含：

```text
GET    /v1/acquire
GET    /v1/sessions
GET    /v1/limits
GET    /v1/history
GET    /v1/connectDevtools                 (WebSocket)
GET    /v1/devtools/session
GET    /v1/devtools/session/{session_id}
POST   /v1/devtools/browser
GET    /v1/devtools/browser/{session_id}
DELETE /v1/devtools/browser/{session_id}
GET    /v1/devtools/browser/{session_id}/json[/version|/list|/protocol]
PUT    /v1/devtools/browser/{session_id}/json/new
GET    /v1/devtools/browser/{session_id}/json/activate/{target_id}
GET    /v1/devtools/browser/{session_id}/json/close/{target_id}
GET    /v1/devtools/browser/{session_id}/page/{page_id}   (WebSocket)
```

route 只是 inventory seed，不是凭空承诺。HTTP method、query、headers、response JSON、legacy length-prefixed CDP framing、
native CDP WebSocket framing、session header 与 errors 由固定 packages/tests 锁定。

### 5.2 Session visibility

Cloudflare binding 的 `sessions()` / reconnect 可见范围必须由固定 package + Cloudflare differential 确认，不能因 LynxOS
产品需求擅自改成 per-user custom API。平台 invariant 是绝不跨 account；account 内究竟按 account、script 还是 binding
划分，在 BR-G0 固定并记录。

若官方语义是 account scope，open-compute 就保持 account scope。LynxOS 要求更强的用户/应用隔离时，应使用独立
open-compute account/provider partition 或在 agent/app 层不共享 session ID，而不是改变标准 Browser binding 返回值。

## 6. Public Browser Run API

### 6.1 DevTools 与 Wrangler

固定 `wrangler@4.127.1` 使用：

```text
wrangler browser create [--keep-alive <seconds>] [--lab] [--json] [--no-open]
wrangler browser list [--json]
wrangler browser view [session-id] [--target <selector>] [--json] [--no-open]
wrangler browser close <session-id> [--json]
```

对应 route family：

```text
GET    /client/v4/accounts/{account_id}/browser-rendering/devtools/session
POST   /client/v4/accounts/{account_id}/browser-rendering/devtools/browser
GET    /client/v4/accounts/{account_id}/browser-rendering/devtools/browser/{session_id}/json
DELETE /client/v4/accounts/{account_id}/browser-rendering/devtools/browser/{session_id}
```

以及固定 CDP target/version/protocol/new/activate/close/page routes。DevTools response 是 raw JSON/101，不套 v4 envelope。
`devtoolsFrontendUrl` 必须指向 deployment-owned Live View/DevTools proxy，不能返回 provider URL 或 Cloudflare
`live.browser.run` origin。

`--lab`/WebMCP 是实验能力。Day 1 route 识别后明确拒绝 `lab=true`，除非固定 browser/provider、security review 与官方
differential 已单独通过；不能忽略该 flag 启动普通 browser。

### 6.2 Quick Actions

Day 1 public subset按固定 OpenAPI实现：

- content；
- screenshot；
- PDF；
- snapshot；
- scrape；
- links；
- markdown；
- accessibility tree；
- `/json` structured extraction 只有在 AI provider contract 通过后开放。

每个 operation 独立登记：request schema、navigation/options、response schema/media type、timeout、body/output bound、
provider capability 与错误。二进制 screenshot/PDF 直接 stream；HTML/text/JSON 是否套 envelope 不从其他 route 推断。

`crawl`、recording、WebMCP/lab、human-in-the-loop、完整 browser persistence/profile upload、任意 extension、custom executable
和未固定的新 beta endpoint 不在 Day 1。route/field 存在但未支持时返回明确 Cloudflare-style failure，不能忽略参数执行
一个语义更弱的 action。

`/json` 需要模型时复用 AI Search 的 operator-owned model catalog、secret reference、bounded request/response、timeout 和
stable provider error classes。tenant 不能在 Browser request 中提供 AI endpoint/key。模型不可用时 `/json` fail closed，
不影响不需要模型的 screenshot/content 等 operation。

## 7. External Browser Provider

### 7.1 Provider 角色

Provider 负责：

- 启动已安装、已固定 revision 的 browser process/container；
- 创建、查询、关闭 session；
- 暴露 CDP HTTP/WebSocket；
- 执行或协助执行声明支持的 Quick Actions；
- 对 browser process、profile/temp files、crash/orphan 做 cleanup；
- 执行 operator 指定的 sandbox、network/egress、CPU/memory/disk/process limits。

`ocd` 负责：

- v4/account/binding auth、request/schema validation 和 public error contract；
- capacity admission、session lease、public session ID、provider mapping 与 contract fencing；
- HTTP/WebSocket proxy 的 backpressure、deadline、redaction 与 observability；
- provider health/capability negotiation 和 restart reconciliation；
- 防止任何 provider identity/credential/URL 到达 tenant。

### 7.2 Operator config

配置模式复制 AI provider 的原则，不复制 tenant-visible key。建议 domain：

以下是 schema 形状而不是可直接复制的配置；容量值均为 operator 必填正整数：

```text
[browser]
default_provider = "local"
max_sessions = <required-positive-integer>
max_pending_acquires = <required-positive-integer>
acquire_timeout_ms = <required-positive-integer>
command_timeout_ms = <required-positive-integer>
idle_reap_interval_ms = <required-positive-integer>

[browser.providers.local]
adapter = "cloudflare_browser_v1"
base_url = "http://127.0.0.1:9224"
browser_revision = "operator-pinned-revision"

[browser.providers.local.auth]
kind = "none"
```

上例只展示结构，不定义默认数值。真正 schema：

- `deny_unknown_fields`；
- `auth:none` 仅允许显式 loopback HTTP；非 loopback 必须使用 secret reference 的 Bearer，后续 mTLS 只有共用 transport
  已实现才开放；
- `base_url` canonicalize，禁止 query/userinfo/fragment；
- process startup 在不访问网络的情况下完成全部静态 validation；
- provider health check 发生在 readiness/调用路径，不阻塞 production binary discovery；
- 全部 capacity 是 operator deployment config，不写入 `wrangler.jsonc`，也不使用 LynxOS “约 20 人”默认值。

实现时使用非零类型或显式 required fields；文档不提供 open-compute 或 LynxOS 的隐含默认容量。

### 7.3 Frozen provider contract

启动时把 config 解析为不含 secret 的 `ResolvedBrowserProviderContract`：

```text
provider_name
adapter/version
canonical endpoint digest
auth_kind
browser_revision
supported binding-protocol revision
supported quick-actions set
supported CDP protocol/revision
contract_sha256
```

session 创建时保存 contract digest。配置变化后旧 session 不允许透明切到另一个 provider/browser revision；只能继续由
原合同服务、明确关闭，或标为 `lost`。与 AI provider 一样，secret value 不进入 digest、SQLite、Debug 或 metrics。

### 7.4 Adapter 最小化

首选 provider adapter 复用固定 Cloudflare/Miniflare Browser binding `/v1/**` route 与 CDP，不再设计第二套 browser RPC。
Quick Actions 可由 provider 接受固定 Cloudflare request/response shape，`ocd` 只做标准 public route translation、validation
和 scope。若某 provider 只提供 raw CDP，则必须通过正式 adapter 实现同一 frozen contract，不能在 handlers 中散落
provider-specific URL/JSON。

Day 1 只承诺一个 `cloudflare_browser_v1` adapter。Browserless、Selenium Grid、Playwright server、remote Chrome 等产品
可以由部署方适配，但不把它们各自 API 变成 public compatibility surface。

## 8. Session model、lease 与 reconciliation

SQLite 建议 authority：

```text
browser_sessions
  id, account_id, visibility_scope, provider_name,
  provider_session_id_ciphertext, provider_contract_sha256,
  state, keep_alive_ms, created_at, connected_at,
  last_activity_at, closing_at, closed_at, lost_at,
  lease_generation, close_reason
```

public session ID 是 open-compute opaque ID；provider session ID 不返回。若 provider ID 足以连接 CDP，按 sensitive value
处理并加密/受保护存储；日志只使用 public ID 或 digest。

状态机：

```text
acquiring -> ready -> connected -> ready
ready/connected -> closing -> closed
acquiring/ready/connected/closing -> lost
```

规则：

- admission permit 从 acquire 开始持有到 closed/lost，不是一次 HTTP request 的短 semaphore；
- pending acquire 有独立 bounded queue 和 deadline；
- public ID 只有在 provider session 创建并登记成功后可用；失败要 best-effort close provider orphan；
- reconnect 不刷新超出标准 keep-alive 的永久 lease；具体 idle semantics 由固定 Cloudflare trace确定；
- close 幂等行为、unknown/closing session response 按固定 route qualification；
- `ocd` restart 后对 provider list/status 做 reconciliation：认领匹配合同与记录的 session、关闭可识别 orphan、标记缺失
  session `lost`；
- provider 不支持安全 list/reconcile 时，readiness 明确降级，restart 后旧 session 全部 `lost` 并 best-effort reap，不能猜测
  WebSocket endpoint。

## 9. HTTP/CDP/WebSocket proxy

所有 public/binding traffic 经 `ocd`：

- HTTP request/response streaming 有 size/deadline/backpressure；
- WebSocket upgrade 前完成 account/binding/session/target authorization；
- `Origin`、Authorization、Cookie、forwarded headers 与 provider credential 分开处理；
- provider WebSocket URL 只在进程内构造，redirect/response body 不能把它泄露给 client；
- text/binary frame、fragmentation、ping/pong、close code/reason 与 half-close 按固定 CDP behavior 转发；
- per-connection message/frame/aggregate bytes 和 outbound queue 有 operator guard；
- client disconnect 释放 connection lease，但是否关闭 browser session 取决于标准 session contract；
- proxy 不能解析并重写正常 CDP messages；只在 admission/header/session fencing 边界检查；
- legacy `/v1/connectDevtools` 的 length-prefix framing 与 native page WebSocket 分开测试。

`wrangler browser view` 所需 `devtoolsFrontendUrl` 指向 `ocd` 自带的静态 DevTools frontend/proxy route 或可验证的
deployment-owned frontend。P12 不在启动时从公网下载 DevTools UI。若不能合法、可复现地随 release 提供兼容 frontend，
`view` Gate 不通过，不能只返回 provider internal URL。

## 10. Quick Action 执行

Quick Action 采用统一 pipeline：

```text
authenticate -> validate fixed schema -> capacity admission
  -> acquire/reuse isolated provider session
  -> navigate/action under deadline
  -> bounded/streamed result validation
  -> close/release according to official contract
```

约束：

- URL、redirect、subresource、download、WebSocket 和 DNS 都发生在 provider network namespace；
- `ocd` 只把 validated action 发送到 operator-fixed provider，绝不直接 fetch tenant URL 代替浏览器；
- response body、DOM、screenshot/PDF、AI prompt/result 默认不进入 logs；
- screenshot dimensions/format、PDF options、selectors、wait conditions、headers/cookies、navigation timeout 逐字段 allowlist；
- unsupported field fail closed，不把 `waitForSelector` 等参数静默丢掉；
- output 不自动写入 R2/Artifacts/团队目录。Worker/调用方要持久化时显式调用对应 binding，保持权限和失败边界清晰；
- `/json` 的 AI call 与 browser session 共用 end-to-end deadline，不能无限等待 provider；
- provider quick-action capability 不足时对应 route `unsupported`，不通过执行自定义脚本模拟半套 semantics。

## 11. Isolation 与 security

Browser Run 执行不可信网页，不能只因为部署在内网就把 browser provider 当成普通 HTTP client。最低 invariant：

- 每个 session 使用独立 incognito/browser context 或固定 provider 更强隔离；cookie/cache/storage/profile 不跨 scope 复用；
- provider process/container 不能读取 `ocd` data directory、socket、secret files 或 workerd runtime files；
- provider credential 只能由 `ocd` 使用；tenant 无法直连 provider listener；
- launch flags 由 operator 固定，tenant 不能传 `--no-sandbox`、extension、proxy、user-data-dir 或 remote-debugging address；
- browser/CDP endpoint 只监听 loopback/受控 service network并鉴权；
- account/session/target auth 在每次 HTTP 和 WebSocket upgrade 执行，不能只在 create 时检查；
- session ID 高熵且不可枚举；list/get/close 的 not-found/forbidden 不泄露跨 account presence；
- URL/redirect/private IP/metadata endpoint/内网访问由 provider egress policy决定并在 capability manifest 明示；若 operator
  允许内网访问，这是部署策略，不宣称 Cloudflare 网络安全等价；
- downloads、file chooser、clipboard、camera/mic、printing、WebUSB/WebBluetooth 与本地 filesystem 默认禁用；
- browser crash、renderer hang、CDP flood、zip bomb/download、巨大 DOM/canvas 都受 provider/ocd 双层 limit；
- P7 logs 只记录 stable metadata/error class，清洗 URL query、headers、cookies、DOM、CDP payload 和 screenshots。

## 12. Limits 与 backpressure

不复制 Cloudflare plan 的并发/session/browser-minutes 数值，也不设置 LynxOS 20 人默认值。operator capacity 至少包括：

- active sessions、pending acquires、sessions per account；
- acquire/command/navigation/idle/maximum lifetime deadline；
- concurrent Quick Actions 和 CDP connections；
- HTTP body、result、WebSocket frame/message/queue bytes；
- screenshot/PDF dimensions/bytes、DOM/text/JSON output；
- provider request in-flight、reconcile/close concurrency；
- session history/metadata retention。

固定 Browser binding 的 `/v1/limits` 返回值要反映 effective deployment capacity，但字段/单位必须与官方 package
一致。它是 deployment capability，不伪装成 Cloudflare plan。P9 另外统计 Worker invocation subrequest/CPU；browser
session permit 不能因 Worker request 结束就漏归还或被错误释放。

## 13. Miniflare 参考边界

采用的 Miniflare 证据：

- `browser` binding 被组装为 service binding；
- `/v1/acquire`、sessions、limits、history 与 DevTools route shape；
- Durable Object 风格的 session identity/lifecycle；
- Chrome HTTP/CDP 与 WebSocket proxy，包括 target JSON；
- local `remote` binding 的开发期语义。

明确不复制：

- 启动时自动下载 Chrome；
- Node `child_process` browser launcher；
- in-memory Durable Object/session authority；
- hard-coded concurrency `6` 或任何 Cloudflare plan number；
- Miniflare loopback `/browser/launch|status|close|sessionIds` 作为 public API；
- dev-only retry/auth/error messages；
- 把一个开发机 Chrome 当作 multi-account production isolation。

`wrangler dev` 的本地体验继续由上游 Wrangler/Miniflare负责；P12 qualification 针对真实 `ocd` + stock workerd +
operator Browser Provider。

## 14. Error 与 observability contract

稳定 provider error classes：

```text
invalid_request, unsupported, capacity_exhausted, acquire_timeout,
provider_unauthorized, provider_unavailable, browser_crashed,
session_not_found, session_lost, target_not_found,
navigation_failed, action_timeout, malformed_response
```

Public code/message/status 由固定 Cloudflare route fixture mapping；内部 class 不直接作为 vendor error body。retryability、
`Retry-After` 与 close code有明确表，不能根据 provider message regex 猜测。

推荐 metrics/log dimensions：

```text
account_id, script_id, operation, provider_name,
provider_contract_sha256, result_class, session_state,
queue_wait_ms, acquire_ms, duration_ms, bytes_in, bytes_out
```

session/target ID 只记录 bounded/digest form；URL host 仅在 operator 明确允许的低基数审计日志中出现，不作默认 metrics label。
P7 realtime tail 可以显示 Worker 触发的 browser call outcome，但不能带 DOM、CDP message、cookie、header、AI content 或
provider detail。

## 15. 实施顺序

### BR0：冻结合同

- 固定 Wrangler schema/commands/upload metadata；
- 固定 Puppeteer/Playwright/types package versions、integrity 与 Browser binding call graph；
- 固定 public OpenAPI、raw DevTools/WebSocket traces、Quick Action schemas；
- 建 route/field/media/frame/error/capability inventory；
- 记录 Browser Rendering -> Browser Run 只改产品名、不改兼容 path 的规则。

### BR-G0：end-to-end feasibility Gate

- 一个 operator-managed provider 实现 frozen adapter；
- 真实 `ocd` + stock workerd 中，固定 `@cloudflare/puppeteer` launch/connect/newPage/navigate/screenshot/close 通过；
- HTTP/CDP/WebSocket 代理在 backpressure、disconnect、provider crash 后正确收敛；
- 固定 Wrangler create/list/view/close 的 raw route/URL shape 可实现；
- provider 不进入 open-compute release，不要求 workerd fork、PATH browser 或 startup download；
- browser/DevTools frontend revision、license、发行和跨平台/provider deployment 可复现。

Exit：若 G0 失败，`browser` binding 和 Browser Run routes 保持 unsupported；不能只做 Quick Actions 后宣称完整 binding。

### BR1：provider/config/session authority

- BrowserConfig、secret reference、contract digest、capability handshake；
- session schema/state/lease/admission；
- provider client、stable error classes、health/readiness；
- restart reconciliation、orphan reap、contract fencing。

### BR2：runtime binding

- P6 multipart decode、immutable Version descriptor、settings/download/rollback；
- `packages/runtime` BrowserTransport Fetcher；
- fixed `/v1/**` HTTP and legacy/native WebSocket behavior；
- account/session visibility differential。

### BR3：DevTools 与 Wrangler

- raw public DevTools routes、session/target APIs；
- CDP WebSocket proxy；
- deployment-owned `devtoolsFrontendUrl` / Live View；
- fixed Wrangler create/list/view/close subprocess Gate。

### BR4：Quick Actions

- content/screenshot/PDF/snapshot/scrape/links/markdown/accessibility tree；
- per-route schema/media/streaming/error limits；
- `/json` 与 operator AI provider 集成；
- unsupported beta/experimental route/field fail closed。

### BR5：isolation、limits 与 operations

- provider sandbox/egress/profile isolation contract；
- P9 accounting、P7 logs/tail、metrics/readiness；
- overload/browser crash/CDP flood/restart/upgrade/soak；
- deploy/runbook/backup（metadata only）/incident cleanup。

### BR6：qualification

- fixed Wrangler、Puppeteer、Playwright、official SDK subprocess/in-runtime matrix；
- public API JSON/raw/binary/WebSocket differential；
- Cloudflare remote differential 或独立 credential-blocked acceptance；
- P6/reference/capability/deviation/examples/Dashboard 同步。

## 16. 必测矩阵

| case | 预期 |
| --- | --- |
| standard JSONC `browser` | multipart 精确 `{name,type:"browser"}` |
| local-only `remote` | 不进入 Version state |
| provider 未配置/合同不匹配 | upload/API fail closed；无本机 Chrome fallback |
| fixed Puppeteer launch/close | stock workerd 中成功，无 custom package |
| fixed Puppeteer reconnect/sessions | visibility、IDs、errors 与固定 authority 一致 |
| fixed Playwright supported flow | 同一 Browser Fetcher contract 通过 |
| Wrangler create/list/view/close | raw JSON、target、URL、exit code 与 fixed CLI 一致 |
| `lab=true` 未支持 | 明确拒绝，不静默降级 |
| public screenshot/PDF | 正确 media type/bytes/streaming，无 JSON 包装错误 |
| content/markdown/links/a11y | schema、encoding、bounds 与 fixed API 一致 |
| `/json` without AI provider | 明确 unavailable；其他 action 不受影响 |
| cross-account session ID | list/get/connect/close 全拒绝且不泄露存在性 |
| provider URL/credential/raw ID | response/log/error/Worker env 均不可见 |
| WebSocket text/binary/fragment/ping/close | 无破坏转发，bounded queue |
| slow/aborted client | backpressure/cancel 生效，无 leaked connection/session permit |
| provider crash/restart | session `lost`、reconcile/reap 收敛，无错误 reconnect |
| `ocd` restart with live sessions | 按 provider capability认领或关闭；不猜测 endpoint |
| max sessions/pending queue | stable capacity error/Retry-After，无无限排队 |
| huge frame/result/DOM/canvas | 两层 limits 生效，服务保持可用 |
| private network navigation | 严格遵循 operator egress capability 声明 |
| provider config/revision change | contract fencing；旧 session 不透明迁移 |

## 17. Definition of Done

P12 只有同时满足以下条件才可归档：

- `wrangler@4.127.1` 的 config、upload、create/list/view/close 对真实 `ocd` 通过；
- 固定 `@cloudflare/puppeteer` 与声明支持的 `@cloudflare/playwright` API 在 stock workerd 中通过，无 fork/custom client；
- public Browser Run Quick Actions、raw DevTools JSON、binary body 与 CDP WebSocket 按逐 route fixture 通过；
- `ocd` 是唯一公开入口，account/binding/session/target scope 与 provider identity完全隔离；
- external Browser Provider 是 operator prerequisite，不被 open-compute 下载、打包、启动、supervise 或暴露；
- 正式 open-compute 发布仍是单个 `ocd` executable + 既有单个 stock workerd child；
- session lease、overload、browser/provider crash、`ocd` restart、orphan cleanup、contract change 与 soak 通过；
- capacity 全部是 operator config/capability，不复制 Cloudflare plan 或 LynxOS 20 人默认值；
- Miniflare 只作为固定行为/开发参考，production 没有 in-memory/hard-coded/download fallback；
- P7/P9 与 AI provider（仅 `/json`）集成的 supported/planned 状态准确；
- Cloudflare differential 完成，或 credential 限制拆成独立 active acceptance；
- P6、reference、capability manifest、examples、runbook 与 Dashboard 同步。

文档变更本身只运行 `git diff --check`、链接和固定命令/源码核对。实现属于 protocol、runtime、WebSocket、security、
provider、persistence 与 release 变更，必须执行仓库 `AGENTS.md` 要求的 focused tests、coverage 与最终 workspace Gate。
