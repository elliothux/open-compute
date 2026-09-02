# Workers

在本机运行 Cloudflare 模块 Worker。`ocd` 启动锁定版本的 `workerd`。不提供全球边缘网络、`workers.dev` 或 Cloudflare 控制台。

可以：

- 使用 `oc deploy` 部署模块 Worker（`export default { fetch }`）
- 绑定 KV、R2、D1、Durable Objects、Queues、Workflows 以及其他 Worker
- 使用 UTC cron 触发 `scheduled()`
- 在同一份部署中提供静态资源

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

仓库示例为 `examples/hello-worker/`。针对已运行的 `ocd` 部署（默认 origin `http://127.0.0.1:8787`）：

```sh
bun run oc deploy --config examples/hello-worker/wrangler.jsonc
```

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| 模块 Worker（`export default { fetch }`） | 提供 | 提供 |
| isolate、`env` 绑定、`fetch` / `scheduled` / `queue` | 提供 | 提供 |
| Cache API、WebSocket hibernation、`cloudflare:sockets`、`node:` | 提供 | 提供，与 [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/) 一致 |
| 全球 Anycast / workers.dev / 自定义域名产品 | 提供 | 不提供 |
| 项目文件 | `wrangler.jsonc` | 使用相同的固定 Wrangler schema |
| `compatibility_date` | 提供 | 必填，并按不可变 Version 持久化 |
| 部署状态 | Cloudflare 控制面 | 本机 SQLite；`ocd` 监督当前 `workerd` 进程 |

## 本节

- [快速开始](/zh/workers/get-started/)
- [概念](/zh/workers/concepts/)
- [示例](/zh/workers/examples/)
- [配置](/zh/workers/configuration/)（[绑定](/zh/workers/configuration/bindings)、[兼容日期](/zh/workers/configuration/compatibility-dates)、[兼容标志](/zh/workers/configuration/compatibility-flags)、[Cron](/zh/workers/configuration/cron-triggers)、[环境变量](/zh/workers/configuration/environment-variables)、[密钥](/zh/workers/configuration/secrets)、[路由](/zh/workers/configuration/routing)）
- [版本与部署](/zh/workers/versions-and-deployments/)
- [静态资源](/zh/workers/static-assets/)
- [缓存](/zh/workers/cache/)
- [运行时 API](/zh/workers/runtime-apis/)（[handlers](/zh/workers/runtime-apis/handlers)、[bindings](/zh/workers/runtime-apis/bindings)、[cache](/zh/workers/runtime-apis/cache)、[WebSockets](/zh/workers/runtime-apis/websockets)、[TCP](/zh/workers/runtime-apis/tcp-sockets)、[Node.js](/zh/workers/runtime-apis/nodejs)）
- [限制](/zh/workers/platform/limits) · [已知问题](/zh/workers/platform/known-issues) · [更新日志](/zh/workers/platform/changelog)

若尚未运行 `ocd`，见 [ocd 安装](/zh/ocd/get-started)。
