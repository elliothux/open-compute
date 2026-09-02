# Compatibility flags

Compatibility flags 由平台 runtime lock 控制。项目 JSON 不能设置它们。

```json
{
  "name": "hello-typescript",
  "main": "src/index.ts"
}
```

不可写入 `compatibilityFlags`。当前 lock：`requiredCompatibilityFlags` 为空；`systemCompatibilityFlags` 为 `experimental` 和 `service_binding_extra_handlers`。这是可执行文件身份的一部分，不是项目级开关。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| flag 名字与语义 | 是，见 [Cloudflare compatibility flags](https://developers.cloudflare.com/workers/configuration/compatibility-flags/) | 来自 workerd / 同一套名字 |
| pinned baseline 已提供 Node 兼容，`node:` 导入无需再开 flag | 取决于项目 flag | 是 |
| 项目 JSON 中的 `compatibilityFlags` / `compatibility_flags` | 是 | 不允许；未知字段失败 |
| 将 Wrangler flag 列表转发给平台 | Wrangler | 不提供 |
| 查看实际集合 | 控制台 / Wrangler | `ocd capabilities --json` 的 `runtime`，以及仓库 `packages/runtime/workerd.lock.json` |
