---
title: "Bindings (`env`)"
---

`env` 只包含部署声明过的名字。Version Metadata 是平台注入的只读对象：`id`、`tag`、`timestamp`。

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const version = env.VERSION.id;
    return env.AUTH.fetch(request);
  },
} satisfies ExportedHandler<Env>;
```

```json
{
  "name": "front",
  "main": "src/index.ts",
  "services": [{ "binding": "AUTH", "service": "auth-worker" }],
  "version_metadata": { "binding": "VERSION", "tag": "release-1" }
}
```

Service Binding：默认/具名 `fetch` 和 RPC。目标是同 instance、可解析的唯一 Worker 名、operator 配置的[扩展](/zh/docs/extension/) slug，或固定[私网 HTTP target](/zh/docs/ocd/configuration/)；部署时冻结 target identity 与 policy revision。可选 `entrypoint`。私网 HTTP target 只暴露 `fetch`。没有新的公开 Binding 类型。

KV / R2 / D1 / DO / Queue / Workflow / Assets / Images 的成员签名见各产品文档。配置语法见 [绑定](/zh/docs/workers/configuration/bindings/)。

## 兼容性

| 主题                                           | Cloudflare                                                                                                                                                                                   | open-compute                                                                                         |
| ---------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| `env.BINDING` 类型                             | 是，见 [Bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/) 与 [Service bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/) | 是                                                                                                   |
| Version Metadata 字段                          | 是，见 [version metadata](https://developers.cloudflare.com/workers/runtime-apis/bindings/version-metadata/)                                                                                 | `id`、`tag`、`timestamp`                                                                             |
| Service Bindings                               | 跨地域 placement / 全球 service discovery                                                                                                                                                    | 仅限本平台；默认/具名 fetch 与 RPC；调用方准入与部署钉扎均在本机判定；失败则关闭                     |
| Dynamic Workers / Worker Loader                | [Loader API](https://developers.cloudflare.com/dynamic-workers/api-reference/)                                                                                                               | 原生 `load/get`、模块、entrypoint/RPC、user tails、动态 DO facets 及显式/委托 limits；实验控制未开放 |
| Workers for Platforms dispatcher               | 是                                                                                                                                                                                           | 不提供                                                                                               |
| mTLS / Rate Limit / Secrets Store / AI binding | 是                                                                                                                                                                                           | 不提供                                                                                               |

## Dynamic Workers

声明 `worker_loaders: [{ binding: "LOADER" }]` 后，`env.LOADER` 使用原生 WorkerLoader API。
`get(id, callback)` 的同一 ID 必须对应不可变代码；不要依赖缓存命中或 callback 次数。
namespace 按账号、Script 和 binding 隔离；Version 回滚保留 namespace，删除后同名重建获得新的 namespace。
每个 Worker invocation 最多 4 个 distinct child，DO context 最多 10 个；同一 child 并发只计一次。

显式 `limits` 使用官方 `cpuMs` 和 `subRequests` 字段；`{}` 选择 Standard 默认值，child、entrypoint 与继续
委托的 Loader ceiling 按各维最严格值组合。CPU、内存、启动、子请求及同时 outbound connection 限额由固定
workerd fork 原生执行。非空 streaming tails 与两个 experimental-control members 仍未开放。Dynamic Python
child cold boot 因本地 Pyodide bootstrap 无法稳定满足官方 1 秒 startup CPU 限额而未取得资格；JavaScript、
Wasm、RPC、动态 Durable Object facets 与 limits 已取得资格。差异见[行为差异](/zh/docs/platform/deviations/)。

structured-clone 值与 Service Binding 可直接通过 `load({ env })` 传递。KV、D1、R2 与 Queue binding 应通过 open-compute helper 转发，以保留 binding boundary，而不是尝试 clone resource object：

```ts
import { loadWorker } from "open-compute:worker-loader";

const child = loadWorker(env.LOADER, {
  ...code,
  env: { CACHE: env.CACHE, DB: env.DB, BUCKET: env.BUCKET, QUEUE: env.QUEUE },
});
```

当前认证日期 `2026-09-08` 对应的固定 Pyodide bundle 随 `ocd` 内嵌，经校验后从实例私有 runtime
cache 加载；其它官方 child 日期/flag 组合保留 workerd 的原生版本选择。
已执行的 Version 仍持有 generation 后台引用时，
Script 删除返回 409；generation 结束后才可删除。本地自动收集 child 日志是平台能力，
并非 Cloudflare 默认将 child 日志写入 parent Workers Logs 的行为。
