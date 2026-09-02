# KV

Workers KV 是 Worker 可访问的键值存储。在 open-compute 上，每个 namespace 对应运行 `ocd` 的主机上的一份 SQLite 数据。

例如：

- 缓存 API 响应
- 存储用户配置
- 存储会话

```ts
export default {
  async fetch(request, env, ctx): Promise<Response> {
    await env.KV.put("KEY", "VALUE");
    const value = await env.KV.get("KEY");
    const allKeys = await env.KV.list();
    await env.KV.delete("KEY");
    return Response.json({ value, allKeys });
  },
} satisfies ExportedHandler<{ KV: KVNamespace }>;
```

在 `wrangler.jsonc` 中绑定已存在的 namespace：

```json
{
  "name": "kv-app",
  "main": "src/index.ts",
  "kv_namespaces": [{ "binding": "KV", "id": "<kv-namespace-id>" }]
}
```

`id` 必须指向 account 中已有的 namespace。语法见[绑定](/zh/workers/configuration/bindings)。namespace 与 value 操作使用固定 Wrangler 或官方 SDK。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [KV API](https://developers.cloudflare.com/kv/api/) | 相同：`put` / `get` / `getWithMetadata` / `list` / `delete`，以及 text / json / arrayBuffer / stream、metadata、TTL、批量 get、list cursor |
| 存储位置 | Cloudflare 边缘网络 | 本机 SQLite |
| `cacheTtl` | 边缘缓存 | 接受该参数，但不建立边缘缓存 |
| 数据驻留（Jurisdictions） | 提供 | 不提供 |
| REST / `client/v4` | 提供 | 兼容 account-scoped namespace 与 value 操作 |

## 本节

- [上手](/zh/kv/get-started/)
- [概念](/zh/kv/concepts/)
- [指南](/zh/kv/guides/)
- [示例](/zh/kv/examples/)
- [限制](/zh/kv/platform/limits)
- [行为差异](/zh/kv/platform/deviations)
