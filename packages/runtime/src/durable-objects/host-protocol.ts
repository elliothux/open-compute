import { bindingError } from "../loader/shared.js";
import type { SocketAuthorityWire } from "../sockets/tunnel.js";
import type {
  DoOrder,
  FacetClassDescriptor,
  TenantDoAuthority,
} from "./protocol.js";

export const INTERNAL = [
  "x-open-compute-binding-token",
  "x-open-compute-account-id",
  "x-open-compute-worker-id",
  "x-open-compute-binding-id",
  "x-open-compute-version-id",
  "x-open-compute-descriptor-sha256",
  "x-open-compute-worker-code-sha256",
  "x-open-compute-route-generation",
  "x-open-compute-namespace-resource-id",
  "x-open-compute-object-id",
  "x-open-compute-object-generation",
  "x-open-compute-class-name",
  "x-open-compute-do-method",
  "x-open-compute-do-url",
  "x-open-compute-do-operation",
  "x-open-compute-do-order-channel",
  "x-open-compute-do-order-sequence",
  "x-open-compute-request-id",
  "x-open-compute-startup-generation",
];
const FORBIDDEN_RPC = new Set([
  "constructor",
  "prototype",
  "__proto__",
  "then",
  "dup",
  "fetch",
  "connect",
  "alarm",
  "webSocketMessage",
  "webSocketClose",
  "webSocketError",
]);
export const ORDER_CHANNEL = /^[0-9a-f]{32}$/;
const ORDER_IDLE_MS = 60_000;
const MAX_ORDER_CHANNELS = 65_536;
const MAX_PENDING_OPERATIONS = 256;
const FACET_NAME_BYTES = 256;
const FACET_TREE_DEPTH = 4;
export const FACET_ENTRYPOINT = /^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/;
export const FACET_TOKEN = /^[0-9a-f]{32}$/;
const encoder = new TextEncoder();
interface PendingOperation {
  resolve: () => void;
}
export interface OrderState {
  next: number;
  expiresAt: number;
  pending: Map<number, PendingOperation>;
}
export interface RegisteredFacet {
  logicalPath: readonly string[];
  physicalName: string;
}
interface PendingTenantConnect {
  kind: "tenant";
  connectAuthority: SocketAuthorityWire;
  authority: TenantDoAuthority;
  expiresAt: number;
  order: DoOrder;
}
interface PendingFacetConnect {
  kind: "facet";
  connectAuthority: SocketAuthorityWire;
  authority: TenantDoAuthority;
  descriptor: FacetClassDescriptor;
  expiresAt: number;
  logicalPath: readonly string[];
}
export type PendingConnect = PendingTenantConnect | PendingFacetConnect;

function facetName(value: unknown): string {
  if (
    typeof value !== "string" ||
    encoder.encode(value).byteLength > FACET_NAME_BYTES
  ) {
    throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
  }
  return value;
}

export function facetPath(value: unknown): readonly string[] {
  if (
    !Array.isArray(value) ||
    value.length < 1 ||
    value.length > FACET_TREE_DEPTH - 1 ||
    value.some(
      (name) =>
        typeof name !== "string" ||
        encoder.encode(name).byteLength > FACET_NAME_BYTES,
    )
  ) {
    throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
  }
  return Object.freeze([...value]);
}

export function childFacetPath(
  parent: unknown,
  name: unknown,
): readonly string[] {
  const raw = Array.isArray(parent) ? parent : [];
  if (raw.length >= FACET_TREE_DEPTH - 1)
    throw bindingError("DO_RUNTIME_EXCEPTION");
  return facetPath([...raw, facetName(name)]);
}

export function pathPrefix(
  path: readonly string[],
  prefix: readonly string[],
): boolean {
  return (
    prefix.length <= path.length &&
    prefix.every((name, index) => path[index] === name)
  );
}

export function validateDescriptor(value: unknown): FacetClassDescriptor {
  if (value === null || typeof value !== "object")
    throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
  const entrypoint = Reflect.get(value, "entrypoint");
  const id = Reflect.get(value, "id");
  if (
    Reflect.get(value, "native") === true &&
    typeof id === "string" &&
    id.length <= 2048
  ) {
    return Object.freeze({ native: true, id });
  }
  if (
    typeof entrypoint !== "string" ||
    !FACET_ENTRYPOINT.test(entrypoint) ||
    typeof id !== "string" ||
    id.length > 2048
  ) {
    throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
  }
  return Object.freeze({ entrypoint, id, props: Reflect.get(value, "props") });
}

export async function physicalFacetName(
  logicalPath: readonly string[],
): Promise<string> {
  const encoded = encoder.encode(JSON.stringify(logicalPath));
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", encoded));
  let name = "f-";
  for (const byte of digest) name += byte.toString(16).padStart(2, "0");
  return name;
}

