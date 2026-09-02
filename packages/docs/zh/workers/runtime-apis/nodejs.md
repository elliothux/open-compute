# Node.js 兼容

pinned baseline 已经提供 Node 兼容，因此 `node:` 导入不需要在项目中开启 flag。这不是 Node 主机：没有 `node_modules` 运行时、没有完整 Node 标准库作为操作系统、未实现的 API 不会被 polyfill。

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

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| 可用的 `node:` 模块集合 | 是，见 [Node.js compatibility](https://developers.cloudflare.com/workers/runtime-apis/nodejs/) | 与 pinned workerd 在该 compatibility date 上的表面对齐 |
| 成功 import 等于完整 Node | 否 | 否 |
| 每个 Worker 可关闭 `nodejs_compat` | 是 | 不提供；baseline 已包含 |
| 未实现的 Node API | 失败或受 flag 约束 | 失败，不会静默 polyfill |
| `node:net` 出站 | Cloudflare 托管网络策略 | 与通用出站相同，见 [TCP sockets](/zh/workers/runtime-apis/tcp-sockets) |
| 请求路径运行时 | workerd | workerd；不在 Bun/Node 中执行生产请求 |

