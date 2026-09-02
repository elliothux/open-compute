# Get started

This guide runs a hello Worker on a ready `ocd`.

## 1. ocd installed, and `/health/ready`

Follow [Install and first start](/ocd/get-started) until `ocd` is running. This page assumes `GET /health/ready` already returns 200. `oc run` does not start another workerd; it activates the Worker on the already-running local platform.

## 2. Hello Worker

Use `examples/hello-worker` from the repo. The project file is `open-compute.json`:

```json
{
  "name": "hello-typescript",
  "main": "src/index.ts",
  "vars": {
    "GREETING": "Hello from TypeScript"
  }
}
```

The entry is a module Worker:

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

## 3. It prints a URL

From the repository root:

```sh
bun run oc run --config examples/hello-worker/open-compute.json --ocd <path-to-ocd>
```

On success it prints the Worker URL. Replace `<path-to-ocd>` with the path to the `ocd` binary (or set `OPEN_COMPUTE_OCD`).

## `open-compute.json` is not `wrangler.jsonc`

The config borrows some Wrangler field names (`name`, `main`, `vars`), but **`open-compute.json` is not `wrangler.jsonc`**. Unknown fields are rejected. There is no `compatibilityDate` in the project JSON: the platform freezes the compatibility date. See `runtime.effective_compatibility_date` from `ocd capabilities --json`.

Next: [Directory](/directory). To run `ocd` as a service, see [Operate](/ocd/).
