# Get started

Use the repository example with the exact pinned Wrangler transport. Start `ocd` first and wait for `GET /health/ready` to return 200.

```sh
export CLOUDFLARE_API_BASE_URL=http://127.0.0.1:8787/client/v4
export CLOUDFLARE_API_TOKEN=<token>
export CLOUDFLARE_ACCOUNT_ID=<account-id>
bun run oc deploy --config examples/hello-worker/wrangler.jsonc
```

`oc deploy` is a thin wrapper around `wrangler@4.127.1 deploy`. Authentication, multipart upload, Versions, Deployments, Secrets, Static Assets, and resource provisioning use the Cloudflare v4 contract.

The project uses standard Wrangler configuration:

```json
{
  "$schema": "../../node_modules/wrangler/config-schema.json",
  "name": "hello-typescript",
  "main": "src/index.ts",
  "compatibility_date": "2026-08-30",
  "workers_dev": false,
  "vars": {
    "GREETING": "Hello from TypeScript"
  }
}
```

For offline validation, `oc build` keeps the repository's TypeScript 7 and Rolldown checks and emits one Worker bundle. `oc types` generates local `Env` types. Neither command contacts the management API. Static Assets are validated locally and uploaded only by Wrangler.

Next: [Workers configuration](/workers/configuration/) and [ocd operations](/ocd/).
