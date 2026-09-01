# Configuration

The Worker project file is `open-compute.json`. It must have `name` and one content shape: `main`, `assets`, `main` + `assets`, or `frameworkOutput`. The parser accepts only the implemented fields below. Unknown fields are rejected.

```json
{
  "name": "hello-typescript",
  "main": "src/index.ts",
  "vars": { "GREETING": "Hello from TypeScript" }
}
```

## Fields

| Field | Role |
| --- | --- |
| `name` | Worker name, `[a-z0-9]` with internal hyphens |
| `main` | TS entry, relative to the config file directory |
| `frameworkOutput` | Already-built framework output; cannot combine with explicit `main` / `assets` |
| `tsconfig` | Defaults to `tsconfig.json` |
| `vars` | Public variables; JSON values enter `env` |
| `secrets` | `{ "TOKEN": { "env": "MY_TOKEN" } }`; environment references only |
| `bindings` | Object: key is the `env` name, value is `{type, id, permissions?}`. DO / Workflow also need `className`. Workflow may set `schedules` |
| `services` | Array `[{binding, service, entrypoint?}]` |
| `assets` | `directory`, `binding?`, `run_worker_first`, `html_handling`, `not_found_handling`, `publish_source_maps` |
| `cache` | `enabled`, `cross_version_cache` |
| `exports` | Cache overrides for named Worker entrypoints; only `{"type":"worker","cache":{...}}` |
| `images` | `{ "binding": "IMAGES" }` |
| `version_metadata` | `{ "binding": "VERSION", "tag"? }` |
| `accountId` | Override the default account |
| `endpoint` | Platform origin; default `http://127.0.0.1:8787` |

`main`, `frameworkOutput`, the assets directory, and `tsconfig` resolve relative to the config file directory and cannot escape the project boundary. Assets-only projects cannot declare vars, secrets, product/service bindings, or Worker-first. All binding names share one `env` namespace. The file is at most 64 KiB and must be strict JSON (not jsonc).

`bindings.type`: `kv_namespace`, `r2_bucket`, `d1_database`, `do_namespace`, `queue_producer`, `workflow`.

## Same as Cloudflare

Field names and semantics borrow common Wrangler configuration: `vars`, `secrets`, assets routing, cache enabled, service bindings. A module Worker's `main` points at a TS/JS entry. Compare [Wrangler configuration](https://developers.cloudflare.com/workers/wrangler/configuration/).

## Intentional delta

This is not a full `wrangler.jsonc` compatibility layer. There is no `compatibility_date` / `compatibility_flags` / `compatibilityDate` / `compatibilityFlags`. There is no `workers_dev`, Custom Domains, CF zone `routes`, placement, observability, AI, or vectorize. Unknown keys fail rather than being ignored. An API the platform does not advertise cannot be turned on from toolchain config.

Subpages: [bindings](/en/workers/configuration/bindings), [compatibility dates](/en/workers/configuration/compatibility-dates), [compatibility flags](/en/workers/configuration/compatibility-flags), [Cron](/en/workers/configuration/cron-triggers), [vars](/en/workers/configuration/environment-variables), [secrets](/en/workers/configuration/secrets), [routing](/en/workers/configuration/routing).
