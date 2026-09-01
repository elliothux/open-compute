# TCP sockets

`connect()` 从 `cloudflare:sockets` 导入，用来建立出站 TCP。API 形状与 Cloudflare 相同；政策边界不是。

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

完整 `Socket` / `SocketAddress` / `SocketOptions` / `startTls()` 签名见 [Cloudflare TCP sockets](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/)，不要在本页复述。`node:net` / `node:tls` 走同一条 general outbound。命名 Service/DO 的 `Fetcher.connect()` 必须走声明过的 capability tunnel，不是第二条通用出网。

## 与 Cloudflare 相同

`connect(address, options?)` 返回带 `readable` / `writable` / `opened` / `closed` / `close()` / `startTls()` 的 `Socket`。`secureTransport`: `off` | `on` | `starttls`。不要在 global scope 创建并跨请求共享 socket。

## 故意不同：OC-WKR-TCP-001

tenant 的 general outbound `fetch()`、`cloudflare:sockets.connect()` 和 `node:net` 共享唯一的 stock-workerd `Network(allow = ["public"])`；命名 Service/DO 的 `Fetcher.connect()` 走声明式 capability tunnel，不是第二个通用 outbound。open-compute 不复制 Cloudflare 自有 IP 段封禁、Worker self-connect/TCP-loop detector 或默认 SMTP 25 封禁。runtime-source、binding-backend 和 workerd 内部 listener 强制 loopback；control/data listener 默认 loopback，但 operator 可以显式暴露，因此不能宣称 public Network 会按“平台所有权”额外拒绝公开地址。operator 负责公开入口和额外公网/SMTP egress policy。

因此 Cloudflare 文档里的 “Cloudflare IPs are blocked”、“TCP Loop detected”、“Connections to port 25 are prohibited” 不能当成这个二进制已经执行的托管策略。public 地址层仍拒绝 private / loopback / link-local / metadata / Unix。
