---
title: "Routing"
---

`ocd wrangler deploy` 在所选平台上激活部署。每个 Worker 使用 canonical name 与 account ID 取得一个本机 origin。

```sh
ocd wrangler --project examples/hello-worker deploy --env dev
# Worker is serving at http://hello-worker.<account-id>.localhost:8787/
```

该 exact origin 下的所有 path 都属于 Worker，由 `fetch` 或 Static Assets 处理。可选的 [Gateway](/zh/docs/gateway/) 能把同一个 Worker 发布到 operator 管理的 HTTPS origin。Static Assets 的 HTML trailing-slash / SPA / Worker-first 路由概念与 [Cloudflare Static Assets routing](https://developers.cloudflare.com/workers/static-assets/) 对齐，见 [Static Assets](/zh/docs/workers/static-assets/)。

## 兼容性

| 主题                                                                                              | Cloudflare                      | open-compute                                             |
| ------------------------------------------------------------------------------------------------- | ------------------------------- | -------------------------------------------------------- |
| Worker origin 上的 HTTP 由 `fetch` 处理                                                           | 是                              | 是                                                       |
| Static Assets HTML trailing-slash / SPA / Worker-first                                            | 是                              | 是，见 [Static Assets](/zh/docs/workers/static-assets/)  |
| [Custom Domains](https://developers.cloudflare.com/workers/configuration/routing/custom-domains/) | 是                              | 不提供 Cloudflare API；使用 [Gateway](/zh/docs/gateway/) |
| [workers.dev](https://developers.cloudflare.com/workers/configuration/routing/workers-dev/)       | 是                              | 只提供本机 `.localhost` origin                           |
| Cloudflare zone Routes / Page Rules                                                               | 是                              | 不提供                                                   |
| 项目文件中的 `routes` / `workers_dev`                                                             | 是                              | 不允许                                                   |
| 公网 URL                                                                                          | `*.workers.dev` / Custom Domain | canonical 本机 origin 或 operator Gateway HTTPS origin   |
| 部署与路由数据源                                                                                  | Cloudflare 控制面               | 本机 SQLite；`ocd` 监督当前 workerd 进程                 |
