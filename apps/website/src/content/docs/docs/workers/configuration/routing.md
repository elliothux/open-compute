---
title: "Routing"
---

`ocd wrangler deploy` activates a deployment on the selected platform. Each Worker receives a local-machine origin using its canonical name and account ID.

```sh
ocd wrangler --project examples/hello-worker deploy --env dev
# Worker is serving at http://hello-worker.<account-id>.localhost:8787/
```

All paths on that exact origin belong to the Worker and are handled by `fetch` or Static Assets. The optional [Gateway](/docs/gateway/) can publish the same Worker at an operator-owned HTTPS origin. Static Assets HTML trailing-slash / SPA / Worker-first routing concepts match [Cloudflare Static Assets routing](https://developers.cloudflare.com/workers/static-assets/); see [Static Assets](/docs/workers/static-assets/).

## Compatibility

| Topic                                                                                             | Cloudflare                      | open-compute                                                   |
| ------------------------------------------------------------------------------------------------- | ------------------------------- | -------------------------------------------------------------- |
| HTTP on the Worker origin is handled by `fetch`                                                   | Yes                             | Yes                                                            |
| Static Assets HTML trailing-slash / SPA / Worker-first                                            | Yes                             | Yes — [Static Assets](/docs/workers/static-assets/)            |
| [Custom Domains](https://developers.cloudflare.com/workers/configuration/routing/custom-domains/) | Yes                             | Cloudflare API not provided; use the [Gateway](/docs/gateway/) |
| [workers.dev](https://developers.cloudflare.com/workers/configuration/routing/workers-dev/)       | Yes                             | Local `.localhost` origin only                                 |
| Cloudflare zone Routes / Page Rules                                                               | Yes                             | Not provided                                                   |
| `routes` / `workers_dev` in the project file                                                      | Yes                             | Not allowed                                                    |
| Public URL                                                                                        | `*.workers.dev` / Custom Domain | Canonical local origin or operator Gateway HTTPS origin        |
| Deployment and route authority                                                                    | Cloudflare control plane        | Local SQLite and one supervised runtime generation             |
