# D1

D1 is a SQLite SQL database bound onto Worker `env`. On this platform each database is a local-primary SQLite file on this one machine. There are no read replicas and no region routing.

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const { results } = await env.DB.prepare(
      "SELECT * FROM Customers WHERE CompanyName = ?",
    ).bind("Bs Beverages").all();
    return Response.json(results);
  },
} satisfies ExportedHandler<{ DB: D1Database }>;
```

## Same as Cloudflare

The Worker API is the [D1 Worker API](https://developers.cloudflare.com/d1/worker-api/): `prepare` / `bind` / `run` / `all` / `first` / `raw` / `exec` / `batch`, sessions, opaque bookmarks, prepared-statement / result / meta. 36 target members are `supported_with_deviation`. The current hosted non-alpha `dump()` is rejected, matching hosted behavior.

```json
{
  "name": "d1-app",
  "main": "src/index.ts",
  "bindings": {
    "DB": { "type": "d1_database", "id": "<d1-database-id>" }
  }
}
```

`id` is an already-existing database on the platform. Binding grammar: [bindings](/en/workers/configuration/bindings). Do not copy Cloudflare REST or Wrangler `d1` subcommands from this page.

## Intentional differences

**`OC-D1-001`**: D1 is a single local-primary SQLite authority. The platform does not claim read-replica/region routing, hosted `served_by` identity, region/colo metadata, or Cloudflare billing counters. Opaque bookmarks preserve same-database local sequential visibility; `rows_read` and `rows_written` are stable local SQLite execution counters. There are no replicas; do not read `served_by_region` / `served_by_colo` as a geography product.

Full text: [Deviations](/en/d1/platform/deviations) and [Compatibility](/en/platform/compatibility).

## In this section

- [Get started](/en/d1/get-started/)
- [Concepts](/en/d1/concepts/)
- [Guides](/en/d1/guides/)
- [Examples](/en/d1/examples/)
- [Limits](/en/d1/platform/limits)
- [Deviations](/en/d1/platform/deviations)
