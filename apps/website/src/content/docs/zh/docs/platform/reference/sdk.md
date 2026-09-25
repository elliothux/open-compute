---
title: "管理 SDK"
---

`@open-compute/sdk` 是 open-compute 管理 API 的 capability-scoped TypeScript SDK。它只暴露 `ocd` 已对照固定的官方 Cloudflare OpenAPI 快照取得资格的操作，open-compute 专有操作位于 `client.openCompute`。所有标准方法都委托给固定版本的官方 [`cloudflare`](https://www.npmjs.com/package/cloudflare) SDK 实现，认证、重试、分页、multipart 上传与错误解析与官方 client 完全一致。

## 安装

```sh
npm install @open-compute/sdk
```

SDK 的 `X.Y.Z` 与同版本的 `ocd` release 配对使用。

## 用法

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

## Client 规则

- `apiToken` 与 `baseURL` 必填；client 不读取环境变量中的隐式凭据。
- `baseURL` 必须是 canonical path 以 `/client/v4` 结尾的绝对 URL；HTTP 仅允许回环测试地址。
- `defaultHeaders` 不能覆盖 `Authorization` 或平台内部 header。
- 构造不联网，也绝不向 `api.cloudflare.com` 发送请求。

## Surface 清单

暴露面由仓库的 OpenAPI 权威生成，记录在仓库内提交的 surface report（`packages/sdk/surface.json`，并镜像到每个 release 的 `release.json`）。要点：

- Workers：scripts、versions、deployments、secrets、schedules、settings、script settings、tails、assets upload、subdomain。
- Worker Script/Version upload 使用一个 JSON `metadata` part 加具名 module part，并包含有类型的 Service `props`、Artifacts binding 与多步 Durable Object migration。历史 Version 删除使用官方 Beta 路径。
- KV、D1（含 time travel）、R2 objects、Queues（含 metrics 与 message push/bulk push）、Workflows、Vectorize、AI Search、memberships、user、accounts。
- `client.openCompute` 下的 vendor 操作：capabilities、system status、scheduler pause/resume/repair、cache garbage collection、image capacity、upgrade check、worker endpoints、durable object inventory、KV 与 D1 backups。

`ocd` 支持但固定版本官方 SDK 未实现的操作不会暴露；排除决策记录在 selection manifest 中。supported 操作的偏差见[行为差异](/zh/docs/platform/deviations/)。

## 错误

失败抛出官方 SDK 错误类（`APIError` 及其子类），已从 package re-export。
