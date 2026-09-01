import { waitUntil } from "cloudflare:workers";
import { socketAuthorityWire } from "../sockets/tunnel.js";
import { loopbackDurableObjectMetadata } from "../loader/wrappers/runtime.js";
import type {
  FacetClassDescriptor, FacetManagerCapability, TenantDoAuthority,
} from "./protocol.js";

interface FacetStartupState {
  readonly callback: () => unknown;
  startup?: Promise<FacetClassDescriptor>;
  aborted?: Readonly<{ reason: unknown }>;
}
interface TenantFacetsState {
  readonly manager: FacetManagerCapability;
  readonly authority: TenantDoAuthority;
  readonly logicalPath: readonly string[];
  readonly inheritedId: unknown;
  barrier: Promise<void>;
}

const FACET_NAME_BYTES = 256;
const FACET_TREE_DEPTH = 4;
const ENTRYPOINT = /^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/;
const FORBIDDEN_RPC = new Set([
  "constructor", "prototype", "__proto__", "then", "dup", "fetch", "connect", "alarm",
  "webSocketMessage", "webSocketClose", "webSocketError",
]);
const encoder = new TextEncoder();
const tenantFacetsState = new WeakMap<object, TenantFacetsState>();

function ownerState(owner: TenantFacets): TenantFacetsState {
  const state = tenantFacetsState.get(owner);
  if (!state) throw new Error("DO_INTERNAL_PROTOCOL_ERROR");
  return state;
}

function settled(owner: TenantFacets): Promise<void> {
  return ownerState(owner).barrier;
}

function object(value: unknown): value is object {
  return value !== null && (typeof value === "object" || typeof value === "function");
}

function callable(value: unknown): value is (...args: unknown[]) => unknown {
  return typeof value === "function";
}

function facetName(value: unknown): string {
  if (typeof value !== "string") throw new TypeError("Facet name must be a string.");
  if (encoder.encode(value).byteLength > FACET_NAME_BYTES) {
    throw new TypeError(`Facet name is too long (max ${FACET_NAME_BYTES} characters).`);
  }
  return value;
}

function path(value: unknown): readonly string[] {
  if (!Array.isArray(value) || value.length > FACET_TREE_DEPTH - 1
      || value.some(name => typeof name !== "string" || encoder.encode(name).byteLength > FACET_NAME_BYTES)) {
    throw new Error("DO_INTERNAL_PROTOCOL_ERROR");
  }
  return Object.freeze([...value]);
}

function descriptor(
  raw: unknown,
  inheritedId: unknown,
): FacetClassDescriptor {
  if (raw === null || typeof raw !== "object") throw new TypeError("Invalid facet startup options.");
  const classValue = Reflect.get(raw, "class");
  const metadata = loopbackDurableObjectMetadata(classValue);
  if (!metadata || !ENTRYPOINT.test(metadata.entrypoint)) {
    throw new TypeError("Invalid Durable Object class for facet.");
  }
  const requestedId = Reflect.get(raw, "id");
  const idValue = requestedId === undefined ? inheritedId : requestedId;
  if (typeof idValue !== "string" && !object(idValue)) {
    throw new TypeError("Invalid Durable Object facet id.");
  }
  let id: string;
  try { id = String(idValue); }
  catch { throw new TypeError("Invalid Durable Object facet id."); }
  return Object.freeze({ entrypoint: metadata.entrypoint, id, props: metadata.props });
}

function safeError(error: unknown): Error {
  const message = error instanceof Error ? error.message : String(error);
  const code = /\b(DO_[A-Z_]+)\b/.exec(message)?.[1] ?? "DO_RUNTIME_EXCEPTION";
  const failure = Object.assign(new Error(code), { stableCode: code });
  failure.stack = `Error: ${code}`;
  return failure;
}

class FacetStubState {
  constructor(
    readonly owner: TenantFacets,
    readonly logicalPath: readonly string[],
    readonly startup: FacetStartupState,
  ) {}

  descriptor(): Promise<FacetClassDescriptor> {
    if (!this.startup.startup) {
      const inheritedId = ownerState(this.owner).inheritedId;
      this.startup.startup = Promise.resolve().then(this.startup.callback).then(value =>
        descriptor(value, inheritedId));
    }
    return this.startup.startup;
  }

  run<T>(operation: (descriptor: FacetClassDescriptor) => Promise<T>): Promise<T> {
    const checkAborted = () => {
      if (this.startup.aborted) throw this.startup.aborted.reason;
    };
    return settled(this.owner).then(() => {
      checkAborted();
      return this.descriptor();
    }).then(descriptor => {
      checkAborted();
      return operation(descriptor);
    }).catch(error => {
      if (this.startup.aborted && Object.is(error, this.startup.aborted.reason)) throw error;
      throw safeError(error);
    });
  }
}

function rpcMember(state: FacetStubState, member: string): (...args: unknown[]) => unknown {
  const owner = ownerState(state.owner);
  let property: Promise<unknown> | undefined;
  const getProperty = () => property ??= state.run(descriptor =>
    owner.manager.__openComputeFacetGet(
      owner.authority, state.logicalPath, descriptor, member,
    ));
  const method = (...args: unknown[]) => state.run(descriptor =>
    owner.manager.__openComputeFacetCall(
      owner.authority, state.logicalPath, descriptor, member, args,
    ));
  return new Proxy(method, {
    get(_target, nested) {
      const result = getProperty();
      const value: unknown = Reflect.get(result, nested, result);
      return callable(value) ? value.bind(result) : value;
    },
  });
}

