# P16：Capability-scoped Cloudflare-compatible TypeScript SDK

状态：**方案完成（2026-09-14）**；待固定输入复核、实现、npm bootstrap、真实 `ocd` Gate 与首次联合发布。

本文落实 [GitHub issue #68](https://github.com/elliothux/open-compute/issues/68)：发布一个
`@open-compute/sdk`，只暴露 `ocd` 已实现并取得资格的 Cloudflare 管理面资源，同时把 open-compute 专有操作放在
`client.openCompute` 下。SDK 是可信管理面客户端；不得注入 tenant Worker，也不得把 deployer/admin token 暴露给
tenant code。

## 1. 结论

Issue 的总体方向合理，保留以下决策：

- Cloudflare-compatible 方法沿用官方 TypeScript SDK 的 resource、method、参数、返回值、分页、upload、retry 和
  `APIError` 行为；
- supported surface 由现有 OpenAPI subset 与 capability authority 决定，不导出完整官方 `Cloudflare` client；
- open-compute-only 方法只位于 `client.openCompute`，route 继续位于 `/open-compute/*`；
- 复用官方 SDK 的 transport 与 resource method implementation，不重新实现 auth、retry、pagination、multipart 或
  error parsing；
- 不把 Stainless 设为 build、CI 或 release 依赖。当前只需一个 TypeScript SDK，仓库已经固定并验证官方
  Stainless-generated transport/resource code，本地生成的职责只是选择 surface 与生成 vendor extension。

原方案必须修正一处：**不能直接把官方 `Base*` class 当成安全的 capability selector**。例如固定
`cloudflare@7.1.0` 的 `BaseScripts` 同时包含 `update/list/delete/get` 和当前未支持的 `search`。即使 parent/child graph
正确，直接导出该 instance 仍会泄漏 unsupported leaf method。

P16 因此生成一个无继承方法的 facade graph：每个公开 node 只包含明确选中的 bound methods 与 child nodes；隐藏的
official `Base*` instance 只作为 delegate。这样运行时 property surface、TypeScript type、文档与 autocomplete 使用同一份
closed operation set，同时实际请求仍由官方 method implementation 和同一个 `BaseCloudflare` transport 发出。

## 2. 当前输入与是否升级 OpenAPI

2026-09-14 的仓库固定输入为：

| 输入 | 当前固定值 | 当日上游事实 | P16 决策 |
| --- | --- | --- | --- |
| OpenAPI dialect | `3.0.3` | `api-schemas` HEAD 仍为 `3.0.3` | **不升级 dialect** |
| Cloudflare API version | `4.0.0` | HEAD 仍为 `4.0.0` | **不改版本号** |
| schema revision | `b8687f42e28fbfcb296a350f7dbf16349ea900af`（2026-09-02） | HEAD 为 `461ea58b4394fbbb2a861e6da962e193ad07a3a9`（2026-09-14） | 实施开始时做协调升级评审，不直接追 moving HEAD |
| Cloudflare TypeScript SDK | `7.1.0` | npm `latest` 仍为 `7.1.0` | **当前无需升级** |
| Wrangler | `4.127.1` | npm `latest` 为 `4.131.2` | 不因 SDK 发布单独升级；按 Wrangler/product Gate 独立评审 |

从当前 schema pin 到当日 HEAD 相差 150 个 upstream commits；HEAD 保持相同 dialect/API version，但在 subset 的 141 个
selected operations 中有 13 个 operation definitions 变化：3 个 Worker secret、9 个 AI Search 和 1 个 Queue create。
这说明需要 review **schema revision drift**，不说明应把 `openapi: 3.0.3` 改成 3.1，也不允许只替换 lock hash 后宣称兼容。

### 2.1 固定输入升级规则

P16 实施的第一批工作执行一次 three-way pin review，随后由 2.2 的常规定时项目持续执行同一规则：

1. 选择一个 immutable `api-schemas` revision，而不是构建时读取 `main`；
2. 取得 npm registry 当时的 `cloudflare` stable version 和完整 tarball identity；
3. 逐项比较 selected OpenAPI operation、official SDK resource/method/type 与当前 `ocd` wire evidence；
4. 只有三者一致的 operation 才能进入 SDK；schema 已变化但 latest official SDK 尚未包含时，保留当前已验证 pin，或把该
   operation 留在 unsupported/planned，不本地伪造官方新类型；
5. schema、official SDK 或 Wrangler 任一 pin 变化时，同步更新 lock、subset、capability projection、fixture、文档与真实
   Gate，不保留双版本生成分支。

因此当前判断是：OpenAPI **格式版本不需要升级**；schema snapshot **需要在 P16 开始时复核并按上述规则协调刷新**；
`cloudflare@7.1.0` 当前已经是 latest，不做无来源的版本变化。Wrangler 不属于 SDK runtime dependency，不能为了“看起来最新”
把 4.131.2 混入本次 SDK 生成。

### 2.2 常规上游刷新项目

P16 必须把首次 three-way review 接入持续维护的
[Cloudflare 上游刷新](references/cloudflare-upstream-refresh.md)，不能只在 SDK 首次实现时人工检查一次。该 reference 统一拥有检查频率、
候选分类、Draft PR、权限、验收与失败处理；P16 只负责实现 scanner、classification fixture、scheduled workflow 和 SDK 所需的
closure checks。定时发现新版本不自动移动正式 pin；只有固定 identity 的兼容组合通过完整资格并经人工 review 后才进入当前模型。

## 3. 权威输入与派生物

保持现有 authority，不增加手写的第二份 endpoint inventory：

| 内容 | 权威输入 |
| --- | --- |
| upstream identities | `openapi/upstream/cloudflare-openapi.lock.json` |
| Cloudflare selected operations 与状态 | `openapi/cloudflare-subset-manifest.json` |
| Cloudflare wire schema | `openapi/cloudflare-v4-subset.json`（由固定 upstream schema 机械生成） |
| vendor routes/schema | `openapi/open-compute-extension.json` |
| product capability projection | `openapi/p6-capability.json` 与 `share/cloudflare-capabilities.json` |
| official method implementation/type | lock 精确固定的 `cloudflare` npm tarball |

新增文件只能是以下派生物：

- generated facade source；
- generated public type aliases；
- generated surface report：operation、official resource path、method、delegate module/export、status、operation digest；
- combined OpenAPI document，供 SDK 文档与 inspection 使用。

surface report 不允许手改，也不参与选择 operation。生成器每次从 authority 和固定 npm package 重新构建它；`--check` 对
byte drift 失败。Combined OpenAPI 不是新的 source of truth；合并时 path、operationId 或 component name 冲突必须 fail closed，
不能以后写覆盖前者。

## 4. Public SDK contract

使用方式：

```ts
import { createOpenComputeClient } from "@open-compute/sdk";

const client = createOpenComputeClient({
  apiToken: process.env.OPEN_COMPUTE_API_TOKEN!,
  baseURL: "https://compute.example/client/v4",
});

await client.workers.scripts.versions.list("app", { account_id });
await client.d1.database.list({ account_id });
await client.openCompute.system.status();
```

### 4.1 Client construction

`createOpenComputeClient()` 创建一个隐藏的 official `BaseCloudflare` transport，但返回值不是完整 `Cloudflare` 或
`PartialCloudflare` instance。返回的 plain object 只含 generated resource graph 和 `openCompute`，不公开 official top-level
resources，也不把 `get/post/request` 等 generic transport method 当作绕过 capability surface 的入口。

`OpenComputeClientOptions` 复用 official `ClientOptions` 的 timeout、retry、fetch、headers、logger 等字段，但收紧：

- `apiToken` 必填且必须由 caller 显式提供；不隐式读取 `CLOUDFLARE_API_TOKEN`；
- `baseURL` 必填；不允许落到 official default `api.cloudflare.com`；
- URL 必须是 absolute、无 userinfo/query/fragment，canonical path 精确结束于 `/client/v4`；
- production 使用 HTTPS；HTTP 只允许 loopback test/development address；
- 禁止调用方通过 `defaultHeaders` 覆盖 Authorization 或平台内部 headers；每 request 的 official safe options 仍可用。

SDK 不在 construction 时联网，也不自动探测版本。`client.openCompute.capabilities.get()` 提供显式 inspection；pre-1.0 只对与
SDK 相同 `X.Y.Z` release 的 `ocd` 做正式资格声明，但版本不匹配不能通过 fallback、alias 或偷偷改写请求来修复。

### 4.2 Cloudflare-compatible graph

对每个 selected standard operation，生成器必须证明：

- official resource `_key` 对应公开 path，例如 `workers.scripts.versions`；
- official method name、参数顺序、parameter types 和 return type 原样复用；
- official implementation 发出的 HTTP method/path 与 subset operation 唯一匹配；
- operation 状态为 `supported` 或带公开 deviation 的 `supported_with_deviation`；
- 同一 operation 只导出一次，且每个 supported operation 恰好导出一次。

每个 facade node 使用无 prototype 的普通对象。selected method 绑定到隐藏 delegate；child node 由同一 graph 递归生成。不得把
official class instance、namespace wildcard、完整 `Cloudflare` type 或整包 `resources` re-export 出去。用户需要的 params、result、
page 和 error types 只按 selected method 的 reachable public types 生成 alias/re-export，不能顺手导出 unsupported resource types。

### 4.3 `openCompute` namespace

`openCompute` 从 `open-compute-extension.json` 生成，继续复用隐藏的 `BaseCloudflare` transport。它遵守相同 URL encoding、retry、
timeout、request options 和 `APIError` 语义。Vendor operation 不得挂在 Cloudflare standard resource 下。

当一个 vendor feature 后来成为官方 Cloudflare API 时，在 schema、official SDK、`ocd` 与 Gate 同时一致后直接迁移到 official
resource/method，删除原 vendor method；不保留 alias 或 duplicate call path。

## 5. 生成流程

实施时把 `packages/cloudflare-extension` 直接改为 `packages/sdk`；package 名称改为 `@open-compute/sdk`，删除旧 private package，
不同时维护 extension-only 和 public SDK 两套入口。

### 5.1 Offline deterministic generator

正常 build/check 的 generator 不联网，只消费 checkout、Bun lock 已安装的精确 `cloudflare` package 和 lock identities：

1. 验证 OpenAPI subset、manifest、extension 与 lock digest；
2. 验证 installed `cloudflare/package.json`、tarball integrity 和当前 lock 中记录的 sentinel/source digests；
3. 用 TypeScript compiler AST 静态读取 published `resources/**/*.mjs` 与对应 declarations，提取 class `_key`、prototype method、
   HTTP verb、normalized path template 和 public signature；不执行 package source，也不使用只匹配少数 fixture 的 regex；
4. 以 `(HTTP method, normalized path)` 把每个 supported operation 唯一映射到 official resource method；missing、ambiguous、
   dynamic/unresolved route 一律停止生成；
5. 生成 hidden delegates、closed facade nodes 和 reachable selected types；
6. 从 extension schema 生成 `openCompute` methods/types；
7. 生成 surface report 与 combined OpenAPI；
8. 经 Prettier 格式化并写入 committed generated source；`generate --check` 在临时目录重建并逐字节比较。

如果 official SDK 改变 generator pattern，先升级 scanner 并用旧/新 fixture 证明解析规则，再接受 pin；不能加入 operation-specific
hardcode 或退回导出完整 resource class。

### 5.2 Combined OpenAPI

`openapi/open-compute-sdk.json` 机械合并 Cloudflare subset 的 supported operations 与 vendor extension：

- 不包含 `planned`、`unsupported` 或未取得资格的 route；
- 保留 standard operation 的 official schema、status/deviation metadata 和 upstream identity；
- vendor component 使用 `OpenCompute` 前缀；冲突直接失败；
- server base 统一为 `/client/v4`；
- 顶层记录 SDK version、official SDK pin、schema revision 和 surface digest。

该文件可供 website 生成 API reference，但不反向驱动 SDK，也不提交 hosted service 才能生成的产物。

## 6. Package contract

`packages/sdk/package.json` 的发行合同：

- name：`@open-compute/sdk`；
- version：与同一 tag 的 workspace/`ocd` `X.Y.Z` 精确一致；
- public scoped package，`publishConfig.access = "public"`；
- `cloudflare` 是 exact catalog runtime dependency；pack 后必须解析为 lock 固定的普通精确版本，不能发布 `catalog:`、`workspace:`
  或 range；
- `sideEffects: false`，无 install/prepare/postinstall lifecycle script；
- Node.js 20+、Bun 1+；发布 ESM 与 CommonJS，以及对应 declarations；
- `files` 只含 `dist/`、package README、LICENSE；不发布 tests、source authority、token、fixture、`.temp` 或 sourcemap。

Rolldown 生成 `dist/index.mjs` 与 `dist/index.js`，TypeScript 7 strict check 并生成 `.d.mts`/`.d.ts`。Official `cloudflare` 保持
external dependency，不复制或 vendor 它的源码。`dist/` 与其他 generated build output 一样不跟踪；committed generator source 和
surface report 才接受 drift check。

每次 package build 在 `.temp/sdk-package/` 执行 `bun pm pack`，因为 Bun 会把 catalog 解析成普通 npm version。随后检查 tarball：

- package name/version/repository/license/exports/engines 正确；
- dependency 精确为 lock 中的 official SDK version；
- 文件 allowlist、mode、size 与无 secret scan 通过；
- 从 tarball 安装到隔离项目后，Node ESM、Node CommonJS、Bun 和 TypeScript consumer smoke 通过；
- packed shasum、SRI integrity、surface digest 与 official SDK pin 写入 package report。

## 7. 验证与 Gate

### 7.1 Generated/static checks

- clean checkout 重建 generated source 与 reports，结果 byte-identical；
- exported standard operation 与 subset supported set 双向一一对应；
- extension methods 与 `open-compute-extension.json` 双向一一对应；
- runtime property walk 和 TypeScript symbol walk 得到相同 graph；
- negative compile fixtures 分别证明 unsupported top-level module、unsupported sibling、unsupported leaf method 和 raw generic request
  不可访问；这些 fixture 作为预期失败的独立 `tsc` invocation，不用 `any`、double assertion 或 ignore directive；
- public type declarations不存在完整 Cloudflare namespace 或 unreachable unsupported types；
- baseURL/auth/header validation 的 success/failure matrix 通过；
- package tarball allowlist、dual-module consumer 与 no-network import smoke 通过。

### 7.2 Official behavior and real process

扩展现有 `crates/service/tests/cloudflare_sdk_gate.rs`，仍只启动一个 ready production `ocd` process，不建立第二套兼容 suite：

- representative JSON request/response；
- Workers multipart upload；
- raw/binary download；
- single-page 与 V4 pagination；
- bodyless POST；
- retry、timeout 和 official `APIError` envelope；
- path parameter encoding；
- extension method；
- standard method 与 official full client 对同一 fixture 的 request trace 等价。

Gate 捕获全部 outbound request，断言没有请求目标是 `api.cloudflare.com`，没有 token、secret、module source 或 internal path 出现在
generated files、stdout/stderr、errors 与 `ocd` logs。Package 变更完成后执行仓库要求的 TypeScript checks、coverage 与一次最终
workspace Gate；不重复同一冻结输入的 aggregate。

## 8. Versioning 与联合发布流程

P16 改变当前“只发布 native executable、不发布 npm package”的 release contract。完成时必须同步修改
`docs/references/releasing.md`、release notes 模板/校验、`release.json` schema、release tests 与
`.github/workflows/release.yml`。

### 8.1 Version policy

- SDK 与 `ocd` 使用同一个 stable `X.Y.Z`，只由已经合入 `release` 的 annotated `vX.Y.Z` tag 触发；
- 不做独立 SDK tag、nightly、floating canary 或从任意 branch publish；
- pre-1.0 只对同版本 SDK/server 组合给出正式兼容声明；
- SDK-only fix 也发布新的完整 patch release，不覆盖 npm version 或既有 GitHub Release；
- `latest` 只由成功的 stable publish 推进。

### 8.2 Build and qualification

1. `main` CI 执行 generator drift、typecheck、unit/negative fixtures、build、pack inspection 和 isolated consumer smoke；
2. version PR 同时更新 Cargo workspace version、`packages/sdk` version 和 release notes；不由 workflow 改 version；
3. tag `validate` 拒绝 SDK/workspace/tag version 不一致；
4. `sdk-package` job 在 GitHub-hosted Ubuntu 上用固定 Bun、Node 24 与 npm 11.5.1+ 从 clean checkout 构建一次 tarball，生成 report，
   并作为内部 workflow artifact 传给 publish job；
5. 现有 coverage、macOS workspace Gate、Linux privileged egress 和三个 native package jobs 并行运行；
6. 所有 qualification 与 native assemble 成功后才允许 registry writes。

`release.json` 增加 SDK identity：package name/version、tarball SHA-1/SHA-512 integrity、surface digest、OpenAPI revision 和 official
`cloudflare` version。GitHub Release 仍只有三个 executable、`release.json` 与 `SHA256SUMS` 五个 assets；SDK tarball 由 npm registry
分发，不复制为第六个 GitHub asset。

### 8.3 npm publish

常规发布使用 npm Trusted Publishing：

- npm package 把 `elliothux/open-compute`、`release.yml`、`npm-release` Environment 配置为唯一 trusted publisher，并只允许
  `npm publish`；
- publish job 使用 GitHub-hosted runner，job-level permission 只有 `contents: read` 与 `id-token: write`；
- `actions/setup-node` 固定 Node 24、registry 为 `https://registry.npmjs.org`，显式验证 npm CLI >= 11.5.1；
- 依赖安装、build 和 pack 仍由 Bun/`bun.lock` 负责；registry write 使用 `npm publish <verified-tarball>`，以获得 OIDC trusted
  publishing 与自动 provenance；
- 不保存 `NPM_TOKEN`，不在 PR/main/self-hosted runner 中授予 OIDC publish permission。

`@open-compute/sdk` 目前不存在，npm 要求 package 已存在才能配置 trusted publisher。因此第一次不发布 placeholder：在用户明确
授权 external write 后，用受保护的一次性 bootstrap job 和短期 granular token 发布**第一个真实、已完整验收**的 package tarball，
同时生成 provenance；随后立刻为该 package 配置 trusted publisher、撤销 token，并从永久 workflow/config 删除 token fallback。
P16 在 trusted publisher 已生效且没有长期 publish token 前不能标记完成。

### 8.4 Cross-registry ordering and recovery

npm 与 GitHub Releases 没有跨 registry transaction。固定顺序为：

```text
all qualification PASS
  -> create/upload/verify GitHub Draft
  -> publish npm exact tarball
  -> read back npm version + shasum + integrity + provenance
  -> publish the already verified GitHub Draft
```

这样公开 GitHub release 永远不会宣称一个尚未验证的 SDK。npm 成功而 GitHub Draft publication 暂时失败时，可能存在一个短暂的
npm-first window；保留 Draft 并重试最后一步，不撤回或覆盖 npm package。

publish response unknown 或 rerun 时先读 registry：

- version absent：可用同一冻结 tarball重试；
- version exists 且 shasum/integrity/surface identity 全部匹配：视为 publish 已完成，继续 GitHub Draft；
- version exists 但任一 identity 不同：fail closed，保留证据并发布新的 patch version；
- 不使用 `--tolerate-republish` 隐藏 identity mismatch，不 `npm unpublish`，不移动 tag。

Release notes 新增必填 `## SDK`，记录 package/version、official SDK/OpenAPI pins、surface change、安装命令与 npm version link；
历史 release 不回写。`publish` job 只有在 npm read-back 和既有五个 GitHub assets read-back 都成功后才把 Draft 公开。

## 9. 文档与用户迁移

- package README 与 website 提供 installation、client creation、supported resource inventory、deviations、error/retry、pagination、upload
  和 version pairing；
- `openapi/open-compute-sdk.json` 驱动 HTTP/API reference，generated surface report 驱动 SDK resource index；
- 现有 `createOpenComputeExtension(new Cloudflare(...))` 是 private 未发布入口，P16 直接删除并改为
  `createOpenComputeClient()`；不发布 compatibility alias；
- Dashboard、Wrangler 和 tenant bindings 不改用该 SDK。Dashboard 是否以后消费 SDK 必须另行证明 bundle/权限收益，不能让本次
  package 发布扩大 browser admin surface。

## 10. 实施顺序

1. 协调复核 OpenAPI/official SDK/Wrangler pins，冻结 P16 输入；
2. 实现可供本地与 scheduled workflow 共用的 upstream scanner、classification fixture 和 three-way reports；
3. 实现 official SDK AST surface scanner 和 closure reports；
4. 把 extension package 直接改成 SDK package，生成 standard facade 与 `openCompute`；
5. 接入 strict options、dual-module build、type/runtime graph checks 与 package inspection；
6. 扩展一个现有 real-process SDK Gate；
7. 更新 website/package docs 与 combined OpenAPI；
8. 接入每周 upstream review、单一 tracking issue 与 frozen-identity Draft PR flow；
9. 更新 version/release manifest、CI 和 tag workflow；
10. 经明确 external-write 授权完成首包 bootstrap、Trusted Publishing 切换和首次联合 release；
11. 完成静态检查、coverage 和一次最终 workspace Gate，记录精确 npm/GitHub read-back evidence。

## 11. Definition of Done

P16 只有同时满足以下条件才可归档：

- `@open-compute/sdk` 从固定 authority 可重复生成并只暴露 qualified Cloudflare subset + `openCompute`；
- public runtime graph、types、autocomplete/docs 和 surface report 均无 unsupported module/resource/method；
- 所有 standard methods 委托给精确固定的 official SDK implementation，real `ocd` behavior Gate 通过；
- mandatory baseURL 防止任何请求默认发往 Cloudflare，credentials/headers 不泄漏；
- package tarball 的 ESM/CJS/types、dependency identity、内容 allowlist 与 isolated install smoke 通过；
- 同版本 tag release 同时发布并回读验证 npm package 与五个 GitHub assets；
- npm Trusted Publishing + provenance 生效，无长期 publish token；
- 每周 upstream review 能区分 `ready`、`blocked` 与 `breaking`，用 frozen identities 生成单一 tracking cycle/Draft PR，且不能自动
  merge、tag 或 publish；
- 文档、capability/deviation matrix、release notes 和 machine-readable identities 一致；
- 按仓库政策完成 coverage 与一次最终 workspace Gate。

## 12. 非目标

- 完整 Cloudflare API client 或 generic raw request escape hatch；
- 为 unsupported operation 生成“以后会用”的类型；
- Python、Go、Java、Terraform、CLI 或 MCP SDK；
- 把 SDK/deployer credential 暴露给 tenant Worker；
- build/startup 时联网更新 schema、SDK 或 package；
- 定时任务自动移动正式 pin、自动合并、自动 tag 或自动 release；
- 同时维护旧 extension package、旧 factory 或多套 version generation；
- 因 hosted codegen/docs 便利引入 Stainless build/release authority。
