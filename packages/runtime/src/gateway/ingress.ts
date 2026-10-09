import { WorkerEntrypoint } from "cloudflare:workers";
import type { DoRouterRpc } from "../durable-objects/protocol.js";
import {
  inboundSocketAddress,
  tunnelSockets,
  type SocketAuthorityWire,
} from "../sockets/tunnel.js";
import { tokenEquals } from "./token.js";

const READY_PATH = "/internal/ready";
const LIVE_PATH = "/internal/live";
const TOKEN_HEADER = "x-open-compute-internal-token";
const INTERNAL_PATHS = new Set([
  "/internal/dispatch",
  "/internal/validate",
  "/internal/prepare-python",
  "/internal/probe",
  "/internal/validate-do",
  "/internal/queue",
  "/internal/scheduled",
  "/internal/workflow",
  "/internal/validate-workflow",
  "/internal/worker-loaders/revoke",
]);
const DO_ADMIN_PATH = "/internal/do-delete";
const DO_ALARM_PATHS = new Set([
  "/internal/do-alarm",
  "/internal/do-alarm-repair",
]);

function deny() {
  return new Response(null, { status: 404 });
}

/** The only socket entrypoint, including native RPC delegated by the prepare child. */
export default class Ingress extends WorkerEntrypoint<{
  INTERNAL_TOKEN: string;
  LOADER_HOST: Fetcher;
  DO_ROUTER: DoRouterRpc;
  BROWSER_ACTIONS: Fetcher;
}> {
  // Workerd admits RPC only through the generation's opaque capnpConnectHost.
  // These methods retain the main process's DO router and storage authority.
  dispatchFetch(identity: Record<string, string>, request: Request) {
    return this.env.DO_ROUTER.dispatchFetch(identity, request);
  }

  dispatchRpc(
    identity: Record<string, string>,
    method: string,
    args: unknown[],
  ) {
    return this.env.DO_ROUTER.dispatchRpc(identity, method, args);
  }

  getRpcProperty(identity: Record<string, string>, property: string) {
    return this.env.DO_ROUTER.getRpcProperty(identity, property);
  }

  prepareConnect(
    identity: Record<string, string>,
    authority: SocketAuthorityWire,
  ) {
    return this.env.DO_ROUTER.prepareConnect(identity, authority);
  }

  cancelOrder(identity: Record<string, string>) {
    return this.env.DO_ROUTER.cancelOrder(identity);
  }

  async connect(socket: Socket): Promise<void> {
    try {
      const address = await inboundSocketAddress(socket);
      const target = this.env.DO_ROUTER.connect(address, {
        allowHalfOpen: true,
      });
      await target.opened;
      await tunnelSockets(socket, target);
    } catch {
      await socket.close().catch(() => undefined);
      throw Object.assign(new Error("DO_RUNTIME_EXCEPTION"), {
        stableCode: "DO_RUNTIME_EXCEPTION",
      });
    }
  }

  async fetch(request: Request): Promise<Response> {
    const env = this.env;
    const url = new URL(request.url);
    const presented = request.headers.get(TOKEN_HEADER);
    if (!tokenEquals(presented, env.INTERNAL_TOKEN)) {
      return deny();
    }
    if (
      request.method === "GET" &&
      (url.pathname === READY_PATH || url.pathname === LIVE_PATH) &&
      url.search === ""
    ) {
      // Readiness gates admission; liveness proves the event loop, system Worker dispatch,
      // and generation credential still complete one minimal exchange. Neither touches
      // SQLite, S3, or any external dependency.
      if (request.headers.has("content-type")) return deny();
      const length = request.headers.get("content-length");
      if (length !== null && length !== "0") return deny();
      const body = await request.arrayBuffer();
      return body.byteLength === 0
        ? new Response(null, { status: 204 })
        : deny();
    }
    if (
      url.pathname === "/internal/browser-action" &&
      request.method === "POST" &&
      url.search === ""
    ) {
      const headers = new Headers(request.headers);
      headers.set("x-open-compute-startup-generation", env.INTERNAL_TOKEN);
      return env.BROWSER_ACTIONS.fetch(new Request(request, { headers }));
    }
    if (url.pathname === "/internal/do/v1/fetch" && url.search === "") {
      const headers = new Headers(request.headers);
      headers.delete(TOKEN_HEADER);
      return env.DO_ROUTER.fetch(new Request(request, { headers }));
    }
    const websocket =
      request.method === "GET" &&
      url.pathname === "/internal/dispatch" &&
      request.headers.get("upgrade")?.toLowerCase() === "websocket";
    if (
      (request.method !== "POST" && !websocket) ||
      !INTERNAL_PATHS.has(url.pathname) ||
      url.search !== ""
    ) {
      if (
        request.method === "POST" &&
        DO_ALARM_PATHS.has(url.pathname) &&
        url.search === ""
      ) {
        return env.DO_ROUTER.fetch(
          new Request(`http://do-router${url.pathname}`, {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: request.body,
          }),
        );
      }
      if (
        request.method === "POST" &&
        url.pathname === DO_ADMIN_PATH &&
        url.search === ""
      ) {
        return env.DO_ROUTER.fetch(
          new Request("http://do-router/internal/do-delete", {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: request.body,
          }),
        );
      }
      return deny();
    }
    const headers = new Headers(request.headers);
    // Forward only the authenticated generation token to the platform-owned
    // loader host. The host removes it before constructing the tenant Request.
    headers.set(TOKEN_HEADER, env.INTERNAL_TOKEN);
    return env.LOADER_HOST.fetch(
      new Request(`http://loader-host${url.pathname}`, {
        method: websocket ? "GET" : "POST",
        headers,
        body: websocket ? null : request.body,
        redirect: "manual",
      }),
    );
  }
}

/** A private service endpoint with no ambient IP or socket capability. */
export class NoOutbound extends WorkerEntrypoint {
  fetch(): Response {
    return new Response(null, { status: 404 });
  }
}
