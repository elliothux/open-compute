# Compatibility flags

Flags are controlled by the platform runtime lock. Project JSON cannot set them.

```json
{
  "name": "hello-typescript",
  "main": "src/index.ts"
}
```

Do not add `compatibilityFlags`. Current lock: `requiredCompatibilityFlags` is empty; `systemCompatibilityFlags` is `experimental` and `service_binding_extra_handlers`. That is executable identity, not a project-level switch.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Flag names and semantics | Yes — [Cloudflare compatibility flags](https://developers.cloudflare.com/workers/configuration/compatibility-flags/) | From workerd / the same names |
| Pinned baseline already includes Node compatibility; `node:` imports need no extra flag | Depends on project flags | Yes |
| `compatibilityFlags` / `compatibility_flags` in project JSON | Yes | Not allowed; unknown fields fail |
| Forwarding a Wrangler flag list to the platform | Wrangler | Not provided |
| Live set | Dashboard / Wrangler | `ocd capabilities --json` `runtime`, and `packages/runtime/workerd.lock.json` in the repo |

