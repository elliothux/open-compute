# Overview

Workers on this machine: one `ocd` process supervises one pinned `workerd` child and runs Cloudflare module Workers locally. There is no global edge, no `workers.dev`, and no Cloudflare dashboard or pricing.

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

The repo sample is `examples/hello-worker/`. Locally: `bun run oc run --config examples/hello-worker/open-compute.json --ocd <ocd-path>` against an already-running `ocd` (default `http://127.0.0.1:8787`).

## Same as Cloudflare

[Module Workers](https://developers.cloudflare.com/workers/reference/migrate-to-module-workers/): `export default { fetch }`, `satisfies ExportedHandler<Env>`. Isolates, `env` bindings, `fetch` / `scheduled` / `queue` handlers, the Cache API, WebSocket hibernation, `cloudflare:sockets`, and `node:` imports on the pinned baseline follow the same [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/). Do not paste member signatures here; use [Runtime APIs](/en/workers/runtime-apis/) and the Cloudflare pages.

## Intentional delta

One process, one machine, one SQLite authority. No Anycast, no global rollout, no Custom Domains / `workers.dev` product. The project file is `open-compute.json`, not a full `wrangler.jsonc`. Unknown fields are rejected. The project JSON cannot set `compatibilityDate` / `compatibilityFlags` — the platform freezes the compatibility date in the runtime lock (currently `2026-08-30`).

Registered deviations: [`OC-WKR-TCP-001`](/en/workers/runtime-apis/tcp-sockets), [`OC-WKR-LIMIT-001`](/en/workers/platform/limits), [`OC-DEPLOY-001`](/en/workers/versions-and-deployments/), [`OC-ASSETS-001`](/en/workers/static-assets/), [`OC-SERVICE-001`](/en/workers/runtime-apis/bindings), [`OC-CACHE-001`](/en/workers/cache/), [`OC-CACHE-002`](/en/workers/cache/), [`OC-CRON-001`](/en/workers/configuration/cron-triggers). Full text: [Deviations](/en/platform/deviations), [Compatibility](/en/platform/compatibility), and `docs/references/p1-deviations.md` in the repo.

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

If the platform is not up yet, start at [ocd get started](/en/ocd/get-started).
