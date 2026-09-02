# Bindings (`env`)

`env` contains only names declared on the deployment. Version Metadata is a platform-injected read-only object: `id`, `tag`, `timestamp`.

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const version = env.VERSION.id;
    return env.AUTH.fetch(request);
  },
} satisfies ExportedHandler<Env>;
```

```json
{
  "name": "front",
  "main": "src/index.ts",
  "services": [{ "binding": "AUTH", "service": "auth-worker" }],
  "version_metadata": { "binding": "VERSION", "tag": "release-1" }
}
```

Service Bindings: default/named `fetch` and RPC. The target must be a uniquely resolvable Worker name in the same account; deploy time freezes a target ID. `entrypoint` is optional.

Member signatures for KV / R2 / D1 / DO / Queue / Workflow / Assets / Images belong on those product pages. Config grammar: [configuration · bindings](/en/workers/configuration/bindings).

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| `env.BINDING` types | Yes — [Bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/) and [Service bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/) | Yes |
| Version Metadata fields | Yes — [version metadata](https://developers.cloudflare.com/workers/runtime-apis/bindings/version-metadata/) | `id`, `tag`, `timestamp` |
| Service Bindings | Cross-region placement / global service discovery | Same-platform only; default/named fetch and RPC; target admission, deployment pins, capability lifetime, and recovery are local and fail closed |
| Workers for Platforms dispatcher / Dynamic Worker Loaders | Yes | Not provided |
| mTLS / Rate Limit / Secrets Store / AI binding | Yes | Not provided |

