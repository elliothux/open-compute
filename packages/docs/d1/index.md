# D1

D1 is a SQLite SQL database that you query from a Worker. On this platform, each database is a local-primary SQLite file on the node running ocd.

For example, you can use D1 for:

- Querying relational data from a Worker
- Importing a schema and running SQL
- Batching statements in one transaction

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

Bind an existing database with Wrangler's standard D1 field:

```json
{
  "name": "d1-app",
  "main": "src/index.ts",
  "d1_databases": [
    { "binding": "DB", "database_name": "app", "database_id": "<d1-database-id>" }
  ]
}
```

`id` is an existing database on this platform. Binding grammar: [bindings](/workers/configuration/bindings). The CLI is `oc` / `oc deploy` / `oc types`.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [D1 Worker API](https://developers.cloudflare.com/d1/worker-api/) | Same: `prepare` / `bind` / `run` / `all` / `first` / `raw` / `exec` / `batch`, sessions, opaque bookmarks, prepared-statement / result / meta |
| Topology | Hosted D1 with read replicas | Local primary SQLite on the node running ocd |
| Read replicas | Available | Not provided |
| Region routing | Available | Not provided |
| `served_by` geography | Region / colo metadata | Not provided; `served_by_*` is not a geography product |
| Bookmarks | Cross-replica causality | Local ordering on the same database |
| `rows_read` / `rows_written` | Billing counters | Local SQLite execution counts |
| `dump()` | Rejected on hosted non-alpha | Rejected (`D1_DUMP_ERROR`) |
| REST / `client/v4` | Available | Compatible account-scoped database and query operations |

## Next

- [Get started](/d1/get-started/)
- [Concepts](/d1/concepts/)
- [Guides](/d1/guides/)
- [Examples](/d1/examples/)
- [Limits](/d1/platform/limits)
- [Behavior differences](/d1/platform/deviations)
