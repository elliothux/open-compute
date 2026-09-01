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

## Same as Cloudflare

The available `node:` modules align with pinned workerd / [Node.js compatibility](https://developers.cloudflare.com/workers/runtime-apis/nodejs/) at this compatibility date. To check a module, use the matching Cloudflare page (`buffer`, `crypto`, `path`, `stream`, `net`, …). Do not assume that a successful import means full Node.

## Intentional delta

There is no per-Worker `nodejs_compat` flag to turn off. Unimplemented Node APIs fail; they are not silently polyfilled. `node:net` outbound is still [`OC-WKR-TCP-001`](/en/workers/runtime-apis/tcp-sockets). Workers do not execute inside Bun/Node; the production request path does not call Bun or Node.
