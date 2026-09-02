# Node.js compatibility

The pinned baseline already includes Node compatibility, so `node:` imports do not need a project-level flag. This is not a Node host: there is no `node_modules` runtime, no full Node standard library as an OS, and unimplemented APIs are not polyfilled.

```ts
import { Buffer } from "node:buffer";
import { createHash } from "node:crypto";

export default {
  fetch(): Response {
    const digest = createHash("sha256").update(Buffer.from("ok")).digest("hex");
    return new Response(digest);
  },
} satisfies ExportedHandler;
```

The toolchain does not provide a Node runtime, does not fill in unimplemented product APIs, and does not download remote imports.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Available `node:` modules | Yes — [Node.js compatibility](https://developers.cloudflare.com/workers/runtime-apis/nodejs/) | Aligned with pinned workerd at this compatibility date |
| Successful import means full Node | No | No |
| Per-Worker `nodejs_compat` flag to turn off | Yes | Not provided; included in the baseline |
| Unimplemented Node APIs | Fail or flag-gated | Fail; not silently polyfilled |
| `node:net` outbound | Cloudflare hosted network policy | Same general outbound; see [TCP sockets](/workers/runtime-apis/tcp-sockets) |
| Request-path runtime | workerd | workerd; production requests do not execute inside Bun/Node |

