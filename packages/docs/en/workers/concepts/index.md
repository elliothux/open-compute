# Concepts

Workers run in isolates: a V8 sandbox, with code and `env` frozen per deployment, hosted by one `workerd` child on the node. One `ocd` owns one data-dir and one workerd child. Do not start a second `ocd` on the same data-dir.

```ts
export default {
  fetch(request: Request, env: Env, ctx: ExecutionContext): Response {
    ctx.waitUntil(Promise.resolve());
    return new Response(env.GREETING);
  },
} satisfies ExportedHandler<Env>;
```

`env` exposes only the vars, secrets, and bindings declared on the deployment. Tenants cannot access SQLite paths, S3 credentials, or another account's resources.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Isolates (not containers) | Yes — [How Workers works](https://developers.cloudflare.com/workers/reference/how-workers-works/) | Yes |
| Module Workers; bindings injected as `env`; handlers receive `request` / `env` / `ctx` | Yes | Yes |
| Compatibility date selects runtime behavior | Yes — [compatibility dates](https://developers.cloudflare.com/workers/configuration/compatibility-dates/) | Yes; the date is frozen by the platform runtime lock |
| `compatibilityDate` / `compatibilityFlags` in project JSON | Yes | Not allowed; current `effective_compatibility_date` is `2026-08-30` |
| Global edge / colo / Smart Placement | Yes | Not provided |
| Outbound network | Cloudflare hosted network policy | Tenant general outbound shares stock workerd's one `Network(allow = ["public"])`; see [TCP sockets](/en/workers/runtime-apis/tcp-sockets) |
| Request-scoped CPU / subrequest quotas | Yes | Not enforced by stock OSS workerd `LimitEnforcer`; live numbers on [Limits](/en/workers/platform/limits) |

Next: [Configuration](/en/workers/configuration/), [Runtime APIs](/en/workers/runtime-apis/).
