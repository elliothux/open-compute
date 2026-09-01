# Known issues

An honest list, not a roadmap.

```sh
bun run oc run --config examples/hello-worker/open-compute.json --ocd "$PWD/target/debug/ocd"
```

Re-run that command after source changes. There is no `wrangler dev` hot reload.

## Same as Cloudflare

Workers are still module isolates. This page occupies the same nav slot as Cloudflare [known issues](https://developers.cloudflare.com/workers/platform/known-issues/); it is not the same list.

## Intentional delta

- **No watch / HMR.** Re-run `bun run oc run ...` after source changes. `oc run` does not start workerd and does not watch files.
- **`open-compute.json` ≠ `wrangler.jsonc`.** Unknown fields are rejected. No `compatibilityDate`, no full Wrangler product keys, no jsonc comments.
- **No Cloudflare REST v4.** The control plane is this machine's `ocd` HTTP API (`/v1/account`, `/v1/accounts/.../workers`). Do not send Wrangler / Cloudflare API tokens at this platform.
- **No playground, no dashboard editor, no `workers.dev`.**
- **No Vite plugin as this product's dev server.** Framework output is imported via `frameworkOutput`, not the `@cloudflare/vite-plugin` Wrangler workflow.
- **Outbound policy is not hosted Cloudflare TCP policy.** See [`OC-WKR-TCP-001`](/en/workers/runtime-apis/tcp-sockets).
- **Request-scoped CPU / subrequest quotas are not enforced.** See [`OC-WKR-LIMIT-001`](/en/workers/platform/limits).

Comparing to Cloudflare's hosted-fleet known issues is not useful. This page is what you hit on this binary.
