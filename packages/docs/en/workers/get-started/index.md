# Get started

Use the CLI to type-check, bundle, validate, and activate a module Worker on an already-running local `ocd`. `oc run` does not start another workerd. If the platform is not up, go to [ocd get started](/en/ocd/get-started) first.

```sh
bun run oc run --config examples/hello-worker/open-compute.json \
  --ocd "$PWD/target/debug/ocd"
```

The default platform origin is `http://127.0.0.1:8787`. The command type-checks, bundles with Rolldown, creates or reuses a Worker of the same name, validates and promotes, then prints a reachable URL. Re-run the same command after source changes; there is no watch / HMR. Remote deploys use `oc deploy` and accept HTTPS origins only.

## Same as Cloudflare

Still an ES module Worker: `export default { fetch }`. Handler signatures, `Response.json`, and `env` injection match the programming model in the [Cloudflare Workers CLI guide](https://developers.cloudflare.com/workers/get-started/guide/). Types come from pinned `@open-compute/workers-types` (the pinned `@cloudflare/workers-types`).

## Intentional delta

The CLI is `oc` (`bun run oc ...`), not Wrangler. The project file is `open-compute.json`, not a full `wrangler.jsonc`. There is no C3 scaffolder, no Cloudflare login, no `workers.dev` preview, and no dashboard. `--ocd` (or `OPEN_COMPUTE_OCD`) must point at a matching `ocd` to encode the bundle; `run` requires the platform to already be listening.

## Hello Worker files

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

`build --out` must be a file that does not already exist. `types` does not need `ocd`. The admin token is read from `OPEN_COMPUTE_ADMIN_TOKEN` (or `--token-env`). Next: [Concepts](/en/workers/concepts/), [Configuration](/en/workers/configuration/).
