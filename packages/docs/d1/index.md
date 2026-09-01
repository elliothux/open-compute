# D1

D1 是 SQLite SQL 数据库，用于从 Worker 查询关系数据。本平台上，每个 database 是运行 ocd 的该节点上的一份本地主 SQLite。

例如：

- 从 Worker 查询关系数据
- 导入 schema 并执行 SQL
- 在一次事务中 batch 多条语句

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

在 `open-compute.json` 中绑定已有的 database。普通产品 binding 为 `{ type, id, permissions? }`：

```json
{
  "name": "d1-app",
  "main": "src/index.ts",
  "bindings": {
    "DB": { "type": "d1_database", "id": "<d1-database-id>" }
  }
}
```

`id` 是本平台上已存在的 database。绑定语法见 [bindings](/workers/configuration/bindings)。CLI 为 `oc` / `oc run` / `oc types`。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [D1 Worker API](https://developers.cloudflare.com/d1/worker-api/) | 相同：`prepare` / `bind` / `run` / `all` / `first` / `raw` / `exec` / `batch`、session、opaque bookmark、prepared-statement / result / meta |
| 拓扑 | 托管 D1，含 read replica | 运行 ocd 的该节点上的本地主 SQLite |
| Read replica | 提供 | 不提供 |
| Region routing | 提供 | 不提供 |
| `served_by` 地理 | region / colo metadata | 不提供；`served_by_*` 不是地理产品 |
| Bookmark | 跨副本因果 | 同一数据库的本地顺序 |
| `rows_read` / `rows_written` | 计费计数 | 本地 SQLite 执行计数 |
| `dump()` | hosted 非 alpha 拒绝 | 同样拒绝（`D1_DUMP_ERROR`） |
| REST / `client.v4` | 提供 | 不提供；使用 Worker binding |

## 本节

- [上手](/d1/get-started/)
- [概念](/d1/concepts/)
- [指南](/d1/guides/)
- [示例](/d1/examples/)
- [限制](/d1/platform/limits)
- [行为差异](/d1/platform/deviations)
