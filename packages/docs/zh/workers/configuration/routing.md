# Routing

`oc run` 在已运行的平台上激活部署，然后打印该 Worker 的默认 `platform_path` URL（平台 origin + path prefix）。默认 origin 是 `http://127.0.0.1:8787`。

```sh
bun run oc run --config examples/hello-worker/open-compute.json --ocd "$PWD/target/debug/ocd"
# Worker is serving at http://127.0.0.1:8787/<path>
```

HTTP 请求到达绑定了该 Worker 的路径后，由 `fetch` handler 处理。Static Assets 的 HTML trailing-slash / SPA / Worker-first 路由概念与 [Cloudflare Static Assets routing](https://developers.cloudflare.com/workers/static-assets/) 对齐，见 [Static Assets](/zh/workers/static-assets/)。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| 绑定路径上的 HTTP 由 `fetch` 处理 | 是 | 是 |
| Static Assets HTML trailing-slash / SPA / Worker-first | 是 | 是，见 [Static Assets](/zh/workers/static-assets/) |
| [Custom Domains](https://developers.cloudflare.com/workers/configuration/routing/custom-domains/) | 是 | 不提供 |
| [workers.dev](https://developers.cloudflare.com/workers/configuration/routing/workers-dev/) | 是 | 不提供 |
| Cloudflare zone Routes / Page Rules | 是 | 不提供 |
| 项目文件中的 `routes` / `workers_dev` | 是 | 不允许 |
| 预览 URL | `*.workers.dev` / Cloudflare preview | `oc run` 打印的本机 URL |
| 部署与路由数据源 | Cloudflare 控制面 | 本机 SQLite；`ocd` 监督当前 workerd 进程 |

