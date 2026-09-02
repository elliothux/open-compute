# Get started

Deploy a module Worker through the exact pinned Wrangler client:

```sh
CLOUDFLARE_API_BASE_URL=http://127.0.0.1:8787/client/v4 \
CLOUDFLARE_API_TOKEN=<token> \
CLOUDFLARE_ACCOUNT_ID=<account-id> \
bun run oc deploy --config examples/hello-worker/wrangler.jsonc
```

The example is a standard Wrangler project with `name`, `main`, `compatibility_date`, `workers_dev: false`, and `vars`. Re-run the same command after changing source. Online upload and activation are owned by `wrangler@4.127.1`.

Local-only commands remain available:

```sh
bun run oc build --config examples/hello-worker/wrangler.jsonc \
  --ocd "$PWD/target/debug/ocd" --out /absolute/new-worker.bundle
bun run oc types --config examples/hello-worker/wrangler.jsonc
```

`build` type-checks with TypeScript 7, bundles with Rolldown, validates configured assets, and writes one Worker bundle without overwriting an existing file. Assets-only projects deploy directly with Wrangler. `types` writes `worker-configuration.d.ts` by default.

Next: [Configuration](/workers/configuration/) and [Versions and deployments](/workers/versions-and-deployments/).