export function assertOrder(order: unknown): asserts order is DoOrder {
  if (!order || typeof order !== "object")
    throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
  const candidate = order as Partial<DoOrder>;
  if (
    typeof candidate.channelId !== "string" ||
    !ORDER_CHANNEL.test(candidate.channelId) ||
    !Number.isSafeInteger(candidate.sequence) ||
    candidate.sequence! < 0
  ) {
    throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
  }
}

function grantNextOperation(state: OrderState): void {
  const pending = state.pending.get(state.next);
  if (!pending) return;
  state.pending.delete(state.next);
  state.next += 1;
  pending.resolve();
}

export function ordered<T>(
  states: Map<string, OrderState>,
  order: DoOrder,
  run: () => Promise<T>,
): Promise<T> {
  assertOrder(order);
  const now = Date.now();
  let state = states.get(order.channelId);
  if (!state) {
    for (const [channelId, candidate] of states) {
      if (candidate.pending.size === 0 && candidate.expiresAt <= now)
        states.delete(channelId);
    }
    if (states.size >= MAX_ORDER_CHANNELS)
      throw bindingError("DO_STORAGE_LIMIT");
    state = { next: 0, expiresAt: now + ORDER_IDLE_MS, pending: new Map() };
    states.set(order.channelId, state);
  }
  if (
    order.sequence < state.next ||
    state.pending.has(order.sequence) ||
    state.pending.size >= MAX_PENDING_OPERATIONS
  ) {
    throw bindingError("DO_RUNTIME_EXCEPTION");
  }
  state.expiresAt = now + ORDER_IDLE_MS;
  if (order.sequence === state.next) {
    state.next += 1;
    let value: Promise<T>;
    try {
      value = run();
    } catch (error) {
      grantNextOperation(state);
      throw error;
    }
    grantNextOperation(state);
    return value;
  }
  const turn = new Promise<void>((resolve) => {
    state!.pending.set(order.sequence, { resolve });
  });
  return turn.then(() => {
    let value: Promise<T>;
    try {
      value = run();
    } catch (error) {
      grantNextOperation(state);
      throw error;
    }
    grantNextOperation(state);
    return value;
  });
}

export function assertRpcMember(member: unknown): asserts member is string {
  if (
    typeof member !== "string" ||
    FORBIDDEN_RPC.has(member) ||
    member.startsWith("__openCompute")
  ) {
    throw bindingError("DO_RPC_UNSUPPORTED");
  }
}

export function required(
  headers: Headers,
  name: string,
  pattern: RegExp,
): string {
  const value = headers.get(name) || "";
  if (!pattern.test(value)) throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
  return value;
}

export function authorityFromHeaders(headers: Headers) {
  const accountId = required(
    headers,
    "x-open-compute-account-id",
    /^[0-9a-f-]{36}$/,
  );
  const workerId = required(
    headers,
    "x-open-compute-worker-id",
    /^[0-9a-f-]{36}$/,
  );
  const versionId = required(
    headers,
    "x-open-compute-version-id",
    /^[0-9a-f-]{36}$/,
  );
  const workerCodeSha256 = required(
    headers,
    "x-open-compute-worker-code-sha256",
    /^[0-9a-f]{64}$/,
  );
  const objectId = required(
    headers,
    "x-open-compute-object-id",
    /^[0-9a-f]{64}$/,
  );
  const namespaceResourceId = required(
    headers,
    "x-open-compute-namespace-resource-id",
    /^[0-9a-f-]{36}$/,
  );
  const className = required(
    headers,
    "x-open-compute-class-name",
    /^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/,
  );
  const routeGeneration = Number(
    headers.get("x-open-compute-route-generation"),
  );
  const objectGeneration = Number(
    headers.get("x-open-compute-object-generation"),
  );
  if (
    !Number.isSafeInteger(routeGeneration) ||
    routeGeneration < 1 ||
    !Number.isSafeInteger(objectGeneration) ||
    objectGeneration < 1
  ) {
    throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
  }
  return {
    accountId,
    workerId,
    versionId,
    workerCodeSha256,
    objectId,
    namespaceResourceId,
    className,
    routeGeneration,
    objectGeneration,
    loaderKey: `${accountId}/${workerId}/${versionId}`,
  };
}

export function deleteAuthorityFromHeaders(headers: Headers) {
  const objectId = required(
    headers,
    "x-open-compute-object-id",
    /^[0-9a-f]{64}$/,
  );
  const objectGeneration = Number(
    headers.get("x-open-compute-object-generation"),
  );
  if (!Number.isSafeInteger(objectGeneration) || objectGeneration < 1) {
    throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
  }
  return { objectId, objectGeneration };
}
