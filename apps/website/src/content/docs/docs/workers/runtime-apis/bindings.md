---
title: "Bindings (`env`)"
---

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

Service Bindings: default/named `fetch` and RPC. The target is a uniquely resolvable Worker name, an operator-configured [extension](/docs/extension/) slug, or a fixed [private HTTP target](/docs/ocd/configuration/) in the same instance; deploy time freezes the target identity and policy revision. `entrypoint` is optional. Private HTTP targets expose `fetch` only. There is no new public Binding type.

Member signatures for KV / R2 / D1 / DO / Queue / Workflow / Assets / Images belong on those product pages. Config grammar: [configuration · bindings](/docs/workers/configuration/bindings/).

## Compatibility

| Topic                                          | Cloudflare                                                                                                                                                                                   | open-compute                                                                                                                                       |
| ---------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| `env.BINDING` types                            | Yes — [Bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/) and [Service bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/) | Yes                                                                                                                                                |
| Version Metadata fields                        | Yes — [version metadata](https://developers.cloudflare.com/workers/runtime-apis/bindings/version-metadata/)                                                                                  | `id`, `tag`, `timestamp`                                                                                                                           |
| Service Bindings                               | Cross-region placement / global service discovery                                                                                                                                            | Same-platform only; default/named fetch and RPC; target admission, deployment pins, capability lifetime, and recovery are local and fail closed    |
| Dynamic Workers / Worker Loader                | [Loader API](https://developers.cloudflare.com/dynamic-workers/api-reference/)                                                                                                               | Native `load/get`, modules, entrypoint/RPC, user tails, dynamic DO facets, and explicit/delegated limits; experimental controls remain unavailable |
| Workers for Platforms dispatcher               | Yes                                                                                                                                                                                          | Not provided                                                                                                                                       |
| mTLS / Rate Limit / Secrets Store / AI binding | Yes                                                                                                                                                                                          | Not provided                                                                                                                                       |

## Dynamic Workers

Declare `worker_loaders: [{ binding: "LOADER" }]` to expose the native WorkerLoader API as `env.LOADER`.
The same `get(id, callback)` ID must describe immutable code; do not depend on cache hits or callback counts.
Namespaces are isolated by account, Script and binding. Version rollback retains the namespace; deleting and
recreating a Script gives it a new namespace. A Worker invocation permits 4 distinct children in flight,
and a DO context permits 10; concurrent calls to the same child count once.

Explicit `limits` use the official `cpuMs` and `subRequests` fields; `{}` selects Standard defaults, and child,
entrypoint, and delegated Loader ceilings compose by the strictest dimension. CPU, memory, startup, subrequest,
and simultaneous outbound-connection limits are enforced by the pinned workerd fork. Nonempty streaming tails
and two experimental-control members remain unavailable. Dynamic Python child cold boot is not qualified because
local Pyodide bootstrap cannot reliably meet the official one-second startup CPU limit; JavaScript, Wasm, RPC,
dynamic Durable Object facets, and limits are qualified. See [behavior differences](/docs/platform/deviations/).

Structured-clone values and Service Bindings can be passed directly in `load({ env })`. Forward KV, D1, R2, and Queue bindings through the open-compute helper, which preserves the binding boundary instead of cloning the resource object:

```ts
import { loadWorker } from "open-compute:worker-loader";

const child = loadWorker(env.LOADER, {
  ...code,
  env: { CACHE: env.CACHE, DB: env.DB, BUCKET: env.BUCKET, QUEUE: env.QUEUE },
});
```

For the certified `2026-09-08` date, the pinned Pyodide
bundle is embedded in `ocd`, verified, and loaded from the instance's private runtime cache. Other official
child date/flag combinations retain workerd's native version selection. Script deletion returns 409 while an executed Version retains generation
background references; deletion can proceed after that generation ends. Automatic local collection of
child logs is a platform feature, not Cloudflare's default parent Workers Logs behavior.
