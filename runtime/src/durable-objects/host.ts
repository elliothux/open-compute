import { DurableObject } from "cloudflare:workers";
import type { DoHostEnv, DoPlainValue, DoWireValue, LoadedDurableObject } from "./protocol.js";
import {
  PROFILE,
  bindingError,
  doPolicy,
  modulesFor,
  resolveSnapshot,
  tenantEnv,
} from "../loader/host.js";

const INTERNAL = [
  "x-open-compute-binding-token",
  "x-open-compute-account-id",
  "x-open-compute-worker-id",
  "x-open-compute-binding-id",
  "x-open-compute-deployment-id",
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
  "x-open-compute-request-id",
  "x-open-compute-startup-generation",
];
const PUBLIC_METHOD = /^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/;
const FORBIDDEN_RPC = new Set(["constructor", "prototype", "__proto__", "then", "fetch"]);
const DELETE_ALL_DELETES_ALARM = "delete_all_deletes_alarm";
const DELETE_ALL_PRESERVES_ALARM = "delete_all_preserves_alarm";

function tenantCompatibilityFlags(flags: readonly string[]): string[] {
  const result = flags.filter(flag => flag !== DELETE_ALL_DELETES_ALARM);
  if (!result.includes(DELETE_ALL_PRESERVES_ALARM)) result.push(DELETE_ALL_PRESERVES_ALARM);
  return result;
}

function decodeWire(value: unknown): DoPlainValue {
  if (!Array.isArray(value) || typeof value[0] !== "string") {
    throw bindingError("DO_RPC_UNSUPPORTED");
  }
  switch (value[0]) {
    case "z": return null;
    case "s": if (typeof value[1] === "string") return value[1]; break;
    case "b": if (typeof value[1] === "boolean") return value[1]; break;
    case "n": if (typeof value[1] === "number" && Number.isFinite(value[1])) return value[1]; break;
    case "x": {
      if (typeof value[1] !== "string") break;
      const binary = atob(value[1]);
      const bytes = new Uint8Array(binary.length);
      for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
      return bytes.buffer;
    }
    case "a": if (Array.isArray(value[1])) return value[1].map(decodeWire); break;
    case "o": {
      if (!Array.isArray(value[1])) break;
      const result: Record<string, DoPlainValue> = Object.create(null);
      for (const entry of value[1]) {
        if (!Array.isArray(entry) || entry.length !== 2 || typeof entry[0] !== "string") {
          throw bindingError("DO_RPC_UNSUPPORTED");
        }
        Object.defineProperty(result, entry[0], {
          value: decodeWire(entry[1]), enumerable: true, writable: true, configurable: true,
        });
      }
      return result;
    }
  }
  throw bindingError("DO_RPC_UNSUPPORTED");
}

function encodeWire(value: unknown, seen = new WeakSet<object>()): DoWireValue {
  if (value === null) return ["z"];
  if (typeof value === "string") return ["s", value];
  if (typeof value === "boolean") return ["b", value];
  if (typeof value === "number" && Number.isFinite(value)) return ["n", value];
  if (value instanceof ArrayBuffer || ArrayBuffer.isView(value)) {
    const bytes = value instanceof ArrayBuffer
      ? new Uint8Array(value)
      : new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    let binary = "";
    for (const byte of bytes) binary += String.fromCharCode(byte);
    return ["x", btoa(binary)];
  }
  if (!value || typeof value !== "object" || value instanceof Promise
      || value instanceof ReadableStream || seen.has(value)) {
    throw bindingError("DO_RPC_UNSUPPORTED");
  }
  const prototype = Object.getPrototypeOf(value);
  if (!Array.isArray(value) && prototype !== Object.prototype && prototype !== null) {
    throw bindingError("DO_RPC_UNSUPPORTED");
  }
  seen.add(value);
  const encoded: DoWireValue = Array.isArray(value)
    ? ["a", value.map(item => encodeWire(item, seen))]
    : ["o", Object.entries(value).map(([key, item]) => [key, encodeWire(item, seen)])];
  seen.delete(value);
  return encoded;
}

function required(headers: Headers, name: string, pattern: RegExp): string {
  const value = headers.get(name) || "";
  if (!pattern.test(value)) throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
  return value;
}

