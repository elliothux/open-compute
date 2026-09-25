---
title: "Workers"
---

在本机运行 Cloudflare 模块 Worker。`ocd` 为每个运行中的实例监督一个锁定版本的 `workerd`。平台提供自己的 operator Dashboard 与可选公网 [Gateway](/zh/docs/gateway/)，但不提供 Cloudflare 全球边缘、`workers.dev` 或托管控制面。

可以：

- 使用项目内 Wrangler 部署模块 Worker（`export default { fetch }`）
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
cd examples/hello-worker
ocd wrangler deploy --env dev
```

## 兼容性

| 主题                                                            | Cloudflare        | open-compute                                                                                  |
| --------------------------------------------------------------- | ----------------- | --------------------------------------------------------------------------------------------- |
| 模块 Worker（`export default { fetch }`）                       | 提供              | 提供                                                                                          |
| isolate、`env` 绑定、`fetch` / `scheduled` / `queue`            | 提供              | 提供                                                                                          |
| Cache API、WebSocket hibernation、`cloudflare:sockets`、`node:` | 提供              | 提供，与 [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/) 一致 |
| 全球 Anycast / workers.dev / Cloudflare Custom Domains API      | 提供              | 不提供；公网 HTTPS origin 使用 operator [Gateway](/zh/docs/gateway/)                          |
| 项目文件                                                        | `wrangler.jsonc`  | 使用相同的固定 Wrangler schema                                                                |
| `compatibility_date`                                            | 提供              | 必填，并按不可变 Version 持久化                                                               |
| 部署状态                                                        | Cloudflare 控制面 | 每个实例自己的 SQLite 和受监督 runtime generation                                             |

## 下一步

- [开发与部署应用](/zh/docs/develop/)
- 语言示例：[Python](/zh/docs/workers/languages/python/) 与 [Rust](/zh/docs/workers/languages/rust/)
- [项目配置](/zh/docs/workers/configuration/)与 [bindings](/zh/docs/workers/configuration/bindings/)
- [版本与部署](/zh/docs/workers/versions-and-deployments/)
- [Runtime APIs](/zh/docs/workers/runtime-apis/)、[Static Assets](/zh/docs/workers/static-assets/)和 [Cache](/zh/docs/workers/cache/)
- [日志与实时 Tail](/zh/docs/workers/observability/)
- [兼容性与限制](/zh/docs/reference/)

平台尚未启动时，从[快速开始](/zh/docs/get-started/)开始。
