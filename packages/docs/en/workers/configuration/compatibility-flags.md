# Compatibility flags

Flags are controlled by the platform runtime lock. Project JSON cannot set them.

```json
{
  "name": "hello-typescript",
  "main": "src/index.ts"
}
```

Do not add `compatibilityFlags`. Current lock: `requiredCompatibilityFlags` is empty; `systemCompatibilityFlags` is `experimental` and `service_binding_extra_handlers`. That is executable identity, not a menu of switches.

## Same as Cloudflare

Flag names and semantics come from workerd / [Cloudflare compatibility flags](https://developers.cloudflare.com/workers/configuration/compatibility-flags/). The pinned baseline already includes Node compatibility, so `node:` imports do not need you to flip a flag.

## Intentional delta

`open-compute.json` must not contain `compatibilityFlags` / `compatibility_flags`. Unknown fields fail. The toolchain also does not forward a Wrangler flag list to the platform. For the live set: `ocd capabilities --json` `runtime`, and `packages/runtime/workerd.lock.json` in the repo.
