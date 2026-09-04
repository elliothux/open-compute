# P3.1：Static Assets 与框架产物导入

状态（2026-09-01）：**Day1 核心实现与本地最终验收完成，设计已归档；Assets 直接 Cloudflare
differential 由[独立验收计划](../acceptance/p3-assets-service-bindings-acceptance.md)继续追踪**。
当前维护 Gate 已覆盖声明的本地产品矩阵；Static Assets 已映射到 P3.4 固定 catalog、capability、
deviation 和 contract report。共享 runner 已在真实 Cloudflare 上完成一项 Cache API fixture，但该
结果不覆盖 Assets routing/binding，因此不能给扩展目标的 hosted verdict。后续固定 vinext workload
已经取得 Application Go，证明选定应用的 Assets/browser 路径；它仍不替代完整 Assets contract 的
direct differential。

本阶段把静态文件纳入现有不可变 deployment：框架构建产物经过导入、校验和上传后，代码、
资源 manifest、路由配置一起 ready、promote、rollback。请求仍由一个 `platformd` 和一个
受监督的 stock workerd 处理；不增加静态服务器、Node SSR 进程、Redis 或第二套 S3 配置。

本文细化[总方案](open-compute-workerd-platform.md)的 P3.1，向
[Service Binding 方案](p3-2-service-bindings.md)提供统一的默认 HTTP 路由。P3.2 的 Node API
适配、P3.3 的通用 Cache/Images 和 P3.4 的 Cloudflare conformance 仍是独立工作，不以本阶段通过代替。

## 1. 基线与进入条件

### 1.1 已有基础与本次新增

以下是当前工作树的源码观察，不是本次重新运行的验收结论。

| 已有基础 | 本阶段改动 |
| --- | --- |
| `packages/toolchain/src/build-worker.ts`：普通 Worker 的 TS 检查、Rolldown 打包 | 新增框架已构建产物 importer；不把 vinext/Vite 多环境构建改成单入口打包 |
| `packages/toolchain/src/deploy-worker.ts`：bundle body + 有界 metadata header | 增加资源上传会话和引用；不把全量 manifest 塞入 header |
| `crates/workers/src/pipeline.rs`：staging/validation/ready 和产品 promotion | 增加 assets、static-only，继续使用同一个部署状态机 |
| `crates/artifacts`：S3 ArtifactStore、校验及本地缓存 | 复用上传、对象身份与 provider；增加 manifest/资源读取及回收引用 |
| `packages/runtime/src/loader/host.ts`：每次 resolve 后再 WorkerLoader get | 在默认 fetch 前接资源路由，命名入口和非 HTTP 事件不受影响 |
| `crates/service/src/workers_http.rs`：public route 和响应 body pin | 扩展为 deployment dispatch；核实执行、后台任务与 body 的完整存活期 |

遵循已验收的 [Day1 约束](day1-architecture-cleanup.md)：直接修改当前 schema、descriptor、工具链和
测试；不增加旧 open-compute deployment/manifest 双读、V1/V2 引擎或历史升级适配。
当前源码仍有历史命名不意味着新增实现应复制这些模式。平台 SQL 按当前模型整理，并同步
校验和、装配和测试；不自动重置使用者的数据目录。

### 1.2 平台契约与应用样本

平台验收首先固定 Cloudflare Static Assets 官方配置/HTTP 契约、workers-sdk asset/router worker
revision、正式 workerd pin、compatibility date/flags 与 portable fixtures；具体 catalog 和双端 adapter
由后续 P3.4 统一拥有。不能从一个框架产物能否运行反推 Static Assets
支持范围。

