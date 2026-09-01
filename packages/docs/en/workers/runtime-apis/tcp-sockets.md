# TCP sockets

`connect()` is imported from `cloudflare:sockets` for outbound TCP. The API shape matches Cloudflare; the policy boundary does not.

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

Full `Socket` / `SocketAddress` / `SocketOptions` / `startTls()` signatures: [Cloudflare TCP sockets](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/). Do not restate them here. `node:net` / `node:tls` share the same general outbound. Named Service/DO `Fetcher.connect()` must use a declared capability tunnel; it is not a second general outbound path.

## Same as Cloudflare

`connect(address, options?)` returns a `Socket` with `readable` / `writable` / `opened` / `closed` / `close()` / `startTls()`. `secureTransport`: `off` | `on` | `starttls`. Do not create a socket in global scope and share it across requests.

## Intentional delta: OC-WKR-TCP-001

Tenant general outbound `fetch()`, `cloudflare:sockets.connect()`, and `node:net` share one stock-workerd `Network(allow = ["public"])` address authority. Named Service/DO `Fetcher.connect()` uses an explicitly declared capability tunnel and is not a second general outbound path. Unlike [Cloudflare's hosted TCP policy](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/#troubleshooting), open-compute does not add Cloudflare-owned IP-range blocking, a Worker self-connect/TCP-loop detector, or the default SMTP port 25 prohibition. Runtime-source, binding-backend, and workerd-internal listeners are loopback-only. Control/data listeners default to loopback but an operator may explicitly expose them, so the public Network does not add an ownership-based rejection for such public addresses. The operator owns exposed ingress and any additional public-IP, reverse-proxy, or SMTP egress policy.

So Cloudflare docs that say “Cloudflare IPs are blocked”, “TCP Loop detected”, or “Connections to port 25 are prohibited” are not policies this binary already enforces. The public address layer still rejects private / loopback / link-local / metadata / Unix.
