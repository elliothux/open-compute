# Examples

Create a table, batch inserts, and query from a Worker. `rows_read` / `rows_written` are local SQLite execution counts.

## Create a table and query

```ts
export default {
  async fetch(_request: Request, env: Env): Promise<Response> {
    await env.DB.exec(
      "CREATE TABLE IF NOT EXISTS items (id INTEGER PRIMARY KEY, name TEXT);",
    );
    await env.DB.batch([
      env.DB.prepare("INSERT INTO items (name) VALUES (?)").bind("alpha"),
      env.DB.prepare("INSERT INTO items (name) VALUES (?)").bind("beta"),
    ]);
    const { results, meta } = await env.DB.prepare("SELECT * FROM items").all();
    return Response.json({ results, rows_read: meta.rows_read, rows_written: meta.rows_written });
  },
} satisfies ExportedHandler<Env>;
```

Config: [Get started](/en/d1/get-started/).
