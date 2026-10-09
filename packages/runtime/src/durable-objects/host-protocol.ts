import { bindingError } from "../loader/shared.js";
import type { SocketAuthorityWire } from "../sockets/tunnel.js";
import { sanitizeDoError } from "./errors.js";
import { identityFromHeaders, OBJECT_IDENTITY_HEADER } from "./identity.js";
import type {
  DoOrder,
  FacetClassDescriptor,
  LoadedDurableObject,
  TenantDoAuthority,
} from "./protocol.js";

export const INTERNAL = [
  OBJECT_IDENTITY_HEADER,
  "x-open-compute-binding-token",
  "x-open-compute-instance-id",
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
  reject: (error: Error) => void;
}
export interface OrderState {
  next: number;
  starting: boolean;
  expiresAt: number;
  pending: Map<number, PendingOperation>;
  skipped: Set<number>;
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

function advanceOrderCursor(state: OrderState): void {
  while (state.skipped.has(state.next)) {
    state.skipped.delete(state.next);
    state.next += 1;
  }
}

function grantNextOperation(state: OrderState): void {
  if (state.starting) return;
  advanceOrderCursor(state);
  const pending = state.pending.get(state.next);
  if (!pending) return;
  state.pending.delete(state.next);
  state.next += 1;
  state.starting = true;
  pending.resolve();
}

/** Drop a queued or future order slot without running tenant work. */
export function cancelOrderedOperation(
  states: Map<string, OrderState>,
  order: DoOrder,
): void {
  assertOrder(order);
  const state = orderState(states, order.channelId);
  if (order.sequence < state.next) return;
  advanceOrderCursor(state);
  if (state.skipped.has(order.sequence)) return;
  const pending = state.pending.get(order.sequence);
  if (
    !pending &&
    order.sequence !== state.next &&
    state.pending.size + state.skipped.size >= MAX_PENDING_OPERATIONS
  )
    throw bindingError("DO_STORAGE_LIMIT");
  state.pending.delete(order.sequence);
  state.skipped.add(order.sequence);
  pending?.reject(bindingError("DO_RUNTIME_EXCEPTION"));
  grantNextOperation(state);
}

function orderState(
  states: Map<string, OrderState>,
  channelId: string,
): OrderState {
  const now = Date.now();
  let state = states.get(channelId);
  if (!state) {
    for (const [channelId, candidate] of states) {
      if (
        !candidate.starting &&
        candidate.pending.size === 0 &&
        candidate.expiresAt <= now
      )
        states.delete(channelId);
    }
    if (states.size >= MAX_ORDER_CHANNELS)
      throw bindingError("DO_STORAGE_LIMIT");
    state = {
      next: 0,
      starting: false,
      expiresAt: now + ORDER_IDLE_MS,
      pending: new Map(),
      skipped: new Set(),
    };
    states.set(channelId, state);
  }
  state.expiresAt = now + ORDER_IDLE_MS;
  return state;
}

export function ordered<T>(
  states: Map<string, OrderState>,
  order: DoOrder,
  run: (started: () => void) => Promise<T>,
  waitForStart = false,
): Promise<T> {
  assertOrder(order);
  const state = orderState(states, order.channelId);
  advanceOrderCursor(state);
  if (order.sequence < state.next || state.skipped.has(order.sequence)) {
    throw bindingError("DO_RUNTIME_EXCEPTION");
  }
  if (state.pending.has(order.sequence)) {
    throw bindingError("DO_RUNTIME_EXCEPTION");
  }
  if (
    (order.sequence !== state.next || state.starting) &&
    state.pending.size + state.skipped.size >= MAX_PENDING_OPERATIONS
  ) {
    throw bindingError("DO_STORAGE_LIMIT");
  }
  const start = () => {
    state.starting = true;
    let granted = false;
    const started = () => {
      if (!granted) {
        granted = true;
        state.starting = false;
        grantNextOperation(state);
      }
    };
    let value: Promise<T>;
    try {
      value = run(started);
    } catch (error) {
      started();
      throw error;
    }
    if (waitForStart) void value.then(started, started);
    else started();
    return value;
  };
  if (order.sequence === state.next && !state.starting) {
    state.next += 1;
    return start();
  }
  const turn = new Promise<void>((resolve, reject) => {
    state.pending.set(order.sequence, { resolve, reject });
  });
  return turn.then(start);
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
  const instanceId = required(
    headers,
    "x-open-compute-instance-id",
    /^[0-9a-f]{32}$/,
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
  const identity = identityFromHeaders(headers, objectId);
  return {
    objectName: identity.name,
    jurisdiction: identity.jurisdiction,
    instanceId,
    workerId,
    versionId,
    workerCodeSha256,
    objectId,
    namespaceResourceId,
    className,
    routeGeneration,
    objectGeneration,
    loaderKey: `${instanceId}/${workerId}/${versionId}`,
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

/** Preserve start order across the native RPC and HTTP event paths. */
export function orderedTenantRpc(
  states: Map<string, OrderState>,
  order: DoOrder,
  facet: Fetcher<LoadedDurableObject>,
  kind: "call" | "get",
  member: string,
  args: unknown[],
): Promise<unknown> {
  return ordered(
    states,
    order,
    async (started) => {
      try {
        return await facet.__openComputeInvokeRpc(
          kind,
          member,
          args,
          async () => started(),
        );
      } catch (error) {
        throw sanitizeDoError(error, "DO_RUNTIME_EXCEPTION");
      }
    },
    true,
  );
}

/** Acknowledge actual fetch admission without transferring WebSockets over RPC. */
export function orderedTenantFetch(
  states: Map<string, OrderState>,
  order: DoOrder,
  facet: Fetcher<LoadedDurableObject>,
  request: Request,
  timeoutMs: number,
): Promise<Response> {
  return ordered(
    states,
    order,
    async (started) => {
      const token = crypto.randomUUID();
      try {
        await facet.__openComputePrepareFetch(
          token,
          async () => started(),
          timeoutMs,
        );
        request.headers.set("x-open-compute-fetch-admission", token);
        return await facet.fetch(request);
      } catch (error) {
        throw sanitizeDoError(error, "DO_RUNTIME_EXCEPTION");
      } finally {
        await facet.__openComputeCancelFetch(token).catch(() => undefined);
      }
    },
    true,
  );
}
