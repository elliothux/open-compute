# Node.js 兼容

pinned baseline 已经提供 Node 兼容，因此 `node:` 导入不需要你在项目里开 flag。这不是 Node 主机：没有 `node_modules` 运行时、没有完整 Node 标准库作为操作系统、未实现的 API 不会被 polyfill。

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

工具链不提供 Node 运行环境、不填补未实现的产品 API、也不下载远程 import。

## 与 Cloudflare 相同

可用的 `node:` 模块集合跟 pinned workerd / [Node.js compatibility](https://developers.cloudflare.com/workers/runtime-apis/nodejs/) 在该 compatibility date 上的表面对齐。要查某个模块，去 Cloudflare 对应页（`buffer`、`crypto`、`path`、`stream`、`net` 等），不要假设“能 import 就等于完整 Node”。

## 故意不同

没有 per-Worker `nodejs_compat` flag 可关。未实现的 Node API 失败，而不是静默填 polyfill。`node:net` 出网仍受 [`OC-WKR-TCP-001`](/workers/runtime-apis/tcp-sockets) 约束。Worker 不是在 Bun/Node 里执行；生产请求路径不调用 Bun 或 Node。
