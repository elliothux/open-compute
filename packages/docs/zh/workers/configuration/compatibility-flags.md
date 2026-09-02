# Compatibility flags

项目使用 Wrangler 标准 `compatibility_flags` 数组。server 只接受 extension capabilities 广告且 pinned runtime 支持的 flag。

```json
{
  "name": "hello-typescript",
  "main": "src/index.ts",
  "compatibility_date": "2026-08-30",
  "compatibility_flags": []
}
```

使用 Wrangler 的 snake_case 字段。内部 system flags 仍属于 executable identity，不复制到项目配置。

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| flag 名称与语义 | [Cloudflare compatibility flags](https://developers.cloudflare.com/workers/configuration/compatibility-flags/) | 来自 workerd 的相同名称 |
| 项目 `compatibility_flags` | 是 | 按不可变 Version 持久化和验证 |
| 不支持的 flag | upload 失败 | fail closed |
| 实际支持集合 | Dashboard / Wrangler | extension capabilities 与 `packages/runtime/workerd.lock.json` |
