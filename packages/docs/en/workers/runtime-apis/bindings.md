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

## Same as Cloudflare

`env.BINDING` types match [Bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/) and [Service bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/). Version Metadata fields: [version metadata](https://developers.cloudflare.com/workers/runtime-apis/bindings/version-metadata/). Member signatures for KV / R2 / D1 / DO / Queue / Workflow / Assets / Images belong on those product pages, not here.

## Intentional delta: OC-SERVICE-001

Service Bindings provide default/named fetch and RPC within one platform authority. They do not claim Cloudflare cross-region placement or global service discovery; target admission, deployment pins, capability lifetime, and recovery are local and fail closed.

No Workers for Platforms dispatcher, no Dynamic Worker Loaders as a tenant product, no mTLS / Rate Limit / Secrets Store / AI binding.
