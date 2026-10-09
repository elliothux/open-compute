import { WorkerEntrypoint } from "cloudflare:workers";
import {
  inboundSocketAddress,
  tunnelSockets,
  type SocketAuthorityWire,
} from "../sockets/tunnel.js";
import type {
  DoHostEnv,
  FacetClassDescriptor,
  TenantDoAuthority,
} from "./protocol.js";

/** Retain host identity, never the actor stub that would prevent idle eviction. */
export class FacetManager extends WorkerEntrypoint<
  DoHostEnv,
  { hostId: string }
> {
  get #host() {
    return this.env.DO_HOST.get(
      this.env.DO_HOST.idFromString(this.ctx.props.hostId),
    );
  }

  async fetch(request: Request): Promise<Response> {
    return this.#host.fetch(request);
  }

  async connect(socket: Socket): Promise<void> {
    const target = this.#host.connect(await inboundSocketAddress(socket), {
      allowHalfOpen: true,
    });
    await target.opened;
    await tunnelSockets(socket, target);
  }

  async __openComputePrepareNativeFacet(
    authority: TenantDoAuthority,
    path: readonly string[],
  ): Promise<string> {
    return this.#host.__openComputePrepareNativeFacet(authority, path);
  }

  async __openComputeFacetCall(
    authority: TenantDoAuthority,
    path: readonly string[],
    descriptor: FacetClassDescriptor,
    method: string,
    args: unknown[],
  ): Promise<unknown> {
    return this.#host.__openComputeFacetCall(
      authority,
      path,
      descriptor,
      method,
      args,
    );
  }

  async __openComputeFacetGet(
    authority: TenantDoAuthority,
    path: readonly string[],
    descriptor: FacetClassDescriptor,
    property: string,
  ): Promise<unknown> {
    return this.#host.__openComputeFacetGet(
      authority,
      path,
      descriptor,
      property,
    );
  }

  async __openComputeFacetFetch(
    authority: TenantDoAuthority,
    path: readonly string[],
    descriptor: FacetClassDescriptor,
    request: Request,
  ): Promise<Response> {
    return this.#host.__openComputeFacetFetch(
      authority,
      path,
      descriptor,
      request,
    );
  }

  async __openComputeFacetAbort(
    authority: TenantDoAuthority,
    parentPath: readonly string[],
    name: string,
    reason: unknown,
  ): Promise<void> {
    return this.#host.__openComputeFacetAbort(
      authority,
      parentPath,
      name,
      reason,
    );
  }

  async __openComputeFacetDelete(
    authority: TenantDoAuthority,
    parentPath: readonly string[],
    name: string,
  ): Promise<void> {
    return this.#host.__openComputeFacetDelete(authority, parentPath, name);
  }

  async __openComputeFacetClone(
    authority: TenantDoAuthority,
    parentPath: readonly string[],
    source: string,
    destination: string,
  ): Promise<void> {
    return this.#host.__openComputeFacetClone(
      authority,
      parentPath,
      source,
      destination,
    );
  }

  async __openComputePrepareFacetConnect(
    authority: TenantDoAuthority,
    path: readonly string[],
    descriptor: FacetClassDescriptor,
    token: string,
    socket: SocketAuthorityWire,
  ): Promise<void> {
    return this.#host.__openComputePrepareFacetConnect(
      authority,
      path,
      descriptor,
      token,
      socket,
    );
  }

  async __openComputeCancelFacetConnect(token: string): Promise<void> {
    return this.#host.__openComputeCancelFacetConnect(token);
  }
}
