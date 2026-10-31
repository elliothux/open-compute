# P16：Capability-scoped Cloudflare-compatible TypeScript SDK

状态：**implemented and verified（2026-09-16）**。GitHub issue
[#68](https://github.com/elliothux/open-compute/issues/68) 已关闭；`@open-compute/sdk@0.1.9` 已随
open-compute 0.1.9 发布到 npm，并完成 registry identity 回读。

## 1. 用户结果

`@open-compute/sdk` 是 open-compute 的正式 TypeScript 管理面客户端：

- `createOpenComputeClient({ apiToken, baseURL })` 返回 capability-scoped client；
- Cloudflare-compatible 方法沿用固定官方 SDK 的 resource、method、参数、返回值、分页、上传、重试和
  `APIError` 行为；
- open-compute 专有操作只位于 `client.openCompute`；
- 0.1.9 公开 140 个 standard operations 和 19 个 vendor operations；
- pre-1.0 只对相同 `X.Y.Z` 的 SDK 与 `ocd` 组合声明资格。

```ts
import { createOpenComputeClient } from "@open-compute/sdk";

const client = createOpenComputeClient({
  apiToken: process.env.OPEN_COMPUTE_API_TOKEN!,
  baseURL: "https://compute.example/client/v4",
});

await client.workers.scripts.versions.list("app", { account_id });
await client.openCompute.system.status();
```

`apiToken` 和 `baseURL` 必须显式提供。`baseURL` 必须是无 userinfo、query 或 fragment 的绝对 URL，canonical
path 精确结束于 `/client/v4`；普通 HTTP 只允许 loopback。SDK 不读取隐式 Cloudflare credential 环境变量，也不允许
`defaultHeaders` 覆盖 `Authorization` 或 `x-open-compute-*` 内部 header。

## 2. Capability 与生成 authority

SDK 不导出完整 `Cloudflare` client。公开 graph 是无继承 facade，每个 node 只包含已选中的 bound method 和 child node；
隐藏的官方 SDK instance 只作为 delegate。因此 runtime properties、TypeScript types、autocomplete、文档和 surface report
使用同一 closed operation set，且请求仍由固定官方 transport 和 method implementation 发出。

权威输入保持单一：

| 内容                            | Authority                                                          |
| ------------------------------- | ------------------------------------------------------------------ |
| 上游 identities                 | `openapi/upstream/cloudflare-openapi.lock.json`                    |
| Cloudflare operation 选择与状态 | `openapi/cloudflare-subset-manifest.json`                          |
| Cloudflare wire schema          | `openapi/cloudflare-v4-subset.json`                                |
| vendor routes/schema            | `openapi/open-compute-extension.json`                              |
| capability projection           | `openapi/p6-capability.json`、`share/cloudflare-capabilities.json` |
| official implementation/types   | lock 精确固定的 `cloudflare` npm tarball                           |

`packages/sdk/scripts/generate.ts` 和 `scripts/sdk-scan.ts` 离线生成并校验：

- `packages/sdk/src/generated.ts`：closed facade、`openCompute` 和可达 public types；
- `packages/sdk/surface.json`：operation、delegate identity 和 digest；
- `openapi/open-compute-sdk.json`：standard subset 与 vendor extension 的 combined OpenAPI。

生成器用 TypeScript compiler API 静态解析官方 package，不执行 package source。missing、ambiguous、dynamic route 或
OpenAPI/component 冲突均 fail closed；`--check` 对 committed 生成物做逐字节 drift 检查。未被官方 SDK 实现的已选 route
进入 `sdkExcludedOperations`，不由 facade 手工伪造。

固定输入的长期维护由[Cloudflare 上游刷新](../references/cloudflare-upstream-refresh.md)负责。定时任务只发现并分类
`ready`、`blocked`、`breaking` 候选；不会自动移动 pin、merge、tag 或 publish。

## 3. Package 与发布合同

`packages/sdk` 发布 public scoped package `@open-compute/sdk`：

- version 与同一 tag 的 workspace/`ocd` `X.Y.Z` 精确一致；
- `cloudflare` 是 lock 固定的 exact runtime dependency；
- 发布 ESM、CommonJS 和 declarations，支持 Node.js 20+ 与 Bun 1+；
- package 无 install/prepare/postinstall script，tarball 只包含发行所需文件；
- `packages/cloudflare-extension` 和旧 `createOpenComputeExtension` 已删除，不保留兼容 alias。

正式 tag 在 clean checkout 构建、检查并冻结 tarball。全部 qualification 通过后，发布顺序固定为：

```text
create and verify GitHub Draft
  -> publish exact npm tarball
  -> read back npm shasum and integrity
  -> publish the verified GitHub Draft
```

`release.json` 记录 package/version、tarball identities、surface digest、OpenAPI revision 和 official SDK version。
GitHub Release 仍只有三个 executable、`release.json` 与 `SHA256SUMS`；SDK tarball 由 npm 分发。

0.1.9 使用 release environment 中已授权的 `NPM_ACCESS_TOKEN`。token 只进入 runner 私有临时 npm 配置，不进入
checkout、artifact、日志或 package；该流程不声明 npm provenance。当前发布与恢复语义由
[发布流程](../references/releasing.md)持续维护。

## 4. 安全与支持边界

- SDK 只用于可信管理面，不注入 tenant Worker，也不向 tenant code 暴露 deployer/admin token；
- 不公开 generic raw request、完整 official namespace、unsupported sibling/leaf method 或不可达 resource types；
- build、daemon startup 和 client construction 均不联网更新 schema、SDK 或 package；
- vendor operation 成为已验证的官方 API 后直接迁移到 official resource/method，不保留重复调用路径；
- Dashboard、cf 和 tenant bindings 不以该 SDK 作为新的权限边界。

## 5. 验收证据

0.1.9 冻结输入为 `cloudflare@7.1.0`、Cloudflare OpenAPI revision
`b8687f42e28fbfcb296a350f7dbf16349ea900af`，SDK 与 `ocd` version 均为 `0.1.9`。

发布候选通过：

- generator byte-drift、surface 双向闭包、runtime/type graph 与 negative compile fixtures；
- ESM/CJS/declarations build、tarball allowlist、exact dependency、无 secret 检查；
- 隔离 Node ESM、Node CommonJS、Bun 和 TypeScript consumer smoke；
- real-process SDK Gate，覆盖 JSON、multipart upload、binary download、pagination、bodyless POST、retry、timeout、
  `APIError`、path encoding、vendor methods 和 official-client request-trace equivalence；
- 仓库静态检查、90.03% Rust line coverage，以及一次 52-process/1532-case workspace Gate；
- npm package shasum/integrity 与冻结 tarball 一致，GitHub 五个 release assets 完成回读。

完整版本输入、公开变更和当时验证结果见 [0.1.9 release notes](../releases/0.1.9.md)。历史 PASS 只证明该冻结候选；
当前支持面以源码、机器可读 authority 和[兼容矩阵](../references/cloudflare-compatibility.md)为准。
