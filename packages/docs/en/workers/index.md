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

- [Get started](/en/workers/get-started/)
- [Concepts](/en/workers/concepts/)
- [Examples](/en/workers/examples/)
- [Configuration](/en/workers/configuration/) ([bindings](/en/workers/configuration/bindings), [compatibility dates](/en/workers/configuration/compatibility-dates), [flags](/en/workers/configuration/compatibility-flags), [Cron](/en/workers/configuration/cron-triggers), [environment variables](/en/workers/configuration/environment-variables), [secrets](/en/workers/configuration/secrets), [routing](/en/workers/configuration/routing))
- [Versions and deployments](/en/workers/versions-and-deployments/)
- [Static Assets](/en/workers/static-assets/)
- [Cache](/en/workers/cache/)
- [Runtime APIs](/en/workers/runtime-apis/) ([handlers](/en/workers/runtime-apis/handlers), [bindings](/en/workers/runtime-apis/bindings), [cache](/en/workers/runtime-apis/cache), [WebSockets](/en/workers/runtime-apis/websockets), [TCP](/en/workers/runtime-apis/tcp-sockets), [Node.js](/en/workers/runtime-apis/nodejs))
- [Limits](/en/workers/platform/limits) · [Known issues](/en/workers/platform/known-issues) · [Changelog](/en/workers/platform/changelog)

If the platform is not running yet, start at [ocd get started](/en/ocd/get-started).
