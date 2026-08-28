import type { LoaderEnv } from "../loader/protocol.js";

/** Operator policy text embedded by the checked-in Cap'n Proto configuration. */
export interface DoPolicyEnv {
  DO_MAX_OBJECT_NAME_BYTES: string;
  DO_MAX_RPC_REQUEST_BYTES: string;
  DO_MAX_RPC_RESPONSE_BYTES: string;
  DO_MAX_FETCH_BODY_BYTES: string;
  DO_DISPATCH_TIMEOUT_MS: string;
  DO_MAX_IN_FLIGHT_DISPATCHES: string;
}
export interface DoPolicy {
  maxObjectNameBytes: number;
  maxRpcRequestBytes: number;
  maxRpcResponseBytes: number;
  maxFetchBodyBytes: number;
  dispatchTimeoutMs: number;
  maxInFlightDispatches: number;
}
export interface DoHostEnv extends LoaderEnv {
  DO_HOST: DurableObjectNamespace<import("./host.js").DoHost>;
  DO_DISK_STOP_WRITES_PERCENT: string;
}

/** Supported plain RPC values; no executable values or service capabilities. */
export type DoPlainValue = null | string | boolean | number | ArrayBuffer | ArrayBufferView
  | DoPlainValue[] | { [key: string]: DoPlainValue };
export type DoWireValue = ["z"] | ["s", string] | ["b", boolean] | ["n", number]
  | ["x", string] | ["a", DoWireValue[]] | ["o", [string, DoWireValue][]];
export interface DoRawTransport {
  dispatchRpc(objectId: string, method: string, args: DoWireValue): Promise<unknown>;
  fetch(request: Request): Promise<Response>;
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
export interface ResolvedDoAuthority extends AlarmIdentity {
  accountId: string;
  workerId: string;
  deploymentId: string;
  workerCodeSha256: string;
  routeGeneration: number;
  className: string;
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
