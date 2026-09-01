# Routing

本地路由：`oc run` 在已经运行的平台上激活部署，然后打印该 Worker 的默认 `platform_path` URL（平台 origin + path prefix）。默认 origin 是 `http://127.0.0.1:8787`。

```sh
bun run oc run --config examples/hello-worker/open-compute.json --ocd "$PWD/target/debug/ocd"
# Worker is serving at http://127.0.0.1:8787/<path>
```

没有 Custom Domains 产品，没有 `workers.dev` 子域产品。

## 与 Cloudflare 相同

HTTP 请求打到绑定了该 Worker 的路径，由 `fetch` handler 处理。Static Assets 的 HTML trailing-slash / SPA / Worker-first 路由概念与 [Cloudflare Static Assets routing](https://developers.cloudflare.com/workers/static-assets/) 对齐，见 [Static Assets](/workers/static-assets/)。

## 故意不同

没有 [Custom Domains](https://developers.cloudflare.com/workers/configuration/routing/custom-domains/)、没有 [workers.dev](https://developers.cloudflare.com/workers/configuration/routing/workers-dev/)、没有 Cloudflare zone Routes / Page Rules。`open-compute.json` 没有 `routes` / `workers_dev` 字段；写进去会拒绝。预览 URL 就是 `oc run` 打印的本机 URL，不是 `*.workers.dev` 或 CF preview 产品。部署与路由权威是本机 SQLite（`OC-DEPLOY-001`）。
