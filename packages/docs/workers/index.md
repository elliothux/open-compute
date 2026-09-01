# 概述

这台机器上的 Workers：一个 `ocd` 进程监督一个 pinned `workerd` 子进程，在本机执行 Cloudflare 模块 Worker。没有全球边缘、没有 `workers.dev`、没有 Cloudflare 控制台或计费。

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

仓库示例是 `examples/hello-worker/`。本地用 `bun run oc run --config examples/hello-worker/open-compute.json --ocd <ocd-path>`，对着已经在跑的 `ocd`（默认 `http://127.0.0.1:8787`）。

## 与 Cloudflare 相同

[模块 Worker](https://developers.cloudflare.com/workers/reference/migrate-to-module-workers/)：`export default { fetch }`，`satisfies ExportedHandler<Env>`。isolate、`env` binding、`fetch` / `scheduled` / `queue` handler、Cache API、WebSocket hibernation、`cloudflare:sockets`、pinned baseline 的 `node:` 导入，都按 [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/) 的同一套表面工作。完整成员不要从本页抄，去 [Runtime APIs](/workers/runtime-apis/) 和 Cloudflare 原文。

## 故意不同

一个进程、一台机器、一份 SQLite 权威。没有 Anycast、没有全球 rollout、没有 Custom Domains / `workers.dev` 产品。项目文件是 `open-compute.json`，不是完整 `wrangler.jsonc`。未知字段直接拒绝。项目 JSON 不能写 `compatibilityDate` / `compatibilityFlags`——平台把 compatibility date 冻在 runtime lock（当前 `2026-08-30`）。

登记偏差：[`OC-WKR-TCP-001`](/workers/runtime-apis/tcp-sockets)、[`OC-WKR-LIMIT-001`](/workers/platform/limits)、[`OC-DEPLOY-001`](/workers/versions-and-deployments/)、[`OC-ASSETS-001`](/workers/static-assets/)、[`OC-SERVICE-001`](/workers/runtime-apis/bindings)、[`OC-CACHE-001`](/workers/cache/)、[`OC-CACHE-002`](/workers/cache/)、[`OC-CRON-001`](/workers/configuration/cron-triggers)。全文见 [偏差](/platform/deviations)、[兼容性](/platform/compatibility) 和仓库 `docs/references/p1-deviations.md`。

## 本节

- [上手](/workers/get-started/)
- [概念](/workers/concepts/)
- [示例](/workers/examples/)
- [配置](/workers/configuration/)（[绑定](/workers/configuration/bindings)、[兼容日期](/workers/configuration/compatibility-dates)、[兼容标志](/workers/configuration/compatibility-flags)、[Cron](/workers/configuration/cron-triggers)、[环境变量](/workers/configuration/environment-variables)、[密钥](/workers/configuration/secrets)、[路由](/workers/configuration/routing)）
- [版本与部署](/workers/versions-and-deployments/)
- [静态资源](/workers/static-assets/)
- [缓存](/workers/cache/)
- [运行时 API](/workers/runtime-apis/)（[handlers](/workers/runtime-apis/handlers)、[bindings](/workers/runtime-apis/bindings)、[cache](/workers/runtime-apis/cache)、[WebSockets](/workers/runtime-apis/websockets)、[TCP](/workers/runtime-apis/tcp-sockets)、[Node.js](/workers/runtime-apis/nodejs)）
- [限制](/workers/platform/limits) · [已知问题](/workers/platform/known-issues) · [更新日志](/workers/platform/changelog)

平台还没起来时，先看 [ocd 上手](/ocd/get-started)。
