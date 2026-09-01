# KV

Workers KV 是键值存储，用于从 Worker 读写数据。本平台上，每个 namespace 是运行 ocd 的该节点上的一份 SQLite 数据库。

例如：

- 缓存 API 响应
- 存储用户配置与偏好
- 存储认证会话

```ts
export default {
  async fetch(request, env, ctx): Promise<Response> {
    // 写入键值
    await env.KV.put("KEY", "VALUE");

    // 读取键值
    const value = await env.KV.get("KEY");

    // 列出键
    const allKeys = await env.KV.list();

    // 删除键值
    await env.KV.delete("KEY");

    return Response.json({ value, allKeys });
  },
} satisfies ExportedHandler<{ KV: KVNamespace }>;
```

在 `open-compute.json` 中绑定已有的 namespace。普通产品 binding 为 `{ type, id, permissions? }`：

```json
{
  "name": "kv-app",
  "main": "src/index.ts",
  "bindings": {
    "KV": { "type": "kv_namespace", "id": "<kv-namespace-id>" }
  }
}
```

`id` 是本平台上已存在的 namespace。可选 `permissions`：`{ "read": true, "write": true }`。绑定语法见 [Workers 配置 · bindings](/workers/configuration/bindings)。CLI 为 `oc` / `oc run` / `oc types`。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [KV API](https://developers.cloudflare.com/kv/api/) | 相同：`put` / `get` / `getWithMetadata` / `list` / `delete`，text / json / arrayBuffer / stream、metadata、TTL、bulk get、list cursor |
| 复制 | 全球边缘 | 运行 ocd 的该节点上的单节点 SQLite |
| `cacheTtl` | Colo cache | 接受该参数；无 colo cache |
| Jurisdictions | 提供 | 不提供 |
| REST / `client.v4` | 提供 | 不提供；使用 Worker binding |

## 本节

- [上手](/kv/get-started/)
- [概念](/kv/concepts/)
- [指南](/kv/guides/)
- [示例](/kv/examples/)
- [限制](/kv/platform/limits)
- [行为差异](/kv/platform/deviations)
