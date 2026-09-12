---
title: "D1"
---

D1 是 Worker 可查询的 SQLite 数据库。在 open-compute 上，每个 database 是运行 `ocd` 的主机上的一份 SQLite，不提供只读副本。

例如：

- 从 Worker 查询关系数据
- 导入 schema 并执行 SQL
- 在同一事务中执行多条语句

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const { results } = await env.DB.prepare(
      "SELECT * FROM Customers WHERE CompanyName = ?",
    )
      .bind("Bs Beverages")
      .all();
    return Response.json(results);
  },
} satisfies ExportedHandler<{ DB: D1Database }>;
```

在 `wrangler.jsonc` 中绑定已存在的 database：

```json
{
  "name": "d1-app",
  "main": "src/index.ts",
  "d1_databases": [
    {
      "binding": "DB",
      "database_name": "app",
      "database_id": "<d1-database-id>"
    }
  ]
}
```

`database_id` 必须指向平台上已有的 database。语法见[绑定](/docs/zh/workers/configuration/bindings/)。类型生成使用项目内 Wrangler，真实 target 部署使用 `ocd wrangler deploy`。

## 兼容性

| 主题                         | Cloudflare                                                        | open-compute                                                                                                                         |
| ---------------------------- | ----------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| Worker API                   | [D1 Worker API](https://developers.cloudflare.com/d1/worker-api/) | 相同：`prepare` / `bind` / `run` / `all` / `first` / `raw` / `exec` / `batch`、session、bookmark、prepared-statement / result / meta |
| 存储位置                     | 托管 D1（可含只读副本）                                           | 本机一份 SQLite                                                                                                                      |
| 只读副本                     | 提供                                                              | 不提供                                                                                                                               |
| 按区域路由                   | 提供                                                              | 不提供                                                                                                                               |
| `served_by` 地域信息         | region / colo                                                     | 不提供；`served_by_*` 不表示地域产品                                                                                                 |
| Bookmark                     | 跨副本因果顺序                                                    | 同一数据库上的本地顺序                                                                                                               |
| `rows_read` / `rows_written` | 计费计数                                                          | 本地 SQLite 执行计数                                                                                                                 |
| `dump()`                     | 托管非 alpha 拒绝                                                 | 同样拒绝（`D1_DUMP_ERROR`）                                                                                                          |
| REST / `client/v4`           | 提供                                                              | 兼容 account-scoped database 与 query 操作                                                                                           |

下一步：[使用 bindings 开发](/docs/zh/develop/) · [兼容性与限制](/docs/zh/reference/)
