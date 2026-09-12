---
title: "Configuration"
---

`wrangler@4.127.1/config-schema.json` is the project grammar authority. `ocd wrangler` does not parse the project. Use project-local Wrangler for config resolution and type generation.

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

Framework adapters keep the user `wrangler.jsonc` and emit the standard `.wrangler/deploy/config.json` redirect to a generated Wrangler config. Project-local Wrangler owns type generation and deployment.

See [Bindings](/docs/workers/configuration/bindings/), [compatibility dates](/docs/workers/configuration/compatibility-dates/), [compatibility flags](/docs/workers/configuration/compatibility-flags/), [Cron](/docs/workers/configuration/cron-triggers/), [variables](/docs/workers/configuration/environment-variables/), and [secrets](/docs/workers/configuration/secrets/).
