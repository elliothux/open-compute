---
title: "Configuration"
---

`wrangler@4.127.1/config-schema.json` is the only project grammar authority. `ocd wrangler` does not parse the project. The offline `oc build` / `oc types` tools call Wrangler's config and environment resolvers instead of maintaining a second grammar.

```json
{
  "$schema": "./node_modules/wrangler/config-schema.json",
  "name": "app",
  "main": "src/index.ts",
  "compatibility_date": "2026-09-08",
  "workers_dev": false,
  "vars": { "LOG_LEVEL": "info" }
}
```

Supported fields include standard `name`, `account_id`, `main`, `compatibility_date`, `compatibility_flags`, `env`, build fields, `vars`, product binding arrays, Service Bindings, Static Assets, cron triggers, Images, Workers AI, Version Metadata, cache configuration, and the local-only `secrets.required` declaration. A field passing Wrangler schema validation is not sufficient by itself: unsupported server capabilities fail closed during API or upload validation.

Framework adapters keep the user `wrangler.jsonc` and emit the standard `.wrangler/deploy/config.json` redirect to a generated Wrangler config. Project-local Wrangler owns deployment; `oc build` and `oc types` keep only offline build and type-generation responsibilities.

See [Bindings](/docs/workers/configuration/bindings), [compatibility dates](/docs/workers/configuration/compatibility-dates), [compatibility flags](/docs/workers/configuration/compatibility-flags), [Cron](/docs/workers/configuration/cron-triggers), [variables](/docs/workers/configuration/environment-variables), and [secrets](/docs/workers/configuration/secrets).
