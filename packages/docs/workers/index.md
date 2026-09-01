# Workers

Workers 是在本平台上执行 Cloudflare 模块 Worker 的无服务器运行环境。一个 `ocd` 进程在该节点上监督一个 pinned `workerd` 子进程。本平台不提供全球边缘、`workers.dev` 或 Cloudflare 控制台。

使用 Workers 可以：

- 通过 `oc run` 部署模块 Worker（`export default { fetch }`）
- 绑定 KV、R2、D1、Durable Objects、Queues、Workflows 以及其他 Worker
- 使用 UTC cron 表达式调度 `scheduled()`
- 在同一份不可变部署中提供 Static Assets

```ts
export default {
  fetch(request: Request, env: Env): Response {
    return Response.json({
      message: env.GREETING,
      pathname: new URL(request.url).pathname,
    });
  },
} satisfies ExportedHandler<Env>;
```

仓库示例位于 `examples/hello-worker/`。在已运行的 `ocd` 上部署（默认 origin 为 `http://127.0.0.1:8787`）：

```sh
bun run oc run --config examples/hello-worker/open-compute.json --ocd <ocd-path>
```

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| 模块 Worker（`export default { fetch }`） | 是 | 是 |
| isolate、`env` binding、`fetch` / `scheduled` / `queue` | 是 | 是 |
| Cache API、WebSocket hibernation、`cloudflare:sockets`、`node:` 导入 | 是 | 是，表面与 [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/) 对齐 |
| 全球 Anycast / workers.dev / Custom Domains 产品 | 是 | 不提供 |
| 项目文件 | wrangler.jsonc | `open-compute.json`（未知字段拒绝） |
| 项目 JSON 中的 compatibilityDate | 是 | 不允许；冻结在 runtime lock（`2026-08-30`） |
| 部署权威 | Cloudflare 控制面 | 本节点 SQLite 与一份受监督的 runtime generation |

## 本节

- [快速开始](/workers/get-started/)
- [概念](/workers/concepts/)
- [示例](/workers/examples/)
- [配置](/workers/configuration/)（[绑定](/workers/configuration/bindings)、[兼容日期](/workers/configuration/compatibility-dates)、[兼容标志](/workers/configuration/compatibility-flags)、[Cron](/workers/configuration/cron-triggers)、[环境变量](/workers/configuration/environment-variables)、[密钥](/workers/configuration/secrets)、[路由](/workers/configuration/routing)）
- [版本与部署](/workers/versions-and-deployments/)
- [静态资源](/workers/static-assets/)
- [缓存](/workers/cache/)
- [运行时 API](/workers/runtime-apis/)（[handlers](/workers/runtime-apis/handlers)、[bindings](/workers/runtime-apis/bindings)、[cache](/workers/runtime-apis/cache)、[WebSockets](/workers/runtime-apis/websockets)、[TCP](/workers/runtime-apis/tcp-sockets)、[Node.js](/workers/runtime-apis/nodejs)）
- [限制](/workers/platform/limits) · [已知问题](/workers/platform/known-issues) · [更新日志](/workers/platform/changelog)

平台尚未启动时，见 [ocd 上手](/ocd/get-started)。
