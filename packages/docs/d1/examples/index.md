# 示例

从 Worker 建表、batch 写入并查询。`rows_read` / `rows_written` 是本地 SQLite 执行计数。

## 建表并查询

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

配置见[上手](/d1/get-started/)。
