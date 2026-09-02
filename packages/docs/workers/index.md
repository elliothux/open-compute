# Workers

Workers is a serverless execution environment that runs Cloudflare module Workers on this platform. One `ocd` process supervises one pinned `workerd` child on the node. The platform does not provide a global edge, `workers.dev`, or a Cloudflare dashboard.

With Workers you can:

- Deploy a module Worker (`export default { fetch }`) with `oc run`
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
bun run oc run --config examples/hello-worker/open-compute.json --ocd <ocd-path>
```

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Module Worker (`export default { fetch }`) | Yes | Yes |
| Isolates, `env` bindings, `fetch` / `scheduled` / `queue` | Yes | Yes |
| Cache API, WebSocket hibernation, `cloudflare:sockets`, `node:` imports | Yes | Yes — same [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/) |
| Global Anycast / workers.dev / Custom Domains product | Yes | Not provided |
| Project file | wrangler.jsonc | `open-compute.json` (unknown fields rejected) |
| compatibilityDate in project JSON | Yes | Not allowed; frozen in the runtime lock (`2026-08-30`) |
| Deploy authority | Cloudflare control plane | Local SQLite and one supervised runtime generation |

## In this section

- [Get started](/workers/get-started/)
- [Concepts](/workers/concepts/)
- [Examples](/workers/examples/)
- [Configuration](/workers/configuration/) ([bindings](/workers/configuration/bindings), [compatibility dates](/workers/configuration/compatibility-dates), [flags](/workers/configuration/compatibility-flags), [Cron](/workers/configuration/cron-triggers), [environment variables](/workers/configuration/environment-variables), [secrets](/workers/configuration/secrets), [routing](/workers/configuration/routing))
- [Versions and deployments](/workers/versions-and-deployments/)
- [Static Assets](/workers/static-assets/)
- [Cache](/workers/cache/)
- [Runtime APIs](/workers/runtime-apis/) ([handlers](/workers/runtime-apis/handlers), [bindings](/workers/runtime-apis/bindings), [cache](/workers/runtime-apis/cache), [WebSockets](/workers/runtime-apis/websockets), [TCP](/workers/runtime-apis/tcp-sockets), [Node.js](/workers/runtime-apis/nodejs))
- [Limits](/workers/platform/limits) · [Known issues](/workers/platform/known-issues) · [Changelog](/workers/platform/changelog)

If the platform is not running yet, start at [ocd get started](/ocd/get-started).
