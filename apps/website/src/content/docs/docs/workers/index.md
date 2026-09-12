---
title: "Workers"
---

Workers is a serverless execution environment that runs Cloudflare module Workers on this platform. One `ocd` process supervises one pinned `workerd` child on the node. The platform does not provide a global edge, `workers.dev`, or a Cloudflare dashboard.

With Workers you can:

- Deploy a module Worker (`export default { fetch }`) with project-local Wrangler
- Bind KV, R2, D1, Durable Objects, Queues, Workflows, and other Workers
- Schedule `scheduled()` with UTC cron expressions
- Serve Static Assets from the same immutable deployment

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

The sample in this repository is `examples/hello-worker/`. Deploy it against a running `ocd` (default origin `http://127.0.0.1:8787`):

```sh
cd examples/hello-worker
ocd wrangler deploy --env dev
```

## Compatibility

| Topic                                                                   | Cloudflare               | open-compute                                                                               |
| ----------------------------------------------------------------------- | ------------------------ | ------------------------------------------------------------------------------------------ |
| Module Worker (`export default { fetch }`)                              | Yes                      | Yes                                                                                        |
| Isolates, `env` bindings, `fetch` / `scheduled` / `queue`               | Yes                      | Yes                                                                                        |
| Cache API, WebSocket hibernation, `cloudflare:sockets`, `node:` imports | Yes                      | Yes — same [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/) |
| Global Anycast / workers.dev / Custom Domains product                   | Yes                      | Not provided                                                                               |
| Project file                                                            | `wrangler.jsonc`         | Same pinned Wrangler schema                                                                |
| `compatibility_date`                                                    | Yes                      | Required and persisted per immutable Version                                               |
| Deploy authority                                                        | Cloudflare control plane | Local SQLite and one supervised runtime generation                                         |

## Next

- [Develop and deploy applications](/docs/develop/)
- Language examples: [Python](/docs/workers/languages/python/) and [Rust](/docs/workers/languages/rust/)
- [Project configuration](/docs/workers/configuration/) and [bindings](/docs/workers/configuration/bindings/)
- [Versions and deployments](/docs/workers/versions-and-deployments/)
- [Runtime APIs](/docs/workers/runtime-apis/), [Static Assets](/docs/workers/static-assets/), and [Cache](/docs/workers/cache/)
- [Compatibility and limits](/docs/reference/)

If the platform is not running yet, start at [Get started](/docs/get-started/).
