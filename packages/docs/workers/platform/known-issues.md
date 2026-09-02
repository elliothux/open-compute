# Known issues

Current limitations of this binary. This page is not a roadmap.

```sh
bun run oc deploy --config examples/hello-worker/wrangler.jsonc
```

Re-run that command after source changes.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Module isolates | Yes | Yes |
| Watch / HMR (`wrangler dev`) | Yes | Not provided; `oc deploy` does not start workerd and does not watch files |
| Project file | `wrangler.jsonc` | Same pinned Wrangler JSONC schema; unsupported server capabilities fail closed |
| Control plane | Cloudflare control plane | Supported Cloudflare v4 subset at `/client/v4` plus documented extension operations |
| workers.dev | Yes | Not provided |
| Vite plugin as this product's dev server | `@cloudflare/vite-plugin` | Framework adapters hand off through `.wrangler/deploy/config.json` |
| Outbound network policy | Cloudflare hosted TCP policy | See [TCP sockets](/workers/runtime-apis/tcp-sockets) |
| Request-scoped CPU / subrequest quotas | Yes | Not enforced; see [Limits](/workers/platform/limits) |

Comparing to Cloudflare's [known issues](https://developers.cloudflare.com/workers/platform/known-issues/) is not useful. That list is for their hosted fleet.
