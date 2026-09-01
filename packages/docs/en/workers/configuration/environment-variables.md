# Environment variables

`vars` are public, non-secret values. Structured-clone-compatible JSON enters `env`.

```json
{
  "name": "hello-typescript",
  "main": "src/index.ts",
  "vars": {
    "GREETING": "Hello from TypeScript"
  }
}
```

```ts
export default {
  fetch(_request: Request, env: Env): Response {
    return new Response(env.GREETING);
  },
} satisfies ExportedHandler<Env>;
```

`bun run oc types` writes literals into `worker-configuration.d.ts` (for example `GREETING: "Hello from TypeScript"`). Regenerate types after changing vars. Offline bundles do not contain vars; `run` / `deploy` inject them.

## Same as Cloudflare

Public strings/JSON on `env`, not secrets. See [Environment variables](https://developers.cloudflare.com/workers/configuration/environment-variables/).

## Intentional delta

No Wrangler `[vars]` TOML, no dashboard editor, no per-environment Wrangler environments product. Unknown top-level keys still fail the whole project config. Secrets go in [secrets](/en/workers/configuration/secrets), not `vars`.