function authorityFromHeaders(headers: Headers) {
  const accountId = required(headers, "x-open-compute-account-id", /^[0-9a-f-]{36}$/);
  const workerId = required(headers, "x-open-compute-worker-id", /^[0-9a-f-]{36}$/);
  const deploymentId = required(headers, "x-open-compute-deployment-id", /^[0-9a-f-]{36}$/);
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
    deploymentId,
    workerCodeSha256,
    objectId,
    namespaceResourceId,
    className,
    routeGeneration,
    objectGeneration,
    loaderKey: `${accountId}/${workerId}/${deploymentId}`,
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
  constructor(ctx: DurableObjectState, env: DoHostEnv) {
    super(ctx, env);
    this.ctx.storage.sql.exec(`
      CREATE TABLE IF NOT EXISTS open_compute_host_meta (
        singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
        route_generation INTEGER NOT NULL,
        deployment_id TEXT NOT NULL,
        object_generation INTEGER NOT NULL,
        data_format_version INTEGER NOT NULL
      )
    `);
  }

  #meta() {
    const rows = this.ctx.storage.sql.exec(
      "SELECT route_generation, deployment_id, object_generation, data_format_version "
      + "FROM open_compute_host_meta WHERE singleton = 1",
    ).toArray();
    return rows.length ? rows[0] : null;
  }

  async #tenant(authority: ReturnType<typeof authorityFromHeaders>) {
    const prior = this.#meta();
    if (prior && authority.routeGeneration < Number(prior.route_generation)) {
      throw bindingError("DO_DEPLOYMENT_STALE");
    }
    if (prior && authority.objectGeneration !== Number(prior.object_generation)) {
      throw bindingError("DO_OBJECT_DELETING");
    }
    if (prior && authority.routeGeneration === Number(prior.route_generation)
        && authority.deploymentId !== prior.deployment_id) {
      throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
    }
    if (prior && authority.routeGeneration > Number(prior.route_generation)) {
      await this.ctx.facets.abort("tenant", "deployment-generation-advanced");
    }
    const envelope = {
      loaderKey: authority.loaderKey,
      expected: authority.workerCodeSha256,
      runtimeKey: `runtime/${authority.loaderKey}/${authority.workerCodeSha256}/g/${authority.routeGeneration}/${authority.className}`,
    };
    const snapshot = await resolveSnapshot(
      this.env,
      envelope,
      false,
      false,
      this.env.INTERNAL_TOKEN,
    );
    if (snapshot.routeGeneration !== authority.routeGeneration) {
      throw bindingError("DO_DEPLOYMENT_STALE");
    }
    const built = modulesFor(snapshot, false, authority.className, true);
    const code = {
      compatibilityDate: snapshot.compatibilityDate,
      // Native facet storage has no alarm scheduler until P0.8. Preserve the alarm
      // metadata so deleteAll() remains a local KV/SQL operation for P0.7 tenants.
      compatibilityFlags: tenantCompatibilityFlags(snapshot.compatibilityFlags),
      mainModule: built.mainModule,
      modules: built.modules,
      env: tenantEnv(snapshot, this.ctx, authority.deploymentId, doPolicy(this.env), true),
      globalOutbound: this.ctx.exports.OutboundGateway({
        props: { deploymentId: authority.deploymentId, policyVersion: 1 },
      }),
      limits: PROFILE,
    };
    Object.defineProperty(code.env, "__OPEN_COMPUTE_PRIVATE_ALARM_INDEX", {
      value: this.ctx.exports.AlarmIndex({ props: {
        namespaceResourceId: authority.namespaceResourceId,
        objectId: authority.objectId,
        objectGeneration: authority.objectGeneration,
      } }),
      enumerable: true,
    });
    const loaded = this.env.LOADER.get(envelope.runtimeKey, () => code);
    const cls = loaded.getDurableObjectClass<LoadedDurableObject>(authority.className);
    const facet = this.ctx.facets.get("tenant", () => ({ class: cls, id: authority.objectId }));
    if (!prior || authority.routeGeneration > Number(prior.route_generation)) {
      this.ctx.storage.sql.exec(
        "INSERT OR REPLACE INTO open_compute_host_meta "
        + "(singleton, route_generation, deployment_id, object_generation, data_format_version) "
        + "VALUES (1, ?, ?, ?, 1)",
        authority.routeGeneration,
        authority.deploymentId,
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
    if (operation === "rpc") {
      const payload: unknown = await request.json();
      if (payload === null || typeof payload !== "object" || !("args" in payload) || !("method" in payload)) {
        throw bindingError("DO_RPC_UNSUPPORTED");
      }
      const args = decodeWire(payload.args);
      if (!Array.isArray(args)) throw bindingError("DO_RPC_UNSUPPORTED");
      const value = await this.#dispatchRpc(authority, payload.method, args);
      return Response.json({ value: encodeWire(value) });
    }
    if (operation === "alarm" || operation === "alarm-repair") {
      const payload: unknown = await request.json();
      const facet = await this.#tenant(authority);
      const result = operation === "alarm"
        ? await facet.__openComputeAlarm(payload)
        : await facet.__openComputeAlarmRepair();
      return Response.json(result);
    }
    const tenantMethod = required(request.headers, "x-open-compute-do-method", /^[A-Z]{1,16}$/);
    const tenantUrl = request.headers.get("x-open-compute-do-url") || "https://do.invalid/";
    const headers = new Headers(request.headers);
    for (const name of INTERNAL) headers.delete(name);
    const init: RequestInit = { method: tenantMethod, headers, body: request.body, redirect: "manual" };
    if (tenantMethod === "GET" || tenantMethod === "HEAD") delete init.body;
    const facet = await this.#tenant(authority);
    return facet.fetch(new Request(tenantUrl, init));
  }

  async #dispatchRpc(authority: ReturnType<typeof authorityFromHeaders>, method: unknown, args: DoPlainValue[]): Promise<unknown> {
    if (!authority || typeof authority !== "object" || typeof method !== "string"
        || FORBIDDEN_RPC.has(method) || method.startsWith("__openCompute")
        || !PUBLIC_METHOD.test(method) || !Array.isArray(args)) {
      throw bindingError("DO_RPC_UNSUPPORTED");
    }
    const facet = await this.#tenant(authority);
    const target: unknown = Reflect.get(facet, method);
    if (typeof target !== "function") throw bindingError("DO_RPC_UNSUPPORTED");
    try {
      return await Reflect.apply(target, facet, args);
    } catch {
      throw bindingError("DO_RUNTIME_EXCEPTION");
    }
  }

  async #deleteTenant(authority: ReturnType<typeof deleteAuthorityFromHeaders>) {
    const meta = this.#meta();
    if (meta && authority.objectGeneration !== Number(meta.object_generation)) {
      throw bindingError("DO_OBJECT_DELETING");
    }
    await this.ctx.facets.delete("tenant");
    return true;
  }
}
