import { DurableObject } from "cloudflare:workers";
import type {
  DoHostEnv, DoOrder, FacetClassDescriptor, LoadedDurableObject, ResolvedDoAuthority,
  TenantDoAuthority,
} from "./protocol.js";
import {
  inboundSocketAddress,
  socketAddressFromWire,
  tunnelSockets,
  validateSocketAuthorityWire,
  type SocketAuthorityWire,
} from "../sockets/tunnel.js";
import {
  bindingError,
  doPolicy,
  lockWorkerCode,
  modulesFor,
  resolveSnapshot,
  tenantGlobalOutbound,
  tenantEnv,
} from "../loader/host.js";
import { collectableWorkerCode } from "../observability/collector.js";

const INTERNAL = [
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
  "constructor", "prototype", "__proto__", "then", "dup", "fetch", "connect", "alarm",
  "webSocketMessage", "webSocketClose", "webSocketError",
]);
const ORDER_CHANNEL = /^[0-9a-f]{32}$/;
const ORDER_IDLE_MS = 60_000;
const MAX_ORDER_CHANNELS = 65_536;
const MAX_PENDING_OPERATIONS = 256;
const FACET_NAME_BYTES = 256;
const FACET_TREE_DEPTH = 4;
const FACET_ENTRYPOINT = /^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/;
const FACET_TOKEN = /^[0-9a-f]{32}$/;
const encoder = new TextEncoder();
interface PendingOperation { resolve: () => void }
interface OrderState {
  next: number;
  expiresAt: number;
  pending: Map<number, PendingOperation>;
}
interface RegisteredFacet {
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
type PendingConnect = PendingTenantConnect | PendingFacetConnect;

function facetName(value: unknown): string {
  if (typeof value !== "string" || encoder.encode(value).byteLength > FACET_NAME_BYTES) {
    throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
  }
  return value;
}

function facetPath(value: unknown): readonly string[] {
  if (!Array.isArray(value) || value.length < 1 || value.length > FACET_TREE_DEPTH - 1
      || value.some(name => typeof name !== "string" || encoder.encode(name).byteLength > FACET_NAME_BYTES)) {
    throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
  }
  return Object.freeze([...value]);
}

function childFacetPath(parent: unknown, name: unknown): readonly string[] {
  const raw = Array.isArray(parent) ? parent : [];
  if (raw.length >= FACET_TREE_DEPTH - 1) throw bindingError("DO_RUNTIME_EXCEPTION");
  return facetPath([...raw, facetName(name)]);
}

function pathPrefix(path: readonly string[], prefix: readonly string[]): boolean {
  return prefix.length <= path.length && prefix.every((name, index) => path[index] === name);
}

function validateDescriptor(value: unknown): FacetClassDescriptor {
  if (value === null || typeof value !== "object") throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
  const entrypoint = Reflect.get(value, "entrypoint");
  const id = Reflect.get(value, "id");
  if (typeof entrypoint !== "string" || !FACET_ENTRYPOINT.test(entrypoint)
      || typeof id !== "string" || id.length > 2048) {
    throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
  }
  return Object.freeze({ entrypoint, id, props: Reflect.get(value, "props") });
}

async function physicalFacetName(logicalPath: readonly string[]): Promise<string> {
  const encoded = encoder.encode(JSON.stringify(logicalPath));
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", encoded));
  let name = "f-";
  for (const byte of digest) name += byte.toString(16).padStart(2, "0");
  return name;
}

function assertOrder(order: unknown): asserts order is DoOrder {
  if (!order || typeof order !== "object") throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
  const candidate = order as Partial<DoOrder>;
  if (typeof candidate.channelId !== "string" || !ORDER_CHANNEL.test(candidate.channelId)
      || !Number.isSafeInteger(candidate.sequence) || candidate.sequence! < 0) {
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

function ordered<T>(
  states: Map<string, OrderState>,
  order: DoOrder,
  run: () => Promise<T>,
): Promise<T> {
  assertOrder(order);
  const now = Date.now();
  let state = states.get(order.channelId);
  if (!state) {
    for (const [channelId, candidate] of states) {
      if (candidate.pending.size === 0 && candidate.expiresAt <= now) states.delete(channelId);
    }
    if (states.size >= MAX_ORDER_CHANNELS) throw bindingError("DO_STORAGE_LIMIT");
    state = { next: 0, expiresAt: now + ORDER_IDLE_MS, pending: new Map() };
    states.set(order.channelId, state);
  }
  if (order.sequence < state.next || state.pending.has(order.sequence)
      || state.pending.size >= MAX_PENDING_OPERATIONS) {
    throw bindingError("DO_RUNTIME_EXCEPTION");
  }
  state.expiresAt = now + ORDER_IDLE_MS;
  if (order.sequence === state.next) {
    state.next += 1;
    let value: Promise<T>;
    try { value = run(); }
    catch (error) {
      grantNextOperation(state);
      throw error;
    }
    grantNextOperation(state);
    return value;
  }
  const turn = new Promise<void>(resolve => {
    state!.pending.set(order.sequence, { resolve });
  });
  return turn.then(() => {
    let value: Promise<T>;
    try { value = run(); }
    catch (error) {
      grantNextOperation(state);
      throw error;
    }
    grantNextOperation(state);
    return value;
  });
}

function assertRpcMember(member: unknown): asserts member is string {
  if (typeof member !== "string" || FORBIDDEN_RPC.has(member)
      || member.startsWith("__openCompute")) {
    throw bindingError("DO_RPC_UNSUPPORTED");
  }
}

function required(headers: Headers, name: string, pattern: RegExp): string {
  const value = headers.get(name) || "";
  if (!pattern.test(value)) throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
  return value;
}

function authorityFromHeaders(headers: Headers) {
  const accountId = required(headers, "x-open-compute-account-id", /^[0-9a-f-]{36}$/);
  const workerId = required(headers, "x-open-compute-worker-id", /^[0-9a-f-]{36}$/);
  const versionId = required(headers, "x-open-compute-version-id", /^[0-9a-f-]{36}$/);
  const workerCodeSha256 = required(
    headers,
    "x-open-compute-worker-code-sha256",
    /^[0-9a-f]{64}$/,
  );
  const objectId = required(headers, "x-open-compute-object-id", /^[0-9a-f]{64}$/);
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
  const routeGeneration = Number(headers.get("x-open-compute-route-generation"));
  const objectGeneration = Number(headers.get("x-open-compute-object-generation"));
  if (!Number.isSafeInteger(routeGeneration) || routeGeneration < 1
      || !Number.isSafeInteger(objectGeneration) || objectGeneration < 1) {
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

function deleteAuthorityFromHeaders(headers: Headers) {
  const objectId = required(headers, "x-open-compute-object-id", /^[0-9a-f]{64}$/);
  const objectGeneration = Number(headers.get("x-open-compute-object-generation"));
  if (!Number.isSafeInteger(objectGeneration) || objectGeneration < 1) {
    throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
  }
  return { objectId, objectGeneration };
}

export class DoHost extends DurableObject<DoHostEnv> {
  readonly #activationId = crypto.randomUUID().replaceAll("-", "");
  readonly #facetVersions = new Map<string, number>();
  readonly #orderStates = new Map<string, OrderState>();
  readonly #pendingConnects = new Map<string, PendingConnect>();
  constructor(ctx: DurableObjectState, env: DoHostEnv) {
    super(ctx, env);
    this.ctx.storage.sql.exec(`
      CREATE TABLE IF NOT EXISTS open_compute_host_meta (
        singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
        route_generation INTEGER NOT NULL,
        version_id TEXT NOT NULL,
        object_generation INTEGER NOT NULL,
        data_format_version INTEGER NOT NULL
      )
    `);
    this.ctx.storage.sql.exec(`
      CREATE TABLE IF NOT EXISTS open_compute_host_facets (
        physical_name TEXT PRIMARY KEY,
        logical_path_json TEXT NOT NULL UNIQUE
      ) STRICT
    `);
  }

  #meta() {
    const rows = this.ctx.storage.sql.exec(
      "SELECT route_generation, version_id, object_generation, data_format_version "
      + "FROM open_compute_host_meta WHERE singleton = 1",
    ).toArray();
    return rows.length ? rows[0] : null;
  }

  async #registeredFacets(prefix?: readonly string[]): Promise<RegisteredFacet[]> {
    const rows = this.ctx.storage.sql.exec(
      "SELECT physical_name, logical_path_json FROM open_compute_host_facets ORDER BY logical_path_json",
    ).toArray();
    const facets: RegisteredFacet[] = [];
    for (const row of rows) {
      if (typeof row.physical_name !== "string" || typeof row.logical_path_json !== "string") {
        throw bindingError("DO_STORAGE_UNAVAILABLE");
      }
      let parsed: unknown;
      try { parsed = JSON.parse(row.logical_path_json); }
      catch { throw bindingError("DO_STORAGE_UNAVAILABLE"); }
      const logicalPath = facetPath(parsed);
      const physicalName = await physicalFacetName(logicalPath);
      if (physicalName !== row.physical_name) throw bindingError("DO_STORAGE_UNAVAILABLE");
      if (prefix === undefined || pathPrefix(logicalPath, prefix)) {
        facets.push({ logicalPath, physicalName });
      }
    }
    return facets.sort((left, right) => left.logicalPath.length - right.logicalPath.length
      || JSON.stringify(left.logicalPath).localeCompare(JSON.stringify(right.logicalPath)));
  }

  #registerFacet(logicalPath: readonly string[], physicalName: string): void {
    const encoded = JSON.stringify(logicalPath);
    this.ctx.storage.sql.exec(
      "INSERT INTO open_compute_host_facets (physical_name, logical_path_json) VALUES (?, ?) "
      + "ON CONFLICT DO NOTHING",
      physicalName,
      encoded,
    );
    const row = this.ctx.storage.sql.exec(
      "SELECT logical_path_json FROM open_compute_host_facets WHERE physical_name = ?",
      physicalName,
    ).one();
    if (row.logical_path_json !== encoded) throw bindingError("DO_STORAGE_UNAVAILABLE");
  }

  #unregisterFacets(facets: readonly RegisteredFacet[]): void {
    for (const facet of facets) {
      this.ctx.storage.sql.exec(
        "DELETE FROM open_compute_host_facets WHERE physical_name = ?",
        facet.physicalName,
      );
    }
  }

  #bumpFacetVersions(facets: readonly RegisteredFacet[]): void {
    for (const facet of facets) {
      this.#facetVersions.set(
        facet.physicalName,
        (this.#facetVersions.get(facet.physicalName) ?? 0) + 1,
      );
    }
  }

