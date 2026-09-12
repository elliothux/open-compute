---
title: "Routing"
---

`ocd wrangler deploy` activates a deployment on the selected platform. The default local origin is `http://127.0.0.1:8787`; the Worker serves on its bound `platform_path`.

```sh
ocd wrangler --project examples/hello-worker deploy --env dev
# Worker is serving at http://127.0.0.1:8787/<path>
```

HTTP requests that hit the path bound to the Worker are handled by `fetch`. Static Assets HTML trailing-slash / SPA / Worker-first routing concepts match [Cloudflare Static Assets routing](https://developers.cloudflare.com/workers/static-assets/); see [Static Assets](/docs/workers/static-assets/).

## Compatibility

| Topic                                                                                             | Cloudflare                           | open-compute                                        |
| ------------------------------------------------------------------------------------------------- | ------------------------------------ | --------------------------------------------------- |
| HTTP on the bound path is handled by `fetch`                                                      | Yes                                  | Yes                                                 |
| Static Assets HTML trailing-slash / SPA / Worker-first                                            | Yes                                  | Yes — [Static Assets](/docs/workers/static-assets/) |
| [Custom Domains](https://developers.cloudflare.com/workers/configuration/routing/custom-domains/) | Yes                                  | Not provided                                        |
| [workers.dev](https://developers.cloudflare.com/workers/configuration/routing/workers-dev/)       | Yes                                  | Not provided                                        |
| Cloudflare zone Routes / Page Rules                                                               | Yes                                  | Not provided                                        |
| `routes` / `workers_dev` in the project file                                                      | Yes                                  | Not allowed                                         |
| Preview URL                                                                                       | `*.workers.dev` / Cloudflare preview | Not provided; invoke the configured platform path   |
| Deployment and route authority                                                                    | Cloudflare control plane             | Local SQLite and one supervised runtime generation  |
