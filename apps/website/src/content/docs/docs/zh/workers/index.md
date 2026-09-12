---
title: "Workers"
---

在本机运行 Cloudflare 模块 Worker。`ocd` 启动锁定版本的 `workerd`。不提供全球边缘网络、`workers.dev` 或 Cloudflare 控制台。

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
| 全球 Anycast / workers.dev / 自定义域名产品                     | 提供              | 不提供                                                                                        |
| 项目文件                                                        | `wrangler.jsonc`  | 使用相同的固定 Wrangler schema                                                                |
| `compatibility_date`                                            | 提供              | 必填，并按不可变 Version 持久化                                                               |
| 部署状态                                                        | Cloudflare 控制面 | 本机 SQLite；`ocd` 监督当前 `workerd` 进程                                                    |

## 下一步

- [开发与部署应用](/docs/zh/develop/)
- 语言示例：[Python](/docs/zh/workers/languages/python/) 与 [Rust](/docs/zh/workers/languages/rust/)
- [项目配置](/docs/zh/workers/configuration/)与 [bindings](/docs/zh/workers/configuration/bindings/)
- [版本与部署](/docs/zh/workers/versions-and-deployments/)
- [Runtime APIs](/docs/zh/workers/runtime-apis/)、[Static Assets](/docs/zh/workers/static-assets/)和 [Cache](/docs/zh/workers/cache/)
- [兼容性与限制](/docs/zh/reference/)

平台尚未启动时，从[快速开始](/docs/zh/get-started/)开始。