  async #abortRegisteredFacets(prefix?: readonly string[], reason: unknown = "facet-aborted"): Promise<void> {
    const facets = await this.#registeredFacets(prefix);
    for (const facet of facets.toReversed()) this.ctx.facets.abort(facet.physicalName, reason);
    this.#bumpFacetVersions(facets);
  }

  async #loadedClass(
    authority: TenantDoAuthority,
    entrypoint: string,
    logicalPath: readonly string[],
    tenantProps: unknown,
  ) {
    if (!FACET_ENTRYPOINT.test(entrypoint)) throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
    const physicalName = logicalPath.length === 0 ? "root" : await physicalFacetName(logicalPath);
    const version = this.#facetVersions.get(physicalName) ?? 0;
    const envelope = {
      loaderKey: `${authority.accountId}/${authority.workerId}/${authority.versionId}`,
      expected: authority.workerCodeSha256,
      runtimeKey: `runtime/${authority.accountId}/${authority.workerId}/${authority.versionId}`
        + `/${authority.workerCodeSha256}/g/${authority.routeGeneration}/do/${this.#activationId}`
        + `/${physicalName}/v/${version}/${entrypoint}`,
    };
    const snapshot = await resolveSnapshot(
      this.env,
      envelope,
      false,
      false,
      this.env.INTERNAL_TOKEN,
    );
    if (snapshot.routeGeneration !== authority.routeGeneration) {
      throw bindingError("DO_VERSION_STALE");
    }
    const observabilityGeneration = snapshot.observability?.observabilityGeneration ?? 0;
    envelope.runtimeKey += `/o/${observabilityGeneration}`;
    const built = modulesFor(snapshot, false, entrypoint, true);
    const code = {
      ...lockWorkerCode(this.env),
      mainModule: built.mainModule,
      modules: built.modules,
      env: tenantEnv(snapshot, this.ctx, authority.versionId, doPolicy(this.env), true, false),
      globalOutbound: tenantGlobalOutbound(this.env, false),
    };
    Object.defineProperties(code.env, {
      __OPEN_COMPUTE_PRIVATE_ALARM_INDEX: {
        value: this.ctx.exports.AlarmIndex({ props: {
          namespaceResourceId: authority.namespaceResourceId,
          objectId: authority.objectId,
          objectGeneration: authority.objectGeneration,
        } }),
        enumerable: true,
      },
      __OPEN_COMPUTE_PRIVATE_FACET_MANAGER: {
        value: this.env.DO_HOST.get(this.ctx.id),
        enumerable: true,
      },
      __OPEN_COMPUTE_PRIVATE_FACET_AUTHORITY: {
        value: Object.freeze({ ...authority }),
        enumerable: true,
      },
      __OPEN_COMPUTE_PRIVATE_FACET_PATH: {
        value: Object.freeze([...logicalPath]),
        enumerable: true,
      },
      __OPEN_COMPUTE_PRIVATE_FACET_PROPS: {
        value: tenantProps,
        enumerable: true,
      },
    });
    const loaded = this.env.LOADER.get(envelope.runtimeKey, () =>
      collectableWorkerCode(code, this.ctx, snapshot.observability));
    return loaded.getDurableObjectClass<LoadedDurableObject>(entrypoint);
  }

  async #tenantFacet(
    authority: TenantDoAuthority,
    logicalPathValue: unknown,
    descriptorValue: unknown,
  ) {
    await this.#tenant(authority);
    const logicalPath = facetPath(logicalPathValue);
    const descriptor = validateDescriptor(descriptorValue);
    const physicalName = await physicalFacetName(logicalPath);
    const cls = await this.#loadedClass(
      authority,
      descriptor.entrypoint,
      logicalPath,
      descriptor.props,
    );
    this.#registerFacet(logicalPath, physicalName);
    return this.ctx.facets.get(physicalName, () => ({
      class: cls,
      id: descriptor.id,
    }));
  }

  #purgeExpiredConnects(now = Date.now()): void {
    for (const [token, pending] of this.#pendingConnects) {
      if (pending.expiresAt > now) continue;
      this.#pendingConnects.delete(token);
      if (pending.kind !== "tenant") continue;
      try {
        this.ctx.waitUntil(ordered(
          this.#orderStates, pending.order, async () => undefined,
        ).catch(() => undefined));
      } catch {
        // A duplicate or already-started operation needs no expiry repair.
      }
    }
  }

  async #tenant(authority: TenantDoAuthority) {
    const prior = this.#meta();
    if (prior && authority.routeGeneration < Number(prior.route_generation)) {
      throw bindingError("DO_VERSION_STALE");
    }
    if (prior && authority.objectGeneration !== Number(prior.object_generation)) {
      throw bindingError("DO_OBJECT_DELETING");
    }
    if (prior && authority.routeGeneration === Number(prior.route_generation)
        && authority.versionId !== prior.version_id) {
      throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
    }
    if (prior && authority.routeGeneration > Number(prior.route_generation)) {
      await this.ctx.facets.abort("tenant", "version-generation-advanced");
      await this.#abortRegisteredFacets(undefined, "version-generation-advanced");
    }
    const cls = await this.#loadedClass(authority, authority.className, [], {});
    const facet = this.ctx.facets.get("tenant", () => ({
      class: cls,
      id: authority.objectId,
    }));
    if (!prior || authority.routeGeneration > Number(prior.route_generation)) {
      this.ctx.storage.sql.exec(
        "INSERT OR REPLACE INTO open_compute_host_meta "
        + "(singleton, route_generation, version_id, object_generation, data_format_version) "
        + "VALUES (1, ?, ?, ?, 1)",
        authority.routeGeneration,
        authority.versionId,
        authority.objectGeneration,
      );
    }
    return facet;
  }

  async fetch(request: Request): Promise<Response> {
    const operation = request.headers.get("x-open-compute-do-operation") || "fetch";
    if (operation === "delete") {
      await this.#deleteTenant(deleteAuthorityFromHeaders(request.headers));
      return new Response(null, { status: 204 });
    }
    const authority = authorityFromHeaders(request.headers);
    if (operation === "alarm" || operation === "alarm-repair") {
      const payload: unknown = await request.json();
      const facet = await this.#tenant(authority);
      const result = operation === "alarm"
        ? await facet.__openComputeAlarm(payload)
        : await facet.__openComputeAlarmRepair();
      return Response.json(result);
    }
    this.#purgeExpiredConnects();
    const order = {
      channelId: required(
        request.headers,
        "x-open-compute-do-order-channel",
        ORDER_CHANNEL,
      ),
      sequence: Number(request.headers.get("x-open-compute-do-order-sequence")),
    };
    assertOrder(order);
    const tenantMethod = required(request.headers, "x-open-compute-do-method", /^[A-Z]{1,16}$/);
    const tenantUrl = request.headers.get("x-open-compute-do-url") || "https://do.invalid/";
    const headers = new Headers(request.headers);
    for (const name of INTERNAL) headers.delete(name);
    const init: RequestInit = { method: tenantMethod, headers, body: request.body, redirect: "manual" };
    if (tenantMethod === "GET" || tenantMethod === "HEAD") delete init.body;
    const facet = await this.#tenant(authority);
    const tenantRequest = new Request(tenantUrl, init);
    return ordered(this.#orderStates, order, () => facet.fetch(tenantRequest));
  }

  async dispatchTenantRpc(
    authority: TenantDoAuthority,
    order: DoOrder,
    method: unknown,
    args: unknown[],
  ): Promise<unknown> {
    if (!authority || typeof authority !== "object" || !Array.isArray(args)) {
      throw bindingError("DO_RPC_UNSUPPORTED");
    }
    assertOrder(order);
    assertRpcMember(method);
    this.#purgeExpiredConnects();
    const facet = await this.#tenant(authority);
    const target: unknown = Reflect.get(facet, method);
    if (typeof target !== "function") throw bindingError("DO_RPC_UNSUPPORTED");
    return ordered(this.#orderStates, order, async () => {
      try { return await Reflect.apply(target, facet, args); }
      catch { throw bindingError("DO_RUNTIME_EXCEPTION"); }
    });
  }

  async getTenantRpcProperty(
    authority: TenantDoAuthority,
    order: DoOrder,
    property: unknown,
  ): Promise<unknown> {
    if (!authority || typeof authority !== "object") throw bindingError("DO_RPC_UNSUPPORTED");
    assertOrder(order);
    assertRpcMember(property);
    this.#purgeExpiredConnects();
    const facet = await this.#tenant(authority);
    return ordered(this.#orderStates, order, async () => {
      try { return await Reflect.get(facet, property); }
      catch { throw bindingError("DO_RUNTIME_EXCEPTION"); }
    });
  }

  async __openComputeFacetCall(
    authority: TenantDoAuthority,
    logicalPath: readonly string[],
    descriptor: FacetClassDescriptor,
    method: unknown,
    args: unknown[],
  ): Promise<unknown> {
    if (!Array.isArray(args)) throw bindingError("DO_RPC_UNSUPPORTED");
    assertRpcMember(method);
    const facet = await this.#tenantFacet(authority, logicalPath, descriptor);
    const target: unknown = Reflect.get(facet, method);
    if (typeof target !== "function") throw bindingError("DO_RPC_UNSUPPORTED");
    try { return await Reflect.apply(target, facet, args); }
    catch { throw bindingError("DO_RUNTIME_EXCEPTION"); }
  }

  async __openComputeFacetGet(
    authority: TenantDoAuthority,
    logicalPath: readonly string[],
    descriptor: FacetClassDescriptor,
    property: unknown,
  ): Promise<unknown> {
    assertRpcMember(property);
    const facet = await this.#tenantFacet(authority, logicalPath, descriptor);
    try { return await Reflect.get(facet, property); }
    catch { throw bindingError("DO_RUNTIME_EXCEPTION"); }
  }

  async __openComputeFacetFetch(
    authority: TenantDoAuthority,
    logicalPath: readonly string[],
    descriptor: FacetClassDescriptor,
    request: Request,
  ): Promise<Response> {
    if (!(request instanceof Request)) throw bindingError("DO_RPC_UNSUPPORTED");
    const facet = await this.#tenantFacet(authority, logicalPath, descriptor);
    try { return await facet.fetch(request); }
    catch { throw bindingError("DO_RUNTIME_EXCEPTION"); }
  }

  async __openComputeFacetAbort(
    authority: TenantDoAuthority,
    parentPath: readonly string[],
    name: string,
    reason: unknown,
  ): Promise<void> {
    await this.#tenant(authority);
    await this.#abortRegisteredFacets(childFacetPath(parentPath, name), reason);
  }

  async __openComputeFacetDelete(
    authority: TenantDoAuthority,
    parentPath: readonly string[],
    name: string,
  ): Promise<void> {
    await this.#tenant(authority);
    const facets = await this.#registeredFacets(childFacetPath(parentPath, name));
    for (const facet of facets.toReversed()) await this.ctx.facets.delete(facet.physicalName);
    this.#bumpFacetVersions(facets);
    this.#unregisterFacets(facets);
  }

  async __openComputeFacetClone(
    authority: TenantDoAuthority,
    parentPath: readonly string[],
    sourceName: string,
    destinationName: string,
  ): Promise<void> {
    await this.#tenant(authority);
    const source = childFacetPath(parentPath, sourceName);
    const destination = childFacetPath(parentPath, destinationName);
    if (sourceName === destinationName) {
      await this.#abortRegisteredFacets(source, "facet-cloned-over");
      return;
    }
    const destinationFacets = await this.#registeredFacets(destination);
    for (const facet of destinationFacets.toReversed()) await this.ctx.facets.delete(facet.physicalName);
    this.#bumpFacetVersions(destinationFacets);
    this.#unregisterFacets(destinationFacets);
    const sourceFacets = await this.#registeredFacets(source);
    for (const sourceFacet of sourceFacets) {
      const suffix = sourceFacet.logicalPath.slice(source.length);
      const destinationPath = facetPath([...destination, ...suffix]);
      const destinationPhysical = await physicalFacetName(destinationPath);
      this.ctx.facets.clone(sourceFacet.physicalName, destinationPhysical);
      this.#registerFacet(destinationPath, destinationPhysical);
    }
  }

  async __openComputePrepareFacetConnect(
    authority: TenantDoAuthority,
    logicalPathValue: readonly string[],
    descriptorValue: FacetClassDescriptor,
    token: string,
    authorityWire: SocketAuthorityWire,
  ): Promise<void> {
    if (!FACET_TOKEN.test(token)) throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
    const logicalPath = facetPath(logicalPathValue);
    const descriptor = validateDescriptor(descriptorValue);
    const connectAuthority = validateSocketAuthorityWire(authorityWire);
    await this.#tenantFacet(authority, logicalPath, descriptor);
    this.#purgeExpiredConnects();
    if (this.#pendingConnects.size >= 128 || this.#pendingConnects.has(token)) {
      throw bindingError("DO_STORAGE_LIMIT");
    }
    this.#pendingConnects.set(token, {
      kind: "facet",
      authority,
      logicalPath,
      descriptor,
      connectAuthority,
      expiresAt: Date.now() + 10_000,
    });
  }

  async __openComputePrepareConnect(
    authority: ResolvedDoAuthority,
    order: DoOrder,
    authorityWire: SocketAuthorityWire,
  ): Promise<string> {
    if (!authority || typeof authority !== "object") {
      throw bindingError("DO_RUNTIME_EXCEPTION");
    }
    assertOrder(order);
    const connectAuthority = validateSocketAuthorityWire(authorityWire);
    await this.#tenant(authority);
    const now = Date.now();
    this.#purgeExpiredConnects(now);
    if (this.#pendingConnects.size >= 128) throw bindingError("DO_STORAGE_LIMIT");
    const token = crypto.randomUUID().replaceAll("-", "");
    this.#pendingConnects.set(token, {
      kind: "tenant",
      connectAuthority,
      authority,
      expiresAt: now + 10_000,
      order: { channelId: order.channelId, sequence: order.sequence },
    });
    return token;
  }

  async __openComputeCancelOrder(order: DoOrder): Promise<void> {
    assertOrder(order);
    for (const [token, pending] of this.#pendingConnects) {
      if (pending.kind === "tenant" && pending.order.channelId === order.channelId
          && pending.order.sequence === order.sequence) {
        this.#pendingConnects.delete(token);
      }
    }
    const state = this.#orderStates.get(order.channelId);
    if (state && (order.sequence < state.next || state.pending.has(order.sequence))) return;
    await ordered(this.#orderStates, order, async () => undefined);
  }

  async connect(socket: Socket): Promise<void> {
    try {
      this.#purgeExpiredConnects();
      const tokenAddress = await inboundSocketAddress(socket);
      const match = /^([0-9a-f]{32})\.(do|facet)-connect\.invalid:1$/.exec(tokenAddress);
      const pending = match ? this.#pendingConnects.get(match[1]!) : undefined;
      if (!match || !pending || pending.expiresAt <= Date.now()
          || (match[2] === "do") !== (pending.kind === "tenant")) {
        if (match) this.#pendingConnects.delete(match[1]!);
        throw bindingError("DO_RUNTIME_EXCEPTION");
      }
      this.#pendingConnects.delete(match[1]!);
      if (pending.kind === "facet") {
        const target = await this.#tenantFacet(
          pending.authority,
          pending.logicalPath,
          pending.descriptor,
        );
        const connected = target.connect(socketAddressFromWire(pending.connectAuthority), {
          allowHalfOpen: true,
        });
        await connected.opened;
        await tunnelSockets(socket, connected);
        return;
      }
      const tenant = await this.#tenant(pending.authority);
      await ordered(this.#orderStates, pending.order, async () => {
        const target = tenant.connect(socketAddressFromWire(pending.connectAuthority), {
          allowHalfOpen: true,
        });
        await target.opened;
        await tunnelSockets(socket, target);
      });
    } catch {
      await socket.close().catch(() => undefined);
      throw bindingError("DO_RUNTIME_EXCEPTION");
    }
  }

  async #deleteTenant(authority: ReturnType<typeof deleteAuthorityFromHeaders>) {
    const meta = this.#meta();
    if (meta && authority.objectGeneration !== Number(meta.object_generation)) {
      throw bindingError("DO_OBJECT_DELETING");
    }
    const facets = await this.#registeredFacets();
    for (const facet of facets.toReversed()) await this.ctx.facets.delete(facet.physicalName);
    this.#bumpFacetVersions(facets);
    this.#unregisterFacets(facets);
    await this.ctx.facets.delete("tenant");
    return true;
  }
}
