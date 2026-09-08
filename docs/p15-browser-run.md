# P15：Cloudflare Browser Run 兼容设计

状态：Day 1 合同与分发架构设计完成；待 BR-G0 选择浏览器引擎、实施与验收。

本文细化 [P6 Cloudflare v4 API 与 Wrangler 子集兼容设计](implemented/p6-cloudflare-v4-wrangler-compatibility.md)
中的 `browser` binding、Browser Run API、DevTools session 和固定 Wrangler commands。方案参考现有 workerd 的正式
pin、内嵌压缩 payload、离线物化和子进程监督模型，以及固定 Miniflare Browser Rendering 实现；不把 Miniflare 的开发期
浏览器下载、内存状态或硬编码容量复制到生产。

## 1. 范围与结论

Cloudflare 已把 Browser Rendering 产品名更新为 **Browser Run**，但固定 API/config/binding 中仍使用
`browser-rendering` / `browser`。open-compute 保留这些标准名字，不发明 `browser_run` 配置或 vendor route。

P15 Day 1 目标：

- `wrangler.jsonc` 标准 `browser: { binding }`；
- multipart metadata `{name,type:"browser"}`；
- stock workerd 中能被固定 `@cloudflare/puppeteer` / `@cloudflare/playwright` 使用的 Browser Fetcher binding；
- 固定 Workers types 的 `BrowserRun.fetch()` 与 `BrowserRun.quickAction()`；
- `wrangler browser create/list/view/close`；
- `/client/v4/accounts/{account_id}/browser-rendering/**` 的选定 Quick Actions；
- DevTools session HTTP 与 CDP WebSocket；
- `ocd` 内嵌并监督的 Browser Runtime 的 session allocation、browser execution 与 cleanup。

结论：**Browser Run Day 1 采用与 workerd 相同的单文件分发原则**：正式 `ocd` 内嵌目标平台已固定、已验证的浏览器压缩
payload，首次实际使用 Browser Run 时在 data-dir 排他锁下离线物化，再由 `ocd` 启动和监督浏览器子进程。它是单文件
发行，不是把浏览器链接进 `ocd` 或把浏览器变成同一进程：

1. `ocd` 仍是唯一公开 listener、认证/授权/session-scope authority；
2. 正式 release 仍只有一个原生 `ocd` executable；浏览器 archive、manifest、lock 与许可证随对应目标嵌入，不发布外部
   browser 文件或 sidecar 安装步骤；
3. production 启动和 Browser Run 调用均不下载浏览器、不搜索 PATH，也不接受外部 executable 覆盖；
4. `ocd` 校验 archive、文件集合、browser executable、版本、目标平台和 runtime contract 后原子物化，损坏或不匹配直接
   fail closed；
5. `ocd` 拥有浏览器进程组、readiness、bounded stdout/stderr、session profile、graceful/forced stop、reap、crash recovery
   与孤儿恢复；
6. raw CDP endpoint、内部 token、profile path、process identity 与 engine session ID 永不暴露给 tenant；
7. BR-G0 在 `chrome-headless-shell` 与 Obscura 中选择**一个**正式 Day 1 引擎；release/runtime 不提供动态引擎选择、自动
   fallback 或双实现；
8. 若两个候选都不能满足声明的固定 API 子集，对应 `browser` binding/API 保持 unsupported；不能退回本机任意 Chrome；
9. tenant 不能在 `wrangler.jsonc`、API body 或 Worker call 中指定 endpoint、browser binary、engine 或 launch flags。

外部 Browser Provider 不在 Day 1 范围。未来若有明确的远程执行需求，应另行设计和 qualification，不能预留 provider
registry、endpoint 配置或兼容分支。

## 2. Compatibility authority

实施和 qualification 固定：

