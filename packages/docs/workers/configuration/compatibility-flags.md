# Compatibility flags

flags 由平台 runtime lock 控制。项目 JSON 不能设置它们。

```json
{
  "name": "hello-typescript",
  "main": "src/index.ts"
}
```

不要加 `compatibilityFlags`。当前 lock：`requiredCompatibilityFlags` 为空；`systemCompatibilityFlags` 为 `experimental` 和 `service_binding_extra_handlers`。这是可执行文件身份的一部分，不是给你开关的菜单。

## 与 Cloudflare 相同

flag 名字和语义来自 workerd / [Cloudflare compatibility flags](https://developers.cloudflare.com/workers/configuration/compatibility-flags/)。pinned baseline 已提供 Node 兼容，因此 `node:` 导入无需你再开 flag。

## 故意不同

`open-compute.json` 不得包含 `compatibilityFlags` / `compatibility_flags`。未知字段失败。工具链也不会把 Wrangler 的 flag 列表转给平台。要看实际集合：`ocd capabilities --json` 的 `runtime`，以及仓库 `packages/runtime/workerd.lock.json`。