function facetStub(state: FacetStubState): Fetcher {
  const owner = ownerState(state.owner);
  const target = Object.create(null) as Record<PropertyKey, unknown>;
  return new Proxy(target, {
    get(_owner, property) {
      if (property === "then") return undefined;
      if (property === "fetch") {
        return async (input: RequestInfo | URL, init?: RequestInit) => {
          const request = input instanceof Request && init === undefined ? input : new Request(input, init);
          return state.run(descriptor => owner.manager.__openComputeFacetFetch(
            owner.authority, state.logicalPath, descriptor, request,
          ));
        };
      }
      if (property === "connect") {
        return (address: SocketAddress | string, options?: SocketOptions): Socket => {
          const token = crypto.randomUUID().replaceAll("-", "");
          const prepared = state.run(descriptor => owner.manager.__openComputePrepareFacetConnect(
            owner.authority,
            state.logicalPath,
            descriptor,
            token,
            socketAuthorityWire(address),
          ));
          waitUntil(prepared.then(() => undefined, () => undefined));
          return owner.manager.connect(`${token}.facet-connect.invalid:1`, options);
        };
      }
      if (typeof property !== "string") return Reflect.get(target, property, target);
      if (FORBIDDEN_RPC.has(property) || property.startsWith("__openCompute")) {
        throw new TypeError("DO_RPC_UNSUPPORTED");
      }
      return rpcMember(state, property);
    },
  }) as Fetcher;
}

/** Cloudflare-compatible logical facets flattened below the platform-owned host actor. */
export class TenantFacets implements DurableObjectFacets {
  readonly #startups = new Map<string, FacetStartupState>();

  constructor(
    manager: FacetManagerCapability,
    authority: TenantDoAuthority,
    logicalPath: readonly string[],
    inheritedId: unknown,
  ) {
    tenantFacetsState.set(this, {
      manager, authority, logicalPath, inheritedId, barrier: Promise.resolve(),
    });
    Object.freeze(this);
  }

  #mutate(operation: () => Promise<void>): void {
    const owner = ownerState(this);
    owner.barrier = owner.barrier.then(operation);
    waitUntil(owner.barrier.catch(() => undefined));
  }

  get<T extends Rpc.DurableObjectBranded | undefined = undefined>(
    rawName: string,
    getStartupOptions: () => FacetStartupOptions<T> | Promise<FacetStartupOptions<T>>,
  ): Fetcher<T> {
    const name = facetName(rawName);
    const owner = ownerState(this);
    if (owner.logicalPath.length + 1 >= FACET_TREE_DEPTH) {
      throw new Error(`Facet nesting depth limit exceeded. The maximum depth including the root Durable Object is ${FACET_TREE_DEPTH}.`);
    }
    if (!callable(getStartupOptions)) throw new TypeError("Facet startup callback must be a function.");
    let startup = this.#startups.get(name);
    if (!startup) {
      startup = { callback: getStartupOptions };
      this.#startups.set(name, startup);
    }
    const logicalPath = Object.freeze([...owner.logicalPath, name]);
    return facetStub(new FacetStubState(this, logicalPath, startup)) as Fetcher<T>;
  }

  abort(rawName: string, reason: unknown): void {
    const name = facetName(rawName);
    const owner = ownerState(this);
    const startup = this.#startups.get(name);
    if (startup) startup.aborted = Object.freeze({ reason });
    this.#startups.delete(name);
    this.#mutate(() => owner.manager.__openComputeFacetAbort(
      owner.authority, owner.logicalPath, name, reason,
    ));
  }

  delete(rawName: string): void {
    const name = facetName(rawName);
    const owner = ownerState(this);
    this.#startups.delete(name);
    this.#mutate(() => owner.manager.__openComputeFacetDelete(
      owner.authority, owner.logicalPath, name,
    ));
  }

  clone(rawSource: string, rawDestination: string): void {
    const source = facetName(rawSource);
    const destination = facetName(rawDestination);
    const owner = ownerState(this);
    this.#startups.delete(destination);
    this.#mutate(() => owner.manager.__openComputeFacetClone(
      owner.authority, owner.logicalPath, source, destination,
    ));
  }
}

/** Extract platform-owned logical facet state without exposing it as tenant props. */
export function prepareTenantFacets(
  ctx: DurableObjectState,
  manager: FacetManagerCapability,
  authority: TenantDoAuthority,
  logicalPathValue: unknown,
  tenantProps: unknown,
): { facets: DurableObjectFacets; logicalPath: readonly string[]; tenantProps: unknown } {
  const logicalPath = Array.isArray(logicalPathValue) && logicalPathValue.length === 0
    ? Object.freeze([] as string[])
    : path(logicalPathValue);
  return {
    facets: new TenantFacets(manager, authority, logicalPath, ctx.id),
    logicalPath,
    tenantProps,
  };
}
