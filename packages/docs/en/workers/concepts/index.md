# Concepts

Workers run in isolates: a V8 sandbox, code and `env` frozen per deployment, hosted by this machine's one `workerd` child. One `ocd` owns one data-dir and one workerd child. Do not start a second `ocd` on the same data-dir.

```ts
export default {
  fetch(request: Request, env: Env, ctx: ExecutionContext): Response {
    ctx.waitUntil(Promise.resolve());
    return new Response(env.GREETING);
  },
} satisfies ExportedHandler<Env>;
```

`env` exposes only vars, secrets, and bindings declared on the deployment. Tenants do not get SQLite paths, S3 credentials, or anyone else's resources.

## Same as Cloudflare

[How Workers works](https://developers.cloudflare.com/workers/reference/how-workers-works/): isolates rather than containers; module Workers; bindings injected as `env`; handlers receive `request` / `env` / `ctx`. The compatibility date selects runtime behavior — see Cloudflare [compatibility dates](https://developers.cloudflare.com/workers/configuration/compatibility-dates/).

## Intentional delta

The compatibility date is frozen by the platform runtime lock. The current `effective_compatibility_date` is `2026-08-30`. Project JSON cannot set `compatibilityDate` or `compatibilityFlags`. There is no global edge, no colo, no Smart Placement. Outbound traffic shares stock workerd's one `Network(allow = ["public"])`; see [`OC-WKR-TCP-001`](/en/workers/runtime-apis/tcp-sockets). CPU / subrequest quotas: [`OC-WKR-LIMIT-001`](/en/workers/platform/limits).

Next: [Configuration](/en/workers/configuration/), [Runtime APIs](/en/workers/runtime-apis/).
