# D1

D1 是绑到 Worker `env` 的 SQLite SQL 数据库。本平台上，每个 database 是这一台机器上的一份本地主 SQLite。没有 read replica，没有 region routing。

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

## 与 Cloudflare 相同

Worker API 与 [D1 Worker API](https://developers.cloudflare.com/d1/worker-api/) 相同：`prepare` / `bind` / `run` / `all` / `first` / `raw` / `exec` / `batch`、session、opaque bookmark、prepared-statement / result / meta。36 个目标成员为 `supported_with_deviation`。当前 hosted 非 alpha `dump()` 按托管行为拒绝。

```json
{
  "name": "d1-app",
  "main": "src/index.ts",
  "bindings": {
    "DB": { "type": "d1_database", "id": "<d1-database-id>" }
  }
}
```

`id` 是平台上已存在的 database。绑定语法见 [bindings](/workers/configuration/bindings)。不要从本页抄 Cloudflare REST 或 Wrangler `d1` 子命令。

## 故意不同

**`OC-D1-001`**：D1 是单个本地主 SQLite authority，不声称 read replica、region routing、hosted `served_by` 身份、region/colo metadata 或 Cloudflare 计费计数。opaque bookmark 保证同一数据库的本地顺序可见性；`rows_read` / `rows_written` 是稳定的本地 SQLite 执行计数。没有 replicas，不要读 `served_by_region` / `served_by_colo` 当地理产品。

全文见 [偏差](/d1/platform/deviations) 和 [Compatibility](/platform/compatibility)。

## 本节

- [上手](/d1/get-started/)
- [概念](/d1/concepts/)
- [指南](/d1/guides/)
- [示例](/d1/examples/)
- [限制](/d1/platform/limits)
- [偏差](/d1/platform/deviations)
