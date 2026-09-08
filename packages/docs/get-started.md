# Get started

Use the repository example with the exact project-local Wrangler. Start `ocd` first and wait for `GET /health/ready` to return 200.

```sh
bun install --frozen-lockfile
./target/debug/ocd wrangler --project examples/hello-worker deploy --env dev
```

`ocd wrangler` selects the local instance or explicit remote target, verifies the capability-advertised exact pin, and then replaces itself with Wrangler. Authentication, multipart upload, Versions, Deployments, Secrets, Static Assets, and resource provisioning use the Cloudflare v4 contract.

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

Next: [Wrangler projects and deployment targets](/workers/projects), [Workers configuration](/workers/configuration/), and [ocd operations](/ocd/).
