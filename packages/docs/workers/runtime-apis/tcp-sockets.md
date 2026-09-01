# TCP sockets

`connect()` 从 `cloudflare:sockets` 导入，用于建立出站 TCP。API 形状与 [Cloudflare TCP sockets](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/) 对齐；网络策略边界不同。

```ts
import { connect } from "cloudflare:sockets";

export default {
  async fetch(request: Request): Promise<Response> {
    const socket = connect({ hostname: "example.com", port: 80 });
    const writer = socket.writable.getWriter();
    await writer.write(new TextEncoder().encode("GET / HTTP/1.0\r\n\r\n"));
    await writer.close();
    return new Response(socket.readable, { headers: { "Content-Type": "text/plain" } });
  },
} satisfies ExportedHandler;
```

完整 `Socket` / `SocketAddress` / `SocketOptions` / `startTls()` 签名见 Cloudflare 原文。不可在 global scope 创建并跨请求共享 socket。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| `connect(address, options?)` 返回带 `readable` / `writable` / `opened` / `closed` / `close()` / `startTls()` 的 `Socket` | 是 | 是 |
| `secureTransport`：`off` \| `on` \| `starttls` | 是 | 是 |
| 租户通用出站 `fetch()`、`cloudflare:sockets.connect()`、`node:net` | Cloudflare 托管网络策略 | 共享唯一的 stock-workerd `Network(allow=["public"])` |
| 命名 Service/DO 的 `Fetcher.connect()` | 托管策略 | 使用已声明的 capability tunnel，不是第二条通用出站 |
| Cloudflare 自有 IP 段封禁 / Worker self-connect（TCP Loop）/ 默认 SMTP 25 封禁 | 是，见 [troubleshooting](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/#troubleshooting) | 不提供 |
| private / loopback / link-local / metadata / Unix | 拒绝 | public 地址层拒绝 |

