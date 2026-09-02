# Compatibility flags

Projects use Wrangler's standard `compatibility_flags` array. The server accepts only flags advertised by extension capabilities and supported by the pinned runtime.

```json
{
  "name": "hello-typescript",
  "main": "src/index.ts",
  "compatibility_date": "2026-08-30",
  "compatibility_flags": []
}
```

Use Wrangler's snake_case field names. Internal system flags remain executable identity and are not copied into project configuration.

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Flag names and semantics | [Cloudflare compatibility flags](https://developers.cloudflare.com/workers/configuration/compatibility-flags/) | Same names from workerd |
| Project `compatibility_flags` | Yes | Persisted and validated per immutable Version |
| Unsupported flags | Upload fails | Upload fails closed |
| Live supported set | Dashboard / Wrangler | Extension capabilities and `packages/runtime/workerd.lock.json` |
