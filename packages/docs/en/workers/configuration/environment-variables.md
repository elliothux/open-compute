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

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Public strings/JSON on `env`, not secrets | Yes — [Environment variables](https://developers.cloudflare.com/workers/configuration/environment-variables/) | Yes |
| Wrangler `[vars]` TOML | Yes | Not provided |
| Dashboard editor / Wrangler environments product | Yes | Not provided |
| Unknown top-level keys | May be ignored | Fail the whole project config |
| Where secrets live | Secrets product | [secrets](/en/workers/configuration/secrets), not `vars` |

