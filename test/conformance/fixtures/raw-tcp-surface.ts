import { connect } from "cloudflare:sockets";
import { DurableObject, WorkerEntrypoint } from "cloudflare:workers";

export class SocketService extends WorkerEntrypoint {
  async connect(socket: Socket): Promise<void> {
    const info: SocketInfo = await socket.opened;
    const writer = socket.writable.getWriter();
    await writer.write(new TextEncoder().encode(info.localAddress ?? ""));
    await writer.close();
  }
}

export class SocketObject extends DurableObject {
  async connect(socket: Socket): Promise<void> {
    await socket.readable.pipeTo(socket.writable);
  }
}

interface Env {
  SERVICE: Service<typeof SocketService>;
  OBJECTS: DurableObjectNamespace<SocketObject>;
}

export default {
  async connect(socket: Socket, _env: Env, ctx: ExecutionContext): Promise<void> {
    ctx.waitUntil(socket.closed);
  },

  async fetch(_request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const direct = connect("example.com:443", {
      secureTransport: "on",
      allowHalfOpen: true,
      highWaterMark: 4096n,
    });
    const byObject = connect({ hostname: "example.com", port: 443 }, {
      secureTransport: "starttls",
      allowHalfOpen: false,
    });
    const tls = byObject.startTls({ expectedServerHostname: "example.com" });
    const service = env.SERVICE.connect({ hostname: "service.invalid", port: 1 });
    const object = env.OBJECTS.getByName("socket").connect("object.invalid:1", {
      allowHalfOpen: true,
    });
    const loopback = ctx.exports.SocketService.connect("loopback.invalid:1");
    const opened: Promise<SocketInfo>[] = [direct.opened, byObject.opened, tls.opened, service.opened, object.opened, loopback.opened];
    const closed: Promise<void>[] = [direct.closed, byObject.closed, tls.closed, service.closed, object.closed, loopback.closed];
    const readable: ReadableStream = direct.readable;
    const writable: WritableStream = direct.writable;
    const transport: "on" | "off" | "starttls" = direct.secureTransport;
    const upgraded: boolean = direct.upgraded;
    ctx.waitUntil(Promise.allSettled([...opened, ...closed]).then(() => undefined));
    await direct.close();
    void readable;
    void writable;
    void transport;
    void upgraded;
    return new Response("ok");
  },
} satisfies ExportedHandler<Env>;

declare global {
  namespace Cloudflare {
    interface GlobalProps {
      mainModule: typeof import("./raw-tcp-surface.js");
      durableNamespaces: "SocketObject";
    }
  }
}
