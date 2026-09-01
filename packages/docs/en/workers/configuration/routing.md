# Routing

Local routes: `oc run` activates a deployment on the already-running platform, then prints that Worker's default `platform_path` URL (platform origin + path prefix). Default origin: `http://127.0.0.1:8787`.

```sh
bun run oc run --config examples/hello-worker/open-compute.json --ocd "$PWD/target/debug/ocd"
# Worker is serving at http://127.0.0.1:8787/<path>
```

There is no Custom Domains product and no `workers.dev` subdomain product.

## Same as Cloudflare

HTTP requests that hit the path bound to the Worker are handled by `fetch`. Static Assets HTML trailing-slash / SPA / Worker-first routing concepts match [Cloudflare Static Assets routing](https://developers.cloudflare.com/workers/static-assets/); see [Static Assets](/en/workers/static-assets/).

## Intentional delta

No [Custom Domains](https://developers.cloudflare.com/workers/configuration/routing/custom-domains/), no [workers.dev](https://developers.cloudflare.com/workers/configuration/routing/workers-dev/), no Cloudflare zone Routes / Page Rules. `open-compute.json` has no `routes` / `workers_dev` fields; adding them fails. The preview URL is the local URL printed by `oc run`, not `*.workers.dev` or a CF preview product. Deployment and route authority is local SQLite (`OC-DEPLOY-001`).
