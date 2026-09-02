# TCP sockets

`connect()` is imported from `cloudflare:sockets` for outbound TCP. The API shape matches [Cloudflare TCP sockets](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/). The network policy boundary does not.

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

Full `Socket` / `SocketAddress` / `SocketOptions` / `startTls()` signatures: [Cloudflare TCP sockets](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/). Do not create a socket in global scope and share it across requests.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| `connect(address, options?)` returns a `Socket` with `readable` / `writable` / `opened` / `closed` / `close()` / `startTls()` | Yes | Yes |
| `secureTransport`: `off` \| `on` \| `starttls` | Yes | Yes |
| Tenant general outbound `fetch()`, `cloudflare:sockets.connect()`, `node:net` | Cloudflare hosted network policy | Share one stock-workerd `Network(allow=["public"])` |
| Named Service/DO `Fetcher.connect()` | Hosted policy | Uses the declared capability tunnel; not a second general outbound |
| Cloudflare-owned IP-range block / Worker self-connect (TCP Loop) / default SMTP port 25 prohibition | Yes — [troubleshooting](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/#troubleshooting) | Not provided |
| Private / loopback / link-local / metadata / Unix | Rejected | Rejected by the public address layer |

