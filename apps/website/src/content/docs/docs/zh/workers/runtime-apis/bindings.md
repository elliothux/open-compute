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

Service Binding：默认/具名 `fetch` 和 RPC。目标必须是同账户、可解析的唯一 Worker 名；部署时冻结为目标 ID。可选 `entrypoint`。

KV / R2 / D1 / DO / Queue / Workflow / Assets / Images 的成员签名见各产品文档。配置语法见 [绑定](/docs/zh/workers/configuration/bindings)。

## 兼容性

| 主题                                           | Cloudflare                                                                                                                                                                                   | open-compute                                                                                     |
| ---------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------ |
| `env.BINDING` 类型                             | 是，见 [Bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/) 与 [Service bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/) | 是                                                                                               |
| Version Metadata 字段                          | 是，见 [version metadata](https://developers.cloudflare.com/workers/runtime-apis/bindings/version-metadata/)                                                                                 | `id`、`tag`、`timestamp`                                                                         |
| Service Bindings                               | 跨地域 placement / 全球 service discovery                                                                                                                                                    | 仅限本平台；默认/具名 fetch 与 RPC；调用方准入与部署钉扎均在本机判定；失败则关闭                 |
| Dynamic Workers / Worker Loader                | [Loader API](https://developers.cloudflare.com/dynamic-workers/api-reference/)                                                                                                               | 原生 `load/get`、模块、entrypoint/RPC、user tails 和动态 DO facets；显式 limits 与实验控制未开放 |
| Workers for Platforms dispatcher               | 是                                                                                                                                                                                           | 不提供                                                                                           |
| mTLS / Rate Limit / Secrets Store / AI binding | 是                                                                                                                                                                                           | 不提供                                                                                           |

## Dynamic Workers

声明 `worker_loaders: [{ binding: "LOADER" }]` 后，`env.LOADER` 使用原生 WorkerLoader API。
`get(id, callback)` 的同一 ID 必须对应不可变代码；不要依赖缓存命中或 callback 次数。
namespace 按账号、Script 和 binding 隔离；Version 回滚保留 namespace，删除后同名重建获得新的 namespace。
每个 Worker invocation 最多 4 个 distinct child，DO context 最多 10 个；同一 child 并发只计一次。

显式 `limits`（包括 `{}`）被拒绝，CPU、内存和子请求预算执行尚未实现；非空 streaming tails 被拒绝，
不能开启实验能力。默认限制的差异见[行为差异](/docs/zh/platform/deviations)。
当前认证日期 `2026-09-08` 对应的固定 Pyodide bundle 随 `ocd` 内嵌，经校验后从实例私有 runtime
cache 加载；其它官方 child 日期/flag 组合保留 workerd 的原生版本选择。
已执行的 Version 仍持有 generation 后台引用时，
Script 删除返回 409；generation 结束后才可删除。本地自动收集 child 日志是平台能力，
并非 Cloudflare 默认将 child 日志写入 parent Workers Logs 的行为。
