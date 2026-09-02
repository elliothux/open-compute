# Get started

Use the CLI to type-check, bundle, validate, and activate a module Worker on a running local `ocd`. `oc run` does not start another workerd. If the platform is not running, complete [ocd get started](/ocd/get-started) first.

```sh
bun run oc run --config examples/hello-worker/open-compute.json \
  --ocd "$PWD/target/debug/ocd"
```

The default platform origin is `http://127.0.0.1:8787`. The command type-checks, bundles with Rolldown, creates or reuses a Worker of the same name, validates and promotes, then prints a reachable URL. Re-run the same command after source changes. Watch / HMR is not provided. Remote deploys use `oc deploy` and accept HTTPS origins only.

## Hello Worker

From `examples/hello-worker/`. `open-compute.json`:

```json
{
  "name": "hello-typescript",
  "main": "src/index.ts",
  "vars": {
    "GREETING": "Hello from TypeScript"
  }
}
```

`src/index.ts`:

```ts
export default {
  fetch(request: Request, env: Env): Response {
    return Response.json({
      message: env.GREETING,
      pathname: new URL(request.url).pathname,
    });
  },
} satisfies ExportedHandler<Env>;
```

The same directory also has `package.json` (`@open-compute/workers-types`), `tsconfig.json`, and `worker-configuration.d.ts` from `bun run oc types`. Offline artifacts:

```sh
bun run oc build --config examples/hello-worker/open-compute.json \
  --ocd "$PWD/target/debug/ocd" --out /absolute/new-worker.bundle
bun run oc types --config examples/hello-worker/open-compute.json
```

`build --out` must be a file that does not already exist. `types` does not need `ocd`. The admin token is read from `OPEN_COMPUTE_ADMIN_TOKEN` (or `--token-env`).

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Module Worker (`export default { fetch }`) | Yes | Yes |
| Handler signatures, `Response.json`, `env` injection | Yes — [Workers CLI guide](https://developers.cloudflare.com/workers/get-started/guide/) | Yes |
| Types package | `@cloudflare/workers-types` | Pinned `@open-compute/workers-types` |
| CLI | Wrangler | `oc` (`bun run oc ...`) |
| Project file | wrangler.jsonc | `open-compute.json` (unknown fields rejected) |
| C3 scaffolder / Cloudflare login / workers.dev preview / dashboard | Yes | Not provided |
| `--ocd` / `OPEN_COMPUTE_OCD` | N/A | Must point at a matching `ocd` to encode the bundle; `run` requires the platform to be listening |

Next: [Concepts](/workers/concepts/), [Configuration](/workers/configuration/).
