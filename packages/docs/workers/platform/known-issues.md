# Known issues

Current limitations of this binary. This page is not a roadmap.

```sh
bun run oc run --config examples/hello-worker/open-compute.json --ocd "$PWD/target/debug/ocd"
```

Re-run that command after source changes.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Module isolates | Yes | Yes |
| Watch / HMR (`wrangler dev`) | Yes | Not provided; `oc run` does not start workerd and does not watch files |
| Project file | wrangler.jsonc (comments allowed) | `open-compute.json`; unknown fields rejected; no `compatibilityDate`, no full Wrangler product keys, no jsonc comments |
| Control plane | Cloudflare control plane | Local `ocd` HTTP API (`/v1/account`, `/v1/accounts/.../workers`) |
| workers.dev | Yes | Not provided |
| Vite plugin as this product's dev server | `@cloudflare/vite-plugin` | Not provided; framework output is imported via `frameworkOutput` |
| Outbound network policy | Cloudflare hosted TCP policy | See [TCP sockets](/workers/runtime-apis/tcp-sockets) |
| Request-scoped CPU / subrequest quotas | Yes | Not enforced; see [Limits](/workers/platform/limits) |

Comparing to Cloudflare's [known issues](https://developers.cloudflare.com/workers/platform/known-issues/) is not useful. That list is for their hosted fleet.

