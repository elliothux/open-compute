# Examples

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

`rows_read` / `rows_written` are local counters, not a bill. Config: [Get started](/en/d1/get-started/).