应用 qualification 可以沿用总方案已勘察的 vinext 源码基线
[`5d0b53088c689b75d63672eab6ff66434afa5b3b`](https://github.com/cloudflare/vinext/tree/5d0b53088c689b75d63672eab6ff66434afa5b3b)：
`vinext 1.0.0-beta.8`、`@vinext/cloudflare 1.0.0-beta.6`。它只是可选应用样本，不是平台
规范。开始该应用验收前，必须另行补齐包锁、React/Vite/RSC 插件、浏览器、构建工具、用例清单
及 open-compute revision 的完整输入元组，不能声称已经安装或运行。

正式运行时使用 [workerd lock](../../packages/runtime/workerd.lock.json)；当前为
`v1.20260826.1`。本阶段不隐式升级或下载运行时。接入点按
[runtime 布局](runtime-and-test-layout.md)与[测试规范](../references/testing.md)执行；布局实测
[本机完整验收与实测](runtime-and-test-layout-results.md)，不将局部结果扩写为全部通过。

## 2. 交付范围

| 能力 | 本阶段契约 |
| --- | --- |
| 部署形态 | Worker-only、Worker + Assets、Assets-only；后者不生成伪造租户 Worker |
| 产物 | 原始文件字节、URL 路径、SHA-256、长度、Content-Type；server/client 图隔离 |
| 配置 | `assets.directory`、可选 `binding`、`run_worker_first`、`html_handling`、`not_found_handling` |
| Binding | 声明的 `env.<binding>.fetch(input, init?)`；不要求名字必须是 `ASSETS` |
| HTTP | GET/HEAD、正确 MIME、长度、ETag/304、HTML 路径规范化、缺失响应 |
| 路由规则 | Worker-first 布尔值/路径规则、404/SPA、`_headers`、`_redirects` |
| 一致性 | 一次请求固定一个 deployment；代码和资源不混用；中断发布不改变 active |
| 运维 | 配额、故障分类、上传恢复、GC、备份引用和实际 S3 provider 校验 |

不复制 Cloudflare 的全球 CDN、边缘缓存层、计费、Smart Placement、Pages Functions 或完整
Wrangler 管理 API。资源缓存不是 `caches.default`，也不是 vinext 的 Workers Cache
`ctx.cache`/tag purge；本阶段不借静态资源缓存冒充 ISR/PPR 数据缓存。

Range、预压缩变体、图片转换没有因为“静态服务器通常有”就自动成为承诺。当前参考
asset-worker handler 不提供一个可直接照搬的完整 Range 实现：首版可以按该基线忽略 Range
返回完整 200，不能伪造 206/416 或 `Accept-Ranges: bytes`。若 contract catalog 把这些能力标成
supported，应补充实现与对应矩阵后才算相关用例通过，不能改断言绕过。

## 3. 组件与请求路径

```text
框架构建 → 产物导入 → 上传会话 → 校验/ready → promote
                                  │
                         control.sqlite 引用
                                  │
                          S3 不可变对象

public route / 默认 SERVICE.fetch
  → resolve + pin deployment
  → DefaultHttpRouter ──资源──→ AssetHandler → 私有字节读取 → platformd/S3
                      └─动态──→ WorkerLoader → tenant Worker
                                                   │
                                          ASSETS.fetch → AssetHandler
```

这张图表示逻辑调用，不增加进程。`DefaultHttpRouter`、`AssetHandler` 是受信任 system
Worker 中的 TS 模块；Rust 负责 authority、对象读取、配额和存活引用。静态请求仍经过受信任
的 workerd 路由，但没有资产未命中以外的理由去初始化租户 isolate。

### 3.1 两个内部接口

```ts
routeDefaultHttp(context: PinnedDeploymentContext, request: Request): Promise<Response>;
fetchDeploymentAsset(context: AuthorizedAssetsContext, request: Request): Promise<Response>;
```

这里的 context 是内部能力，不是可由租户构造的 DTO。第一个接口可以选择用户 Worker；
第二个只处理资源，绝不调用 tenant Worker。公共流量、默认 service fetch 复用第一个；
`ASSETS.fetch()` 使用第二个。命名入口、默认 RPC、Queue/Cron/Workflow/DO 事件不经过
`routeDefaultHttp`。

路由实现参考 workers-sdk 的 router-worker 与 asset-worker：抽取路径匹配、响应和规则解析，
保留来源/许可证/固定 revision；不照搬生产 analytics、Sentry、计费、限流套餐、图片端点的
专用分支。也不依赖运行时从 `references/` 读取文件。导入代码进入正常 TS 构建、严格检查和
单文件嵌入清单；不提交或手工改 `packages/runtime/dist/`。

### 3.2 Deployment 类型

领域模型使用明确的联合类型：

```ts
type DeploymentContent =
  | { kind: "worker"; bundle: BundleRef; assets?: AssetsRef }
  | { kind: "assets-only"; assets: AssetsRef };
```

SQL 同步约束：Worker 型必须有完整 code artifact/main module；Assets-only 必须有 assets，
code 字段为空。修改当前 code 必填模型，不用空 JS、空模块名或假 bundle 摘要表示静态站点。
Assets-only 拒绝用户 entrypoint、Queue consumer、Cron handler 等需要执行代码的声明，
也拒绝没有执行主体的 env/secrets 和要求进入 Worker 的 `run_worker_first` 配置；可以拥有
普通 Worker 路由和作为默认 service fetch 目标。其 RPC/命名入口不被视为可调用。

## 4. 构建与配置输入

### 4.1 两条构建路径、一个部署模型

普通 TS Worker 继续使用现有编译器。vinext 使用固定的 vinext/Vite Cloudflare 构建，再由
importer 读取实际生成的入口、模块图、client/public 输出、路由和 bindings。入口文件位置
取自构建输出描述，不猜测所有应用都叫 `dist/worker.js`，也不以 `next build` 替代 vinext。

importer 只做平台适配，不重写业务逻辑或压平 RSC/SSR/client 环境。保留模块 specifier、
动态 import、hash chunk 文件名、CSS/font 引用和 source/client 边界。Wasm/text/data 等非
ES-module 输出按当前 workerd 可接受的模块类型导入；未支持类型明确报错，不能把文件悄悄
丢进 public 目录或回退成 runtime 网络 import。

拟新增配置示例，字段不是当前 CLI 已支持的声明：

```json
{
  "assets": {
    "directory": "./build/client",
    "binding": "ASSETS",
    "run_worker_first": ["/api/*", "!/api/docs/*"],
    "html_handling": "auto-trailing-slash",
    "not_found_handling": "none"
  }
}
```

`directory` 仅供本机构建工具扫描，不进入服务器文件路径解析。`binding` 未配置就不创建
env 项；仍可自动提供资源。绑定名与 vars、secrets、KV/D1/R2/DO/Queue/Workflow/service
共享一个命名空间，重复一律拒绝。规范化后的默认配置进入 descriptor，而不是请求时猜默认值。

### 4.2 安全扫描与 manifest

只扫描显式资产输出根目录。逐文件以打开的文件句柄读取并校验类型，拒绝符号链接逃逸、
特殊文件、目录遍历、NUL/控制字符和规范化后重复 URL。不能仅在扫描前检查 realpath 后又
无保护地按名字打开，给构建中途换链接留下窗口。扫描与上传之间文件变化必须发现并重新
生成 manifest，不能继续沿用旧 digest。

支持 `.assetsignore` 的固定解析规则；`_headers`、`_redirects` 作为配置读取，不作为公开
资源发布；`.assetsignore` 本身也不公开。拒绝把项目根、server 输出或 `.git`/`.env*`/凭据
文件作为发布结果；报错说明命中的规则。允许框架的合法 `.well-known/`，不能简单禁止全部
点文件。client sourcemap 是否发布由显式配置决定，server sourcemap 永不自动进入资源集。

manifest 是一份 canonical UTF-8 JSON，对路径排序并规定 JSON 编码；每项保存逻辑路径、
digest、精确 byte size、确定的 MIME。MIME 由固定映射/显式输出元数据决定，不依赖 S3
返回的 Content-Type，不通过文件正文嗅探把 JS 当 HTML。原始路径中的 `%` 保持文字含义，
不在上传阶段 URL decode；例如文件 `%5Bname%5D.html` 不能与 `[name].html` 合并。

manifest 不含本机绝对路径、S3 endpoint/key、访问 token、secret 或 server 源码。框架构建
主动注入 client 的公开环境变量仍是公开字节；必须另用 secret canary 验证平台没有额外泄露。

### 4.3 配额建议

以下是待基准确认的 SMB 默认值，不是 Cloudflare 套餐限制：

| 限额 | 初始建议 | 实施要求 |
| --- | --- | --- |
| 单文件 | 25 MiB | 同时限制声明值、实际接收量和读取量 |
| 文件数 | 20,000 / deployment | 含路径项；同字节不同路径仍计数 |
| 总资源字节 | 512 MiB / deployment | 按逻辑文件总量计配额，去重不能规避 |
| canonical manifest | 16 MiB | 有界读取、条目长度与深度检查 |
| 同时上传 | 4 / account | 服务端全局并发与磁盘 staging 总量另设上限 |
| 上传会话 | 2 / Worker，24 小时有效 | 到期不删除 ready deployment 引用 |
| 路由规则 | Worker-first 100 条 | 解析复杂度、单条长度另有限制 |

`_headers`、`_redirects` 的规则数、行长沿用固定上游 parser 限制。执行 SA-0 时以 portable asset
corpus 确认以上默认值；可选应用样本只提供额外分布观察，不定义平台限额。调整进入普通 operator
配置与边界测试，不为 fixture 放宽，不关掉限制。

## 5. SQLite、对象引用与上传协议

### 5.1 元数据

复用 `control.sqlite`，不为每个站点新增 SQLite。建议逻辑表如下，最终 DDL 与当前模型一起
整理，不预先指定“第 015 次历史升级”。

| 表 | 核心字段 / 约束 |
| --- | --- |
| `deployment_assets` | `deployment_id` PK/FK、manifest ArtifactRef、`routing_config_json`、可空 `binding_name` |
| `deployment_object_refs` | `(deployment_id, object_kind, digest)` 唯一；长度；bundle/manifest/blob 的可达性索引 |
| `deployment_uploads` | session ID、account/Worker ID、manifest digest、输入指纹、状态、过期时间、最终 deployment ID |
| `deployment_upload_objects` | `(session_id, digest)` PK、类型、声明长度、verified 状态；同 digest 不允许冲突长度 |

manifest 是路径和 MIME 的唯一权威；`deployment_object_refs` 只保存去重后的对象引用用于 GC
及备份，不复制另一份可独立编辑的路径 catalog。提交时从验证过的 manifest 派生该表，在同一
SQLite 事务插入；读取发现摘要/引用矛盾报 invariant 错误，不能以重建索引掩盖损坏。

descriptor 纳入 content kind、bundle ref、manifest ref、规范化 routing、assets binding 名、
所有其他 bindings/vars/secrets 的既有身份。仅换资源也产生新的 deployment/digest。
service target 的处理见独立方案；它不改变 assets 冻结在本 deployment 的规则。

所有对象复用当前 `ArtifactRef` 与 ArtifactStore 的物理 key 生成器。总方案的
`system/assets/blobs/...` 是逻辑分类，不意味着现有 `artifacts/v1/sha256/...` 已有第二套
物理实现；本阶段不必为了目录外观复制 store 或迁移对象。资源类型与可访问性由引用关系
决定，不由拥有某个 SHA-256 字符串决定。

### 5.2 上传会话 API

下列路径是拟新增 API，均在现有账户认证/配额/幂等边界内：

| 请求 | 含义 |
| --- | --- |
| `POST /v1/accounts/{a}/workers/{w}/deployment-uploads` | 提交有界 manifest、bundle inventory 和 routing；返回 session 与需要上传的 digest |
| `PUT .../deployment-uploads/{u}/objects/{sha256}` | 流式接收声明对象；服务端核对长度和 SHA-256 后确认 |
| `GET .../deployment-uploads/{u}` | 查询已确认对象与会话结果；无 secret/S3 key |
| `POST .../deployment-uploads/{u}/finalize` | 提交部署元数据，校验全部引用，进入统一 pipeline；可请求 promote |
| `DELETE .../deployment-uploads/{u}` | 幂等取消尚未 finalize 的会话，撤销临时引用；不删除共享字节 |

工具链的 Worker-only 与带资源部署都应收敛到同一输入/校验模型。是否保留当前 bundle body
端点作为简便入口是 API 组织问题，不得保留第二套部署语义；它调用同一个 pipeline，不承诺
兼容早期开发版 wire format。上传会话不能变成一套并行的 Worker 数据库。

要求：

1. 会话绑定 account、Worker ID、内容指纹；idempotency key 相同但输入不同返回冲突。
   重试必须沿用同一 key，不能每次生成新 key 后声称“幂等重试”。
2. 服务端检查 manifest 和库存；客户端 `missing=false`、S3 ETag、HEAD 或自报摘要都不是
   完整性证明。确认上传必须校验实际字节；resume 只能复用平台已验证且仍有引用的对象。
3. 去重不提供跨账户存在性查询：只向调用方确认它已有权引用的对象，其他 digest 仍要求
   上传字节。内部可做物理去重，但不让猜中的 digest 变成读取凭据。
4. finalize 在确认 bundle、manifest 和全部 blob 完整后，将 staging deployment、引用、
   bindings 等一次入库，再执行既有真实 runtime validation。Assets-only 只验证资源与
   路由，不能伪造一次租户代码验证。
5. ready 后可 promote；相关 Queue/Cron/DO/Workflow 的既有 promotion 协调仍执行，不能
   另写一个只改 active 指针的 assets 快捷通道。
6. S3 I/O 和 workerd validation 不在 SQLite 写事务中。当前 active 不随上传、校验失败或
   会话超时改变。响应丢失后可按会话/幂等记录查询同一结果。

finalize 元数据可以包含 write-only secrets，但 secrets 使用现有封装存储，不进入上传
manifest、状态响应、对象 metadata、CLI 诊断或原始错误。上传认证只走 header；不返回带
platform token 的 URL，不新增租户可访问的 S3 凭据或任意物理 key 上传口。

### 5.3 中断与恢复

会话状态使用 `open → finalizing → committed`，以及 `aborted/expired`。finalizing 的
deployment ID 先持久化；重启检查同一 pipeline 的结果，继续未完成验证或返回已存在的结果，
不能重新造一个 deployment。active 指针的最终事务是发布线性化点。

上传断开时取消本次接收并清理该会话自己的部分 staging 文件；不得删除其他上传或已有对象。
S3 写完而 SQLite 未确认可留下孤儿；恢复重新校验或由 GC 延迟清理，不能将它直接标为成功。
检测到部分 manifest、长度不符、摘要错误或损坏缓存时拒绝 ready/promote。

## 6. 路由与 HTTP 语义

### 6.1 自动路由与显式 binding 不混用

| 情况 | 默认 HTTP 入口 | `ASSETS.fetch()` |
| --- | --- | --- |
| Worker-only | 用户 Worker | 没有声明就没有该 binding |
| Assets-only | 资源 handler | 没有租户执行主体 |
| `run_worker_first: true` | 用户 Worker；不自动二次 fallback | 直接资源 handler |
| 路径规则命中 Worker | 用户 Worker；保留请求 body | 直接资源 handler |
| 路径规则命中排除项 | 资源 handler | 直接资源 handler |
| 默认资源优先 | 用上游 `canFetch` 语义判定，命中资源，否则 Worker | 应用该部署的完整资源规则并返回响应 |

不能用“先真正 fetch 资源，看是否返回 404”实现 `canFetch`：自定义 404 页、SPA fallback、
重定向和请求方法会导致不同决策，也会多读 S3。探测只查固定 manifest/规则，不消费上传
body、不触发 tenant Worker，选定后只执行一个响应路径。用户 Worker 返回 404/500 均不自动
补一次 assets；用户需要 fallback 时显式调用 binding。

路径规则采用上游 glob-only 语义；`!` 排除匹配优先于包含匹配，不能把配置直接送给 glob
文件库。`run_worker_first` 数组与布尔值的 fallback 行为分别建测试。SPA 模式还必须测试
`Sec-Fetch-Mode: navigate`，不能把所有未命中的 JS/API 请求统一变成首页。

保留官方 assets 兼容开关 `assets_navigation_prefers_asset_serving` /
`assets_navigation_has_no_effect` 及 `2025-04-01` 默认启用边界；显式路径规则的模式按上游
`has_static_routing` 处理，不再依赖浏览器 header 决定 fallback。这是有来源的 CF 行为，
不是旧 open-compute 兼容层。语义见
[Worker 路由](https://developers.cloudflare.com/workers/static-assets/routing/worker-script/)与
[SPA 路由](https://developers.cloudflare.com/workers/static-assets/routing/single-page-application/)。

默认资源可绕过用户 Worker，因此鉴权页面必须配置 Worker-first，并在代码中决定是否调用
ASSETS；不能仅在文档里说“Worker 做鉴权”却让默认资源路径先返回文件。

框架生产验收使用独立 hostname 的根路径路由，TLS 沿用已有入口配置，不增加网关依赖。
现有 `platform_path` 开发 URL 不天然支持框架的绝对 `/_next/...` 引用：部署工具必须检测
base path 是否匹配。子路径部署仅在框架构建已显式配置相同 base path 时启用，不静默去掉
路由前缀、改写 HTML/chunk 或把根路径资产请求猜测分配给某个 Worker。Service fetch 与
ASSETS.fetch 同样使用应用传入的 URL 路径，不偷偷套入调用方的 public route 前缀。

### 6.2 HTML、404、规则文件

实现 `html_handling` 的四种值：`auto-trailing-slash`、`force-trailing-slash`、
`drop-trailing-slash`、`none`。目录/index、`.html` 和尾斜杠的优先级用固定 handler 测试移植，
不凭经验增加“试几个文件”的搜索逻辑。参见
[HTML handling](https://developers.cloudflare.com/workers/static-assets/routing/advanced/html-handling/)。

`not_found_handling` 支持 `none`、`404-page`、`single-page-application`；分别覆盖空缺失、
最近层级 `404.html` 与根 `index.html` fallback，以及正确状态码。内部对象缺失不走这些分支。

`_headers` 支持路径/允许的绝对 URL 匹配、添加/覆盖/移除、多个匹配合并和占位符；只影响
资源 handler 响应，不全局改写 Worker 产生的 HTML/Flight。`_redirects` 支持固定上游的
301/302/303/307/308、同站资源 200 rewrite、splat/placeholder 和查询串保留规则；200 rewrite
不访问外网。静态规则与动态规则优先级、redirect 与 headers 的次序遵循固定 parser/handler。
格式/限额错误在 deploy 时返回带行号的稳定诊断，不默默忽略；这比上游部分无效行忽略行为
更严格，列入 conformance 差异。参见
[Headers](https://developers.cloudflare.com/workers/static-assets/headers/)与
[Redirects](https://developers.cloudflare.com/workers/static-assets/redirects/)。

### 6.3 路径和 HTTP 字节

请求 URL、manifest 逻辑路径和本地 cache 文件名是三种身份。匹配沿用上游 decode/re-encode
顺序并保留坏转义处理；不能将 URL pathname 拼接到磁盘根目录，也不能对相同输入重复 decode。
测试至少包括 Unicode、空格、方括号、`%2F`、`%25`、双编码、重复斜杠、反斜杠、点段和
query/fragment。请求 host 不参与对象权限选择，但会影响规则匹配和 redirect Location。

资源响应以 manifest 的 MIME/size 和稳定 ETag 为基础，GET/HEAD 保持同一组表征 metadata，
HEAD 不发 body。ETag 是平台表征标识，不泄露物理 key，也不直接采用 multipart S3 ETag。
支持固定基线的 If-None-Match/304；缺失、弱标记及不匹配项明确测试。没有实际启用压缩时
不产生 `Content-Encoding`。动态 Worker 响应不强加静态缓存 header。

缓存 header 默认取固定上游资源语义，由 `_headers` 的合法配置覆盖。不要给全部 HTML 自动
加一年 immutable，也不凭文件名猜它永不改变。Cloudflare 专有 CDN 命中/colo headers 不伪造。

## 7. 资源读取、缓存与存活期

Rust 提供私有、generation-authenticated 的 manifest/字节读取能力，输入绑定到已验证的
deployment 和其 manifest 成员。tenant 只获得 fetch 能力，不获得 `exists(digest)`、
`getByETag`、物理 key、runtime-source 或原始 backend token。system helper 不导出成可被
RPC 探测的公共方法；使用真正私有方法/模块函数。

本地缓存按 immutable ArtifactRef 存字节，路径/headers 响应按 deployment+manifest+配置
隔离。每次读取先验证权限/引用，暖缓存不能绕过删除状态。首个缓存 miss 应流式下载到有界
staging 文件，核对完整 size/hash 后原子进入 cache，再向响应提供字节；这样损坏字节不会
在最终 hash 发现前已发给浏览器。不把完整 25 MiB 文件放进 JS/Rust 堆，也不把部分文件当
完整 cache。相同对象合并下载；磁盘不足、超时和并发均受预算约束。

服务端响应流负责背压，取消释放其读取句柄和本次 transfer 引用。可信已校验 cache 可以在
S3 短暂故障时继续读取；cache miss 的 provider 故障明确失败，不合成 404。外部人员修改
S3 对象不属于正常写路径，发现后报告完整性错误；不能承诺对所有已缓存对象实时发现外改。

目前 public ingress 的 `PinnedBody` 只直接证明 Rust body 存活。新增实现必须把 execution
completion、body、升级连接和 `waitUntil` 作为不同的完成条件；body EOF 不能单独放行
deployment 删除。复用/补齐统一运行时存活协议，具体门槛见 Service Binding 的 SB-0/SB-4。
assets binding 在后台执行时也要保持所属 deployment 可达；只保护单次 S3 GET 不够。
若 stock pin 无法提供可靠完成证据，应拒绝危险回收并将阶段标为阻塞，不能靠 TTL 猜测完成。

## 8. 发布、回滚、GC 与备份

### 8.1 原子性边界

ready deployment 持有完整 refs 后才允许 promote。所有 public/default service 请求在开始
时取得一个 deployment snapshot；一次响应不得以旧代码配新 manifest。执行中的旧 Worker
继续通过自己的 ASSETS 读取旧资源。回滚使用保留的原部署引用，不重新编译或上传。

逐请求固定不等于浏览器会话固定：浏览器拿到 d1 HTML 后，下一次 chunk 请求可能已遇到
d2。必须专门测试并记录这个边界。默认不搜索历史 manifest、不重写 vinext chunk 路径、
不设置隐藏 sticky cookie；应用可在自己的发布产物中显式保留所需旧 hash chunks。
若基线用例要求跨发布无缝会话，应单独实现可证明的 version-affinity 方案，不能用一次请求
的 pin 或“保留了 S3 对象”冒充已经解决。

### 8.2 GC

GC 的根包括：所有未删除且被保留的 deployment refs、未结束上传会话、在途执行/传输、
保留备份的对象引用。不是只有 active deployment。错误部署和过期会话按清理策略退出根集，
对象在宽限期后才成为候选；宽限期不能代替实际引用检查。

复用平台单写者模型，加一条统一 Artifact 生命周期锁：上传/引用提交持读 guard，GC 删除
对象持写 guard，重新查询引用后才做 S3 删除。guard 可以跨 S3 await，但绝不跨 SQLite
事务 await；全局并发有上限，GC 每次只删除一小批，避免长期阻塞发布。bundle、assets 和
backup 对共享对象的引用增减都走同一 owner，不能只锁 assets 留下另一个 GC 入口。

删除失败记录可重试状态，不能先忘掉对象再假报成功。进程崩溃后重新查 SQLite 根和对象状态，
不凭 S3 list 一次未发现就判定不存在。恢复上传在同一锁内重新确认对象，避免“刚确认可用，
GC 随后删除，接着 ready”的竞态。缓存淘汰单独管理，不得顺带删除 S3 authority。

### 8.3 备份与恢复

现有离线快照纳入新表与 manifest/blob 引用，backup GC 根必须覆盖这些对象。备份可以不
加密；这沿用“备份不考虑保密性”的范围选择，不免除平时的 token/secret 隔离、摘要校验、
恢复一致性和鉴权。恢复预检必须确认所需对象可读且完整，再恢复可服务状态。已有备份若
包含对象副本就复用该路径；若只记录远端 refs，必须明确保留策略及 S3 丢失会导致不可恢复。
一次缓存命中或仅恢复 SQLite 不能证明资源已可恢复。

## 9. 错误和可观测性

拟新增稳定错误：`ASSET_MANIFEST_INVALID`、`ASSET_PATH_INVALID`、`ASSET_LIMIT_EXCEEDED`、
`ASSET_UPLOAD_INCOMPLETE`、`ASSET_UPLOAD_CONFLICT`、`ASSET_INTEGRITY_ERROR`、
`ASSET_STORAGE_UNAVAILABLE`、`ASSET_CONFIG_UNSUPPORTED`。复用已有通用码时保持同一含义。

普通路径未命中使用配置决定的 404/SPA；已经被 manifest 引用的 blob 丢失、超时或损坏是
5xx/稳定系统错误，不是普通未命中。HTTP headers 已发送后只能终止错误流并记录分类，不能
再声称已经返回完整 JSON 错误响应。API 不返回 S3 原始异常、bucket 或凭据。

记录上传/验证字节、manifest 条目数、资源/Worker 路由次数、cache 命中、校验失败、首字节
等待、pin 数和 GC 拒绝原因。metrics 采用有界标签；URL、Worker ID、digest 不做高基数标签。
调试事件可带 request/deployment ID 与稳定分类，不记录正文、cookies、secrets 或完整 query。

## 10. 工作包与验收

### 10.1 依赖顺序

| 顺序 | 工作包 | 必须先有 | 交付与退出条件 |
| --- | --- | --- | --- |
| SA-0 | Static Assets contract 与 portable fixture | P3.0 平台输入 | 官方配置/HTTP/source 映射、模块/资源界限、配额样本、固定上游路由矩阵 |
| SA-1 | domain/schema/descriptor | SA-0 契约范围 | 三种 deployment 类型、统一 binding 名检查、对象引用与会话状态机的事务测试 |
| SA-2 | 上传、导入与 ready | SA-1 | importer/CLI、流式上传、摘要校验、幂等 resume、静态部署和正常 Worker validation |
| SA-3 | 资源 handler 与默认路由 | SA-2 | 同一份匹配/响应逻辑；public/ASSETS；HTML/SPA/rules 与冷暖一致性 |
| SA-4 | 生命周期与运维 | SA-3、共同存活期证明 | publish/rollback、后台任务、GC/备份、故障/取消/重启矩阵 |
| SA-5 | 产品 conformance qualification | SA-4、P3.4 harness | portable fixture 在 platformd/Cloudflare 的结果、deviation 与阶段报告 |
| SA-A1 | 可选应用 qualification | SA-5、独立应用 baseline | 选定 App/Pages/static-export workload 的 JS/CSS/fonts、hydration 和应用报告 |

SA-0 是产品测试和源码契约工作，不恢复已退役的 `/poc` 工程。Service 的 SB-0 可提前验证
共同存活期风险；实现顺序以依赖为准，不要求把所有资产功能做完才发现 pin 协议不成立。
当前 SA-1 至 SA-4 的实现早于正式 P3.4 catalog；这些 Gate 仍是有效核心证据，但必须由 SA-0/SA-5
反向映射和补差，不能因为代码已经存在就跳过 contract qualification。

### 10.2 最小测试矩阵

| Gate 类别 | 必须观察的结果 |
| --- | --- |
| 导入 | 普通 Worker + Assets、Assets-only 与固定多图产物可导入；server/client 隔离；hash chunk/dynamic import 保持；static export 无伪 Worker |
| 扫描 | ignore、链接替换/逃逸、重名、特殊文件、超限、坏 MIME/CRLF、secret canary 正确拒绝或隔离 |
| 部署 | 新增/修改/删除资源改变 descriptor；上传缺块/错 hash 不 ready；幂等冲突和 resume 不多造部署 |
| 基本 HTTP | GET/HEAD、MIME、ETag/304、无 body、缺失响应、显式 Range 策略与已声明配置一致 |
| 路由 | assets-first、Worker-first true/array、排除优先、导航 header/date/flag、Worker 404 不 fallback |
| 路径 | HTML 四模式、404/SPA、编码/双编码、Unicode、redirect/header 优先级和 query；对照上游断言 |
| 身份 | ASSETS 不能读取其他 deployment/account 的 digest；伪造内部 header、暖 cache 和 S3 key 均不越权 |
| 一致性 | 请求途中 promote/rollback 不混资源；旧 Worker 的 ASSETS 保持旧版；跨请求切换边界单列 |
| 流与清理 | 慢读、取消、半包、S3 超时/损坏、磁盘满、waitUntil；已开始执行与 pin 释放有实际证据 |
| GC/恢复 | 上传与 GC 并发、保留旧部署/备份、删除共享对象拒绝、两进程 crash/restart 后引用不丢 |
| 可选应用 | 浏览器确实下载 JS/CSS/fonts 并 hydration，无 console/network 错误；不是只断言 HTML 200；结果只进入 Application verdict |

全部对照使用固定上游测试名称/断言与输入身份，不依赖网页截图判断行为。对失败要区分
上游本来未启用、平台错误、尚未执行和外部条件；新的 skip/fixme 或平台失败不能算通过。
S3 协议夹具用于确定故障时序，实际配置的 provider 另有集成验证，不把 mock 成功写成 provider
已通过。旧 G0 `D-abort` 仅保留既有边界，不豁免上传中断、流错误、pin 或已声明平台 contract；
应用失败另按 Application verdict 分类。

### 10.3 文件与测试入口

拟新增 `packages/toolchain/src/assets/`、`packages/toolchain/src/import/`、
`packages/runtime/src/assets/`；Rust 按已有 crate 归属拆入 assets 子模块。
`core` 放类型/错误，`storage` 放 SQL/repository，`artifacts` 放对象 I/O，`workers` 放
pipeline/descriptor/lifecycle，`service` 放 API 与运行时组合；`workers` 不依赖 `runtime`。
新增 TS 使用严格类型和统一 Bun workspace，不增加 npm/pnpm lock 或生产 JS 运行时。

拟在 `test/gate.py` 注册 `p3-assets` 对应的 Rust 集成目标，并把 JS 路由测试接入现有
`bun run test:js`。以下是注册后才能执行的命令，当前不声称存在：

```sh
./test/gate.py p3-assets --list
./test/gate.py p3-assets
OPEN_COMPUTE_GATE_ROUNDS=3 ./test/gate.py p3-assets
```

开发只执行相关目标一轮。源码冻结后，按 testing reference 完成 build/generated、JS、
Rust 静态检查与 coverage（保持 90% 门槛），最后统一验收：完整 workspace 一轮、登记的时序
用例补两轮。新增 Gate 同时登记完整用例及重复归属；固定协议/路径矩阵不机械重复。一次构建、目标去重，
新增目标经隔离审查后才并行；不递归调用旧 `test-p*.sh`，不要求重跑已退役 POC。
真实浏览器/框架测试使用独立 application manifest 和报告，不能因 Rust Gate 通过就标为已运行，
也不能因它未运行就抹去已有平台产品 Gate。

阶段报告记录固定输入、逐项用例结果、失败/未运行项、配额测量、cold/warm 延迟、字节与
内存/磁盘峰值、GC/恢复证据和来源许可证。核心实现完成后归档设计；外部资格拆到 active
acceptance，不因远端条件继续把已完成设计留在 `docs/` 根目录。

## 11. 参考实现

- [Cloudflare Static Assets binding](https://developers.cloudflare.com/workers/static-assets/binding/)：外部配置与 fetch API。
- [固定 asset handler](https://github.com/cloudflare/workers-sdk/blob/296a1a7c97e027a308740e1eaaa6d904dec8f102/packages/workers-shared/asset-worker/src/handler.ts)：canFetch、HTML、路径编码与 HTTP 响应。
- [固定 router-worker](https://github.com/cloudflare/workers-sdk/blob/296a1a7c97e027a308740e1eaaa6d904dec8f102/packages/workers-shared/router-worker/src/worker.ts)：Worker-first 与 assets-first 的组合。
- [固定规则匹配器](https://github.com/cloudflare/workers-sdk/blob/296a1a7c97e027a308740e1eaaa6d904dec8f102/packages/workers-shared/asset-worker/src/utils/rules-engine.ts)：规则匹配，不照搬静默忽略错误的产品策略。
- [固定兼容开关](https://github.com/cloudflare/workers-sdk/blob/296a1a7c97e027a308740e1eaaa6d904dec8f102/packages/workers-shared/asset-worker/src/compatibility-flags.ts)：官方导航行为开关与日期。

这些来源证明可参考的接口与算法，不证明 open-compute 的上传、权限或生命周期实现已经通过。

## 12. 2026-08-29 实施记录

本次直接实现了当前 Day1 平台模型：Worker-only、Worker + Assets、Assets-only 统一 deployment，
资源 manifest/routing/对象引用与可恢复上传会话，工具链扫描、框架产物 importer 和断点续传
deploy 协议，受信任默认 HTTP router 与显式 assets binding，以及 deployment-scoped 私有读取、
校验缓存、存活 pin、快照引用和删除围栏。Assets-only 不生成伪 Worker；上传 finalize 的成功和
失败均形成可精确重放的终态。

维护证据如下：

- `bun run build`、`bun run typecheck`、`bun run check:generated` 和 59 项 JS 测试通过；
- format、Clippy（workspace/all-targets/all-features）、no-default-features、Rust 1.98 MSRV、metadata、
  dependency boundaries 与 `git diff --check` 通过；
- `./test/coverage.sh --jobs 1` 的 33 个目标全部通过，Rust 行覆盖率为 90.11%，报告位于
  `target/llvm-cov/summary.json`，执行报告为
  `.temp/gate-run/20260829T183335-e3f0d5e7/report.json`；
- `OPEN_COMPUTE_GATE_ROUNDS=3 ./test/gate.py --workspace --jobs 1` 通过：第一轮执行完整
  workspace，后两轮各执行 17 个登记的时序用例；报告为
  `.temp/gate-run/20260829T185236-dd70d44e/report.json`；
- `p3-assets` 使用本地已验证的 stock workerd `v1.20260826.1`，覆盖静态 GET/HEAD、ETag/304、
  HTML/redirect/404、Worker-first 规则、显式 binding、伪造内部 header、跨版本不可变性和
  删除存活围栏。

SA-0/SA-5 的本地 conformance qualification 已完成：仓库已有 P3.4 固定 contract catalog、能力与
deviation 双射，`assets.binding.routing` 在最终 contract report 中由真实 `p3-assets` 产品 Gate
证明通过。共享 portable runner 已实现并取得 Cache API 对照证据，但尚无 Assets routing/binding
直接对照；该缺口现由[独立验收计划](../acceptance/p3-assets-service-bindings-acceptance.md)追踪，不再阻止核心设计
归档。截至 2026-08-29 当次实施记录，vinext/React/Vite/RSC/browser 输入元组尚不存在；后续 P4
[Application Go](p4-nextjs-vinext-results.md)补充了固定应用和浏览器证据，但仍不能由应用结果推导为
完整 Assets Cloudflare differential。
