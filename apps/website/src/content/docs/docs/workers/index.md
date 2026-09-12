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

## In this section

- [Get started](/docs/workers/get-started/)
- [Concepts](/docs/workers/concepts/)
- [Examples](/docs/workers/examples/)
- [Wrangler projects and deployment targets](/docs/workers/projects)
- [Configuration](/docs/workers/configuration/) ([bindings](/docs/workers/configuration/bindings), [compatibility dates](/docs/workers/configuration/compatibility-dates), [flags](/docs/workers/configuration/compatibility-flags), [Cron](/docs/workers/configuration/cron-triggers), [environment variables](/docs/workers/configuration/environment-variables), [secrets](/docs/workers/configuration/secrets), [routing](/docs/workers/configuration/routing))
- [Versions and deployments](/docs/workers/versions-and-deployments/)
- [Static Assets](/docs/workers/static-assets/)
- [Cache](/docs/workers/cache/)
- [Runtime APIs](/docs/workers/runtime-apis/) ([handlers](/docs/workers/runtime-apis/handlers), [bindings](/docs/workers/runtime-apis/bindings), [cache](/docs/workers/runtime-apis/cache), [WebSockets](/docs/workers/runtime-apis/websockets), [TCP](/docs/workers/runtime-apis/tcp-sockets), [Node.js](/docs/workers/runtime-apis/nodejs))
- [Limits](/docs/workers/platform/limits) · [Known issues](/docs/workers/platform/known-issues) · [Changelog](/docs/workers/platform/changelog)

If the platform is not running yet, start at [ocd get started](/docs/ocd/get-started).
