import type { LoaderEnv } from "../loader/protocol.js";
import type { SocketAuthorityWire } from "../sockets/tunnel.js";

/** Operator policy text embedded by the checked-in Cap'n Proto configuration. */
export interface DoPolicyEnv {
  DO_MAX_OBJECT_NAME_BYTES: string;
  DO_MAX_FETCH_BODY_BYTES: string;
  DO_DISPATCH_TIMEOUT_MS: string;
  DO_MAX_IN_FLIGHT_DISPATCHES: string;
}
export interface DoPolicy {
  maxObjectNameBytes: number;
  maxFetchBodyBytes: number;
  dispatchTimeoutMs: number;
  maxInFlightDispatches: number;
}
export interface DoHostEnv extends LoaderEnv {
  DO_HOST: DurableObjectNamespace<import("./host.js").DoHost>;
  DO_DISK_STOP_WRITES_PERCENT: string;
}

/** Native transport with explicit per-stub ordering coordinates. */
export interface DoRawTransport {
  startRpc(
    objectId: string,
    channelId: string,
    sequence: number,
    kind: "call" | "get",
    method: string,
    args: unknown[],
  ): DoRpcResultProvider;
  cancelOrder(objectId: string, channelId: string, sequence: number): Promise<void>;
  prepareConnect(
    objectId: string,
    channelId: string,
    sequence: number,
    operationId: string,
    authority: SocketAuthorityWire,
  ): Promise<void>;
  cancelConnect(operationId: string): Promise<void>;
  fetch(request: Request): Promise<Response>;
  connect(address: SocketAddress | string, options?: SocketOptions): Socket;
}
export interface DoRpcResultStub extends Disposable {}
export interface DoRpcResultProvider extends Promise<DoRpcResultStub> {
  take(): unknown;
}
export interface DoPreparedConnect {
  tokenAddress: string;
}
export interface DoOrder {
  channelId: string;
  sequence: number;
}
/** Service binding RPC surface used by tenant DoTransport. */
export interface DoRouterRpc extends Fetcher {
  dispatchFetch(identity: Record<string, string>, request: Request): Promise<Response>;
  dispatchRpc(identity: Record<string, string>, method: string, args: unknown[]): Promise<unknown>;
  getRpcProperty(identity: Record<string, string>, property: string): Promise<unknown>;
  prepareConnect(identity: Record<string, string>, authority: SocketAuthorityWire): Promise<DoPreparedConnect>;
  cancelOrder(identity: Record<string, string>): Promise<void>;
}
export interface DoNamespaceCapability {
  schemaVersion: 1;
  namespacePrefix: string;
  namespaceNameKey: string;
  maxObjectNameBytes: number;
  transport: DoRawTransport;
}
export interface AlarmProjection { scheduledTimeMs: number; retryCount: number; rowToken: string }
export interface AlarmIdentity { namespaceResourceId: string; objectId: string; objectGeneration: number }
export interface AlarmIndexCapability {
  upsert(row: AlarmProjection): Promise<void>;
  delete(rowToken: string): Promise<void>;
  clear(): Promise<void>;
}
export interface LoadedDurableObject extends Rpc.DurableObjectBranded {
  fetch(request: Request): Promise<Response>;
  __openComputeAlarm(payload: unknown): Promise<unknown>;
  __openComputeAlarmRepair(): Promise<unknown>;
}
export interface FacetClassDescriptor {
  entrypoint: string;
  id: string;
  props: unknown;
}
export interface FacetManagerCapability extends Fetcher {
  __openComputeFacetCall(
    authority: TenantDoAuthority,
    path: readonly string[],
    descriptor: FacetClassDescriptor,
    method: string,
    args: unknown[],
  ): Promise<unknown>;
  __openComputeFacetGet(
    authority: TenantDoAuthority,
    path: readonly string[],
    descriptor: FacetClassDescriptor,
    property: string,
  ): Promise<unknown>;
  __openComputeFacetFetch(
    authority: TenantDoAuthority,
    path: readonly string[],
    descriptor: FacetClassDescriptor,
    request: Request,
  ): Promise<Response>;
  __openComputeFacetAbort(
    authority: TenantDoAuthority,
    parentPath: readonly string[],
    name: string,
    reason: unknown,
  ): Promise<void>;
  __openComputeFacetDelete(
    authority: TenantDoAuthority,
    parentPath: readonly string[],
    name: string,
  ): Promise<void>;
  __openComputeFacetClone(
    authority: TenantDoAuthority,
    parentPath: readonly string[],
    source: string,
    destination: string,
  ): Promise<void>;
  __openComputePrepareFacetConnect(
    authority: TenantDoAuthority,
    path: readonly string[],
    descriptor: FacetClassDescriptor,
    token: string,
    socket: SocketAuthorityWire,
  ): Promise<void>;
}
export interface TenantDoAuthority extends AlarmIdentity {
  accountId: string;
  workerId: string;
  deploymentId: string;
  workerCodeSha256: string;
  routeGeneration: number;
  className: string;
}
export interface ResolvedDoAuthority extends TenantDoAuthority {
  hostKey: string;
}

declare global {
  namespace Cloudflare {
    interface GlobalProps {
      mainModule: typeof import("./router.js");
      durableNamespaces: "DoHost";
    }
  }
}