- [Cloudflare Browser Run API](https://developers.cloudflare.com/api/resources/browser_rendering/)；
- [Chrome DevTools Protocol](https://developers.cloudflare.com/browser-run/cdp/)；
- [Browser session management](https://developers.cloudflare.com/browser-run/cdp/session-management/)；
- [Wrangler browser commands](https://developers.cloudflare.com/browser-run/reference/wrangler-commands/)；
- [Browser Run limits](https://developers.cloudflare.com/browser-run/limits/)；
- [Cloudflare Puppeteer](https://developers.cloudflare.com/browser-run/puppeteer/)与固定 package；
- [Cloudflare Browser Run changelog](https://developers.cloudflare.com/browser-run/changelog/)中的 standard/full CDP 声明；
- [Live View](https://developers.cloudflare.com/browser-run/features/live-view/)；
- [Browser Run rename changelog](https://developers.cloudflare.com/changelog/post/2026-04-15-br-rename/)；
- [Chromium Headless README](https://chromium.googlesource.com/chromium/src/+/master/headless/README.md)、
  [Chrome Headless Shell](https://developer.chrome.com/docs/automation-and-testing/headless-chrome-shell)与
  [Chrome for Testing asset matrix](https://github.com/GoogleChromeLabs/chrome-for-testing#supported-platforms)；
- [Obscura source](https://github.com/h4ckf0r0day/obscura)、
  [Puppeteer/current limits](https://github.com/h4ckf0r0day/obscura/blob/main/docs/Use-with-Puppeteer.md)、release、license 与
  reproducible build/test evidence；
- `wrangler@4.127.1` config schema、upload builder、`browser-rendering/**` commands/tests；
- 固定 `@cloudflare/puppeteer`、`@cloudflare/playwright`、Workers types 与 Miniflare source snapshot；
- 固定 Cloudflare HTTP/WebSocket trace 和 OpenAPI revision/hash。

网页和 upstream source 用于发现合同；进入 Gate 的 route、query、body、header、raw response、WebSocket frame、close code、
错误与 package call sequence 都必须固定为 fixture。Browser Run 当前仍在快速演进，未进入 inventory 的新功能默认
unsupported。

Cloudflare 当前把默认 Browser Run 描述为 headless Chrome，并声明 standard/full CDP 与完整 Puppeteer API，但 CDP endpoint
仍为 Beta，且官方文档存在 Workers/browser service 约束。P15 不把整份 Chrome CDP schema 自动宣布为支持合同：正式范围
由固定 package 实际 call graph、逐 method inventory、Quick Action inventory 和 Cloudflare differential 共同确定。
若 BR-G0 选择 Obscura，所有相对 Chromium 的 CDP、Web Platform、layout、screenshot、PDF、字体、media、service worker
等差异必须进入公开 capability/deviation matrix；不允许使用“CDP compatible”或“可连接 Puppeteer”代替逐项证据，也不
宣称完整 Chrome/CDP 兼容。

## 3. 三层协议，不混为一个 API

| 层 | 调用方 | 协议 | 是否公开 |
| --- | --- | --- | --- |
| Cloudflare public API | Wrangler、SDK、用户 HTTP client | `/client/v4/.../browser-rendering/**` | 是 |
| Worker Browser binding | `@cloudflare/puppeteer` / Worker | Fetcher + `/v1/**` HTTP/WebSocket | 只对已绑定 Worker 可见 |
| Embedded Browser Runtime | `ocd` | local process lifecycle + raw CDP | 否 |

Public API 中 JSON route 是否使用 v4 envelope 必须逐 route 固定。尤其固定 Wrangler 的 DevTools helper 明确把
Browser Run DevTools 当作 **raw JSON**，不能由 P6 的通用 `fetchResult()`/v4 envelope middleware 包装。image/pdf/body 与
WebSocket 同样保持原始媒体类型/upgrade。

Worker binding 的 `/v1/**` 是固定 Cloudflare packages/Miniflare 可观察到的 service contract，但它不是 tenant 可直接
访问的 public management endpoint。Browser Runtime 只提供 `ocd` 内部的 process/CDP 能力，不能因此绕过 `ocd` 的
account、binding 和 session scope。

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
- 当前目标没有已验证的内嵌 Browser Runtime，或 P15 capability 未通过时，upload fail closed，不能删除 binding 后继续部署。

### 4.2 Multipart metadata

固定 Wrangler 生成：

```json
{
  "bindings": [
    { "name": "BROWSER", "type": "browser" }
  ]
}
```

descriptor 是 immutable Version state。engine name/revision、archive digest、launch policy 和 session limits 是平台 release/
operator runtime authority，不写进 tenant Version；但每次 session 会记录 secret-free runtime contract digest，用于
release/config 变化后的 fencing/reconciliation。

## 5. Worker binding contract

Miniflare 把 `browser` binding 组装成 service binding，固定 Cloudflare packages 把它当 Fetcher 使用。open-compute
沿用相同边界：

```text
tenant Worker + fixed Workers types / @cloudflare/puppeteer
  -> env.BROWSER.fetch() / env.BROWSER.quickAction() / WebSocket
  -> packages/runtime BrowserTransport
  -> ocd BrowserService
  -> supervised embedded Browser Runtime
```

不需要修改 stock workerd，也不在 tenant isolate 注入 Node/Chrome process handle。runtime facade 精确提供
`BrowserRun.fetch(input, init)` 和九个固定 `quickAction(action, options)` overload；后者转换为对应 Browser Run action
route 并原样返回标准 `Response`，不返回自定义 object。底层 `BrowserTransport` 只携带：

```text
account_id, script_id, version_id, deployment_id,
binding_name, descriptor_sha256, capability_version
```

它不携带 engine endpoint/token/raw session ID。每次 fetch 都重新验证 immutable deployment snapshot 与 binding identity，
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
open-compute account partition 或在 agent/app 层不共享 session ID，而不是改变标准 Browser binding 返回值。

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
`devtoolsFrontendUrl` 必须指向 deployment-owned Live View/DevTools proxy，不能返回内部 browser endpoint 或 Cloudflare
`live.browser.run` origin。

`--lab`/WebMCP 是实验能力。Day 1 route 识别后明确拒绝 `lab=true`，除非固定 Browser Runtime、security review 与官方
differential 已单独通过；不能忽略该 flag 启动普通 browser。

### 6.2 Quick Actions

Day 1 public subset 按固定 OpenAPI 实现：

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
engine capability 与错误。二进制 screenshot/PDF 直接 stream；HTML/text/JSON 是否套 envelope 不从其他 route 推断。

`crawl`、recording、WebMCP/lab、human-in-the-loop、完整 browser persistence/profile upload、任意 extension、custom executable
和未固定的新 beta endpoint 不在 Day 1。route/field 存在但未支持时返回明确 Cloudflare-style failure，不能忽略参数执行
一个语义更弱的 action。

`/json` 需要模型时复用 AI Search 的 operator-owned model catalog、secret reference、bounded request/response、timeout 和
stable AI provider error classes。tenant 不能在 Browser request 中提供 AI endpoint/key。模型不可用时 `/json` fail closed，
不影响不需要模型的 screenshot/content 等 operation。

## 7. Embedded Browser Runtime

### 7.1 单文件分发与离线物化

Browser Runtime 复制 workerd 已验证的分发原则，不复制它的具体目录或 lock：

- `packages/runtime/browser.lock.json` 是浏览器正式 pin 的权威路径；BR1 创建该文件，不得复用或扩写
  `workerd.lock.json`；
- 每个正式目标只嵌入该目标的一个浏览器压缩 archive、manifest 与所需许可证；
- build 先验证 source revision/release identity、archive SHA-256、文件集合、各 executable/resource SHA-256、version output、
  目标 OS/CPU、启动 flags、CDP revision/capability manifest 和 license inventory，再把同一批字节编入 `ocd`；
- production 不下载、不搜索 PATH、不接受 runtime executable/archive/config override；
- 普通只读命令和未使用 Browser Run 的启动不物化浏览器；第一次 Browser Run admission 在 data-dir 排他锁下把 archive
  解压到私有 staging，逐文件复验后 fsync、原子发布到 content-addressed browser cache；
- 已存在但缺文件、额外文件、摘要/权限/owner/symlink/version 不匹配的 cache 视为损坏并拒绝，不能静默重建覆盖；
- release 仍是单个 `ocd` executable；“单文件”不表示单进程，browser/renderer 子进程和资源目录只存在于 `ocd` 独占的
  data-dir 与进程树内。

浏览器 archive 与 workerd archive 独立 pin、独立验证、独立物化。即使两个引擎都使用 V8，也不共享 V8 library、ICU、
snapshot、heap 或 ABI；不得为了节省体积把 workerd 与 browser 的源码/升级周期绑定。

### 7.2 BR-G0 引擎二选一

BR-G0 只比较以下两个候选，结束时选择一个正式 Day 1 引擎：

| 维度 | `chrome-headless-shell` | Obscura |
| --- | --- | --- |
| 来源 | Chromium/Chrome for Testing 对应 source revision 或可复现自建产物 | 固定 upstream release/source revision 的 Apache-2.0 产物 |
| CDP | 原生 Chromium CDP，最接近 Cloudflare 默认 headless Chrome | 明确子集；逐 method qualification |
| 渲染 | Blink/Skia/PDFium，screenshot/PDF/layout 兼容性最高 | 独立 layout/paint；长尾 CSS、字体、media、service worker、PDF 等有差异 |
| 分发 | 多文件 runtime archive；Chrome 126 官方无 Linux ARM64 asset | 正式 Linux/macOS x64/ARM64 archive 可用 |
| 隔离 | Chromium 多进程 sandbox 与独立 profile | 每 session 独立 process；不能依赖同进程 page/context 作为 tenant 边界 |
| 体积基线 | Chrome 126：约 79.4 MiB macOS ARM64 / 89.8 MiB Linux x64 压缩 | 0.2.2 render archive：约 72--78 MiB macOS / 77--79 MiB Linux |

表中数字只用于设计预算，不是 formal pin。正式选择必须以仓库自行验证的目标产物、精确 bytes、license 和 Gate 报告为准。
调研基线读取于 2026-09-08：[Chrome for Testing known-good assets](https://googlechromelabs.github.io/chrome-for-testing/known-good-versions-with-downloads.json)、
[Obscura v0.2.2](https://github.com/h4ckf0r0day/obscura/releases/tag/v0.2.2)。Chrome 126 archive 解压后约为
163.3 MiB（macOS ARM64）/189.8 MiB（Linux x64）；内嵌正式压缩 payload 时 `ocd` 对应增加约 79.4/89.8 MiB，另有
manifest/linker alignment 的小量开销。Linux ARM64 必须以实际自建产物重新测量，不能沿用估算作为 release bound。
Linux ARM64 `chrome-headless-shell` 若无与固定版本匹配的官方资产，只能使用可复现源码构建并完成与其他正式目标相同的
供应链、sandbox、CDP 和产品 Gate；不能使用发行版 Chromium、非正式下载或另一 revision 冒充。

选择规则：

- 若 `chrome-headless-shell` 在三个正式目标通过固定 Puppeteer/Playwright、Quick Actions、CDP、sandbox/egress 和 lifecycle
  Gate，优先选择它；
- Obscura 只有在声明的 Day 1 method/action inventory 全部通过时才能被选中；所有相对 Chromium/Cloudflare 的差异进入
  capability/deviation matrix，unsupported method 必须明确失败；
- 不用二进制大小替代安全与行为证据，不因某个候选在单一路径失败而运行时 fallback 到另一个；
- G0 完成后从 active design 删除未选候选的生产 wiring/config/schema/test branch，只保留调查证据与选择结论。

### 7.3 `ocd` 生命周期所有权

`ocd` 负责：

- v4/account/binding auth、request/schema validation、capacity admission、session lease、public ID 和 public error contract；
- browser archive/cache verification、materialization、readiness 与 runtime generation；
- 启动固定 executable 与 flags，创建独立 session profile/context 和受控 CDP listener/token；
- process group、bounded stdout/stderr、graceful stop、forced stop、reap、restart backoff、crash/orphan recovery；
- HTTP/WebSocket proxy 的 backpressure、deadline、redaction、target fencing 与 observability；
- sandbox、network/egress、CPU/memory/disk/process/profile limits；
- 防止 raw endpoint/token、process/profile identity 和 engine detail 到达 tenant。

browser child 不能反向成为 authority。session、account、binding、capacity、keep-alive 与 close reason 以 SQLite/`ocd` 为准；
browser/CDP memory 只是当前 generation 的执行状态。

### 7.4 Operator config

operator 只配置部署容量和 deadline，不选择 engine、binary、endpoint、revision、archive 或 launch flags：

```text
[browser]
max_sessions = <required-positive-integer>
max_pending_acquires = <required-positive-integer>
acquire_timeout_ms = <required-positive-integer>
command_timeout_ms = <required-positive-integer>
idle_reap_interval_ms = <required-positive-integer>
```

真正 schema 使用 `deny_unknown_fields` 和非零/显式 required fields。全部 capacity 是 operator deployment config，不写入
`wrangler.jsonc`，也不提供 open-compute 或 LynxOS 的隐含默认值。process startup 在不访问网络、不物化 browser 的情况下
完成静态 validation；Browser Run capability/readiness 在首次物化和每次 runtime generation readiness 后确定。

### 7.5 Frozen runtime contract

build/runtime 解析出不含 secret 的 `ResolvedBrowserRuntimeContract`：

```text
engine_name, engine_version, source_revision
target, archive_sha256, executable_sha256, file_manifest_sha256
version_output, launch_policy_sha256, sandbox_policy_sha256
supported binding-protocol revision
supported quick-actions set
supported CDP protocol/method inventory
license_manifest_sha256
contract_sha256
```

session 创建时保存 contract digest 与 runtime generation。release/config 变化后旧 session 不允许透明切到另一个 engine、
revision、binary、flags 或 sandbox policy；它只能在原 generation 中继续、明确关闭，或标为 `lost`。raw endpoint/token、路径、
process identity 和 secret value 不进入普通 API、Debug、日志或 metrics。

### 7.6 Runtime driver 最小化

BrowserService 实现固定 Cloudflare/Miniflare Browser binding `/v1/**`、Quick Actions 和 public DevTools route；引擎侧只使用
本机 process lifecycle、CDP HTTP/WebSocket 和选中引擎确有证据的能力。允许一个窄的内部 runtime driver 边界隔离启动、
readiness 和 CDP endpoint 解析，但 Day 1 不实现 provider registry、通用 plugin API、remote endpoint、credential 或运行时
engine switching。handlers 中不得散落候选专用 URL/JSON/错误分支；BR-G0 选择后只有一个权威 driver。

## 8. Session model、lease 与 reconciliation

SQLite 建议 authority：

```text
browser_sessions
  id, account_id, visibility_scope,
  runtime_contract_sha256, runtime_generation,
  engine_session_locator_ciphertext, process_identity_sha256,
  state, keep_alive_ms, created_at, connected_at,
  last_activity_at, closing_at, closed_at, lost_at,
  lease_generation, close_reason
```

public session ID 是 open-compute opaque ID；engine session locator、CDP endpoint/token、profile path 和 process identity 不返回。
其中任何足以连接或控制 browser 的值都按 sensitive value 加密/受保护存储；日志只使用 public ID 或 digest。

状态机：

```text
acquiring -> ready -> connected -> ready
ready/connected -> closing -> closed
acquiring/ready/connected/closing -> lost
```

规则：

- admission permit 从 acquire 开始持有到 closed/lost，不是一次 HTTP request 的短 semaphore；
- pending acquire 有独立 bounded queue 和 deadline；
- public ID 只有在 browser process/session readiness 和 SQLite 登记成功后可用；失败要终止并回收 process/profile orphan；
- reconnect 不刷新超出标准 keep-alive 的永久 lease；具体 idle semantics 由固定 Cloudflare trace 确定；
- close 幂等行为、unknown/closing session response 按固定 route qualification；
- `ocd` restart 使旧 runtime generation/token 失效；旧 generation 的 active session 标为 `lost`，不能猜测或复用旧 CDP
  endpoint；
- restart/orphan recovery 只有在 start identity、browser executable digest、process ancestry 和 runtime contract 全部验证后才能
  signal 进程组；可识别 orphan 必须终止并 reap，身份不确定时 fail closed 并输出不含 secret 的运维诊断；
- runtime cache 可跨 restart 复用，但每次都复验 manifest/关键摘要；browser session/profile/runtime generation 不跨 daemon
  restart 透明恢复。

## 9. HTTP/CDP/WebSocket proxy

所有 public/binding traffic 经 `ocd`：

- HTTP request/response streaming 有 size/deadline/backpressure；
- WebSocket upgrade 前完成 account/binding/session/target authorization；
- `Origin`、Authorization、Cookie、forwarded headers 与 runtime-internal token 分开处理；
- browser WebSocket URL/token 只在进程内构造，redirect/response body 不能把它泄露给 client；
- text/binary frame、fragmentation、ping/pong、close code/reason 与 half-close 按固定 CDP behavior 转发；
- per-connection message/frame/aggregate bytes 和 outbound queue 有 operator guard；
- client disconnect 释放 connection lease，但是否关闭 browser session 取决于标准 session contract；
- proxy 不能解析并重写正常 CDP messages；只在 admission/header/session fencing 边界检查；
- legacy `/v1/connectDevtools` 的 length-prefix framing 与 native page WebSocket 分开测试。

`wrangler browser view` 所需 `devtoolsFrontendUrl` 指向 `ocd` 自带的静态 DevTools frontend/proxy route 或可验证的
deployment-owned frontend。P15 不在启动时从公网下载 DevTools UI。若不能合法、可复现地随 release 提供兼容 frontend，
`view` Gate 不通过，不能只返回 browser internal URL。

## 10. Quick Action 执行

Quick Action 采用统一 pipeline：

```text
authenticate -> validate fixed schema -> capacity admission
  -> acquire/reuse isolated engine session
  -> navigate/action under deadline
  -> bounded/streamed result validation
  -> close/release according to official contract
```

约束：

- URL、redirect、subresource、download、WebSocket 和 DNS 都发生在受控 Browser Runtime egress 边界；
- `ocd` 只把 validated action 发送到正式固定的 engine session，绝不直接 fetch tenant URL 代替浏览器；
- response body、DOM、screenshot/PDF、AI prompt/result 默认不进入 logs；
- screenshot dimensions/format、PDF options、selectors、wait conditions、headers/cookies、navigation timeout 逐字段 allowlist；
- unsupported field fail closed，不把 `waitForSelector` 等参数静默丢掉；
- output 不自动写入 R2/Artifacts/团队目录。Worker/调用方要持久化时显式调用对应 binding，保持权限和失败边界清晰；
- `/json` 的 AI call 与 browser session 共用 end-to-end deadline，不能无限等待 engine；
- engine quick-action capability 不足时对应 route `unsupported`，不通过执行自定义脚本模拟半套 semantics。

## 11. Isolation 与 security

Browser Run 执行不可信网页，内嵌 payload 不降低它作为高风险解析器和主动网络客户端的边界。最低 invariant：

- 每个 session 使用独立 browser process/profile；只有通过 G0 证明具有等价强隔离时才可复用 browser process/context；
  cookie/cache/storage/profile 不跨 scope 复用；
- browser process 只能读取只读 browser runtime cache 和分配给该 session 的 profile/temp subtree，不能读取 `ocd` control
  SQLite、master key、secret files、control socket、artifact cache 或 workerd runtime files；
- launch flags、environment 和 executable 由 formal runtime contract 固定；tenant/operator config 都不能传 `--no-sandbox`、
  extension、proxy、user-data-dir、remote-debugging address 或任意 V8/browser flags；
- production 禁止以 `--no-sandbox` 或等价选项回退；目标宿主无法建立已验证 sandbox 时 Browser Run unavailable；
- browser/CDP endpoint 只监听 loopback/继承的私有 FD，并使用每 generation 随机内部 token；tenant 无法直连；
- account/session/target auth 在每次 HTTP 和 WebSocket upgrade 执行，不能只在 create 时检查；
- session ID 高熵且不可枚举；list/get/close 的 not-found/forbidden 不泄露跨 account presence；
- URL、redirect、DNS、subresource、WebSocket 和 download 的 address-level egress 必须拒绝 private、loopback、link-local、
  metadata、Unix、IPv4-mapped private IPv6、DNS-to-private 和所有 platform listener；不能只依赖 hostname 检查、browser flag
  或候选引擎声称的 SSRF protection；无法在目标平台强制执行时 Browser Run unavailable；
- downloads、file chooser、clipboard、camera/mic、printing、WebUSB/WebBluetooth 与本地 filesystem 默认禁用；
- browser crash、renderer hang、CDP flood、zip bomb/download、巨大 DOM/canvas 都受 engine process/`ocd` 双层 limit；
- P7 logs 只记录 stable metadata/error class，清洗 URL query、headers、cookies、DOM、CDP payload 和 screenshots。

## 12. Limits 与 backpressure

不复制 Cloudflare plan 的并发/session/browser-minutes 数值，也不设置 LynxOS 20 人默认值。operator capacity 至少包括：

- active sessions、pending acquires、sessions per account；
- acquire/command/navigation/idle/maximum lifetime deadline；
- concurrent Quick Actions 和 CDP connections；
- HTTP body、result、WebSocket frame/message/queue bytes；
- screenshot/PDF dimensions/bytes、DOM/text/JSON output；
- engine request in-flight、spawn/close/reap concurrency；
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

`wrangler dev` 的本地体验继续由上游 Wrangler/Miniflare 负责；P15 qualification 针对真实单文件 `ocd` + stock workerd +
正式内嵌并物化的 Browser Runtime。

## 14. Error 与 observability contract

稳定 Browser Runtime error classes：

```text
invalid_request, unsupported, capacity_exhausted, acquire_timeout,
runtime_corrupt, runtime_unavailable, browser_launch_failed, browser_crashed,
session_not_found, session_lost, target_not_found,
cdp_unavailable, navigation_failed, action_timeout, malformed_response
```

Public code/message/status 由固定 Cloudflare route fixture mapping；内部 class 不直接作为 vendor error body。retryability、
`Retry-After` 与 close code 有明确表，不能根据 browser stderr/message regex 猜测。

推荐 metrics/log dimensions：

```text
account_id, script_id, operation, engine_name,
runtime_contract_sha256, runtime_generation, result_class, session_state,
queue_wait_ms, acquire_ms, duration_ms, bytes_in, bytes_out
```

session/target ID 只记录 bounded/digest form；URL host 仅在 operator 明确允许的低基数审计日志中出现，不作默认 metrics label。
P7 realtime tail 可以显示 Worker 触发的 browser call outcome，但不能带 DOM、CDP message、cookie、header、AI content 或
engine/process/internal endpoint detail。

## 15. 实施顺序

### BR0：冻结合同

- 固定 Wrangler schema/commands/upload metadata；
- 固定 Puppeteer/Playwright/types package versions、integrity 与 Browser binding call graph；
- 固定 public OpenAPI、raw DevTools/WebSocket traces、Quick Action schemas；
- 建 route/field/media/frame/error/capability inventory；
- 记录 Browser Rendering -> Browser Run 只改产品名、不改兼容 path 的规则。

### BR-G0：end-to-end feasibility Gate

- 分别以固定、显式准备的 `chrome-headless-shell` 与 Obscura 候选运行同一套 inventory，不在测试过程中隐式下载 runtime；
- 真实 `ocd` + stock workerd 中，固定 `@cloudflare/puppeteer` launch/connect/newPage/navigate/evaluate/screenshot/PDF/close
  和声明支持的 Playwright flow 通过；
- Quick Actions、CDP method、DOM/layout/screenshot/PDF/font/network/session 行为与 Cloudflare/Chromium baseline 做 differential，
  每个差异明确为 blocker、supported deviation 或 unsupported；
- HTTP/CDP/WebSocket 代理在 backpressure、disconnect、browser crash/hang 后正确收敛；
- 固定 Wrangler create/list/view/close 的 raw route/URL shape 可实现；
- 每个候选在 Darwin ARM64、Linux GNU ARM64/x64 都有可验证、可复现的 archive/build；
- 验证压缩 payload 内嵌、只在首次使用时离线物化、cache corruption 拒绝、无 PATH/runtime download/external sidecar；
- browser/DevTools frontend revision、license、sandbox/egress、发行体积和跨平台构建可复现；
- 根据第 7.2 节规则选择一个引擎并冻结 runtime contract；不通过的候选不进入生产 wiring。

Exit：只有选中引擎的声明 API 子集、正式目标、安全边界和单文件分发同时通过才进入 BR1。若两个候选都失败，`browser`
binding 和 Browser Run routes 保持 unsupported；不能只做 Quick Actions 后宣称完整 binding，也不能宣称完整 Chrome/CDP。

### BR1：runtime pin、build、materialization 与 supervisor

- 建立唯一 browser lock、target archives、source/build provenance、checksums、license inventory 和 capability manifest；
- build-time verification 与嵌入、data-dir staging/atomic materialization/cache reuse/corruption rejection；
- browser process group、readiness、internal token、profile、bounded stdio、stop/reap、crash/backoff 与 orphan recovery；
- BrowserConfig capacity/deadline、runtime contract digest、health/readiness 与稳定错误类。

### BR2：session authority 与 runtime binding

- session schema/state/lease/admission、restart `lost`、orphan reap 与 contract fencing；
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

- browser sandbox/egress/profile/process isolation contract；
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
| unsupported target/runtime contract mismatch | upload/API fail closed；无 PATH、外部 provider 或本机 Chrome fallback |
| embedded archive identity | archive/file/executable/version/target/license manifest 全部匹配 formal lock |
| first Browser Run use | 在 data-dir lock 下离线原子物化；未调用 browser 时不物化 |
| corrupt/partial runtime cache | 明确拒绝，不执行、不覆盖损坏证据、不隐式下载 |
| fixed Puppeteer launch/close | stock workerd 中成功，无 custom package |
| fixed Puppeteer reconnect/sessions | visibility、IDs、errors 与固定 authority 一致 |
| fixed Playwright supported flow | 同一 Browser Fetcher contract 通过 |
| Wrangler create/list/view/close | raw JSON、target、URL、exit code 与 fixed CLI 一致 |
| `lab=true` 未支持 | 明确拒绝，不静默降级 |
| public screenshot/PDF | 正确 media type/bytes/streaming，无 JSON 包装错误 |
| content/markdown/links/a11y | schema、encoding、bounds 与 fixed API 一致 |
| `/json` without AI provider | 明确 unavailable；其他 action 不受影响 |
| cross-account session ID | list/get/connect/close 全拒绝且不泄露存在性 |
| raw CDP URL/token/profile/process ID | response/log/error/Worker env 均不可见 |
| WebSocket text/binary/fragment/ping/close | 无破坏转发，bounded queue |
| slow/aborted client | backpressure/cancel 生效，无 leaked connection/session permit |
| browser crash/hang | session `lost`、process/profile/reap 收敛，无错误 reconnect |
| `ocd` restart with live sessions | 旧 generation/token 失效，session `lost`；验证后清理 orphan，不猜测 endpoint |
| max sessions/pending queue | stable capacity error/Retry-After，无无限排队 |
| huge frame/result/DOM/canvas | 两层 limits 生效，服务保持可用 |
| private/network metadata navigation | 顶层/redirect/DNS/subresource/WebSocket/download 均由 address-level egress 拒绝 |
| production sandbox unavailable | Browser Run unavailable；不增加 `--no-sandbox` fallback |
| runtime revision/policy change | contract fencing；旧 session 不透明迁移 |
| non-selected candidate | production binary/config/schema/runtime path 中均不存在 |

## 17. Definition of Done

P15 只有同时满足以下条件才可归档：

- `wrangler@4.127.1` 的 config、upload、create/list/view/close 对真实 `ocd` 通过；
- 固定 `@cloudflare/puppeteer` 与声明支持的 `@cloudflare/playwright` API 在 stock workerd 中通过，无 fork/custom client；
- public Browser Run Quick Actions、raw DevTools JSON、binary body 与 CDP WebSocket 按逐 route fixture 通过；
- `ocd` 是唯一公开入口，account/binding/session/target scope 与 browser process/CDP identity 完全隔离；
- BR-G0 已在 `chrome-headless-shell` 与 Obscura 中选择一个引擎，未选引擎没有生产 wiring 或 fallback；
- 正式目标的 browser source/build/archive/file/version/license/capability identity 由唯一 formal lock 固定并复验；
- 正式 open-compute 发布仍是单个 `ocd` executable，内含独立压缩的 workerd 与 browser payload；没有外部 Browser Provider、
  外部 runtime 文件、PATH discovery、startup download 或 operator executable override；
- browser 只在首次 Browser Run 使用时离线物化到 `ocd` 独占 data-dir，并由 `ocd` 完整监督其进程组和 profile 生命周期；
- session lease、overload、browser crash/hang、`ocd` restart、orphan cleanup、contract change 与 soak 通过；
- production sandbox 与 public-only address-level egress 在全部正式目标强制执行，无 `--no-sandbox` 或 hostname-only fallback；
- capacity 全部是 operator config/capability，不复制 Cloudflare plan 或 LynxOS 20 人默认值；
- Miniflare 只作为固定行为/开发参考，production 没有 in-memory/hard-coded/download fallback；
- P7/P9 与 AI provider（仅 `/json`）集成的 supported/planned 状态准确；
- Cloudflare differential 完成，或 credential 限制拆成独立 active acceptance；
- P6、reference、capability manifest、examples、runbook 与 Dashboard 同步。

文档变更本身只运行 `git diff --check`、链接和固定命令/源码核对。实现属于 protocol、runtime、process、WebSocket、security、
persistence 与 release 变更，必须执行仓库 `AGENTS.md` 要求的 focused tests、coverage 与最终 workspace Gate。
