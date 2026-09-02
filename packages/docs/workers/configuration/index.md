# Configuration

`wrangler@4.127.1/config-schema.json` is the only project grammar authority. The local adapter calls Wrangler's config and environment resolvers; it does not maintain a second parser.

```json
{
  "$schema": "./node_modules/wrangler/config-schema.json",
  "name": "app",
  "main": "src/index.ts",
  "compatibility_date": "2026-08-30",
  "workers_dev": false,
  "vars": { "LOG_LEVEL": "info" }
}
```

Supported P6 fields include standard `name`, `account_id`, `main`, `compatibility_date`, `compatibility_flags`, `env`, build fields, `vars`, product binding arrays, Service Bindings, Static Assets, cron triggers, Images, Workers AI, Version Metadata, cache configuration, and the local-only `secrets.required` declaration. A field passing Wrangler schema validation is not sufficient by itself: unsupported server capabilities fail closed during API or upload validation.

Framework adapters keep the user `wrangler.jsonc` and emit the standard `.wrangler/deploy/config.json` redirect to a generated Wrangler config. `oc deploy` and `oc run` invoke pinned Wrangler; `oc build` and `oc types` keep local build and type-generation responsibilities.

See [Bindings](/workers/configuration/bindings), [compatibility dates](/workers/configuration/compatibility-dates), [compatibility flags](/workers/configuration/compatibility-flags), [Cron](/workers/configuration/cron-triggers), [variables](/workers/configuration/environment-variables), and [secrets](/workers/configuration/secrets).
