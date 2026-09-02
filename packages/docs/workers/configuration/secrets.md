# Secrets

Secrets may only reference environment variables: `"secrets": { "TOKEN": { "env": "MY_TOKEN" } }`. Only `run` / `deploy` read the values. Offline bundles do not contain secrets.

```json
{
  "name": "secure",
  "main": "src/index.ts",
  "secrets": {
    "TOKEN": { "env": "MY_TOKEN" }
  }
}
```

```ts
export default {
  fetch(_request: Request, env: Env): Response {
    return new Response(env.TOKEN ? "present" : "missing");
  },
} satisfies ExportedHandler<Env>;
```

The admin token is read from `OPEN_COMPUTE_ADMIN_TOKEN`, or from another variable named by `--token-env`. Do not put secret values in the project file or in command arguments. A secret object may only have the `env` key.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| `env.TOKEN` is a `string`; type generation uses `string`, not a literal | Yes — [Secrets](https://developers.cloudflare.com/workers/configuration/secrets/) | Yes |
| `wrangler secret put` | Yes | Not provided |
| Cloudflare Secrets Store / dashboard ciphertext | Yes | Not provided |
| `file:` reference in **project** JSON | N/A | Not allowed (that form is for ocd operator config) |
| Missing environment variable | Command-dependent | `run` / `deploy` fail |

