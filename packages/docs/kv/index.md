# KV

Workers KV 是绑到 Worker `env` 的键值存储。本平台上，一个 namespace 的权威是这一台机器上的一份 SQLite 数据库。没有全球边缘缓存，也没有跨节点复制。

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

## 与 Cloudflare 相同

Worker binding API 与 [Cloudflare KV API](https://developers.cloudflare.com/kv/api/) 相同：`put` / `get` / `getWithMetadata` / `list` / `delete`，以及 text / json / arrayBuffer / stream、metadata、TTL、bulk get、list cursor。52 个目标成员为 `supported_with_deviation`。

```json
{
  "name": "kv-app",
  "main": "src/index.ts",
  "bindings": {
    "KV": { "type": "kv_namespace", "id": "<kv-namespace-id>" }
  }
}
```

`id` 是平台上已存在的 namespace。绑定语法见 [Workers 配置 · bindings](/workers/configuration/bindings)。不要从本页抄 Cloudflare REST 或 Wrangler KV 子命令。

## 故意不同

**`OC-KV-001`**：KV 是单节点 SQLite 权威存储，不声称 Cloudflare 全球复制或传播时延。`cacheTtl` 只做参数兼容，不建立 colo cache。没有 jurisdiction 产品，没有 `api.cloudflare.com/client/v4`。

全文见 [偏差](/kv/platform/deviations) 和 [Compatibility](/platform/compatibility)。

## 本节

- [上手](/kv/get-started/)
- [概念](/kv/concepts/)
- [指南](/kv/guides/)
- [示例](/kv/examples/)
- [限制](/kv/platform/limits)
- [偏差](/kv/platform/deviations)
