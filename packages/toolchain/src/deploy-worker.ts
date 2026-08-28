import { randomUUID } from "node:crypto";
import type { WorkerArtifact } from "./bundle-worker.ts";
import { record } from "./project.ts";
import type { WorkerProject } from "./project.ts";

/** Explicit destination and process-local credentials for one deployment. */
export interface DeployOptions {
  readonly endpoint?: string;
  readonly accountId?: string;
  readonly token?: string;
  readonly localOnly: boolean;
  readonly env?: Readonly<Record<string, string | undefined>>;
}

/** Public identifiers returned only after successful validation and promotion. */
export interface WorkerDeployment {
  readonly workerId: string;
  readonly deploymentId: string;
  readonly url: string;
  readonly sha256: string;
}

function identifier(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(value);
}

async function readJson(response: Response): Promise<unknown> {
  if (!response.body) throw new Error("platform response has no JSON body");
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  try {
    for (;;) {
      const part = await reader.read();
      if (part.done) break;
      length += part.value.byteLength;
      if (length > 1024 * 1024) throw new Error("platform response exceeds 1 MiB");
      chunks.push(part.value);
    }
    return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(Buffer.concat(chunks)));
  } catch { throw new Error("platform response JSON is invalid or exceeds 1 MiB"); }
  finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
}

/** Compile output enters the ordinary immutable deployment and promotion API. */
export async function deployWorker(project: WorkerProject, artifact: WorkerArtifact, options: DeployOptions): Promise<WorkerDeployment> {
  const endpoint = new URL(options.endpoint ?? project.endpoint);
  const loopback = ["127.0.0.1", "[::1]", "localhost"].includes(endpoint.hostname);
  if (endpoint.username || endpoint.password || endpoint.search || endpoint.hash || endpoint.pathname !== "/"
      || (endpoint.protocol !== "https:" && !(endpoint.protocol === "http:" && loopback))) {
    throw new Error("platform endpoint must be an HTTPS origin or loopback HTTP origin without credentials");
  }
  if (options.localOnly && !loopback) throw new Error("run requires a local platform; use deploy for a remote destination");
  if (options.token !== undefined && !/^[\x21-\x7e]+$/.test(options.token)) throw new Error("invalid platform authentication token");

  const secrets: Record<string, string> = {};
  for (const [name, reference] of Object.entries(project.secrets)) {
    const value = (options.env ?? process.env)[reference.env];
    if (value === undefined || value.length === 0) throw new Error(`missing secret environment reference: ${reference.env}`);
    Object.defineProperty(secrets, name, { value, enumerable: true });
  }
  // HTTP field values are ASCII. Escaping UTF-16 code units preserves JSON values,
  // including astral characters, without putting credentials in URLs or artifacts.
  const metadata = JSON.stringify({
    mainModule: artifact.mainModule, compatibilityDate: project.compatibilityDate,
    compatibilityFlags: project.compatibilityFlags, vars: project.vars, secrets,
    bindings: project.bindings, promote: true,
  }).replace(/[^\x20-\x7e]/g, value => `\\u${value.charCodeAt(0).toString(16).padStart(4, "0")}`);
  if (metadata.length > 1024 * 1024) throw new Error("deployment metadata exceeds 1 MiB");

  const request = async (path: string, method = "GET", body?: RequestInit["body"], extraHeaders?: Record<string, string>): Promise<unknown> => {
    const headers = new Headers(extraHeaders);
    if (options.token !== undefined) headers.set("authorization", `Bearer ${options.token}`);
    if (method === "POST") headers.set("idempotency-key", randomUUID());
    let response: Response;
    try {
      response = await fetch(new URL(path, endpoint), {
        method, headers, ...(body === undefined ? {} : { body }),
        redirect: "error", signal: AbortSignal.timeout(120_000),
      });
    } catch { throw new Error("platform request failed; inspect platform state before retrying a mutation"); }
    if (!response.ok) {
      await response.body?.cancel().catch(() => {});
      throw new Error(`platform request failed (HTTP ${response.status})`);
    }
    return readJson(response);
  };

  let account = options.accountId ?? project.accountId;
  if (account === undefined) {
    const identity = await request("/v1/account");
    if (!record(identity) || !identifier(identity.accountId)) throw new Error("invalid platform account response");
    account = identity.accountId;
  }
  if (!identifier(account)) throw new Error("invalid account ID");
  const collection = `/v1/accounts/${account}/workers`;
  const listed = await request(collection);
  if (!record(listed) || !Array.isArray(listed.workers)) throw new Error("invalid Worker list response");
  const workers: readonly unknown[] = listed.workers;
  let workerId: string | undefined;
  for (const item of workers) {
    if (!record(item) || !identifier(item.id) || item.accountId !== account || typeof item.name !== "string") throw new Error("invalid Worker list entry");
    if (item.name === project.name && item.deletedAtMs === null) {
      if (workerId !== undefined) throw new Error("ambiguous Worker name");
      workerId = item.id;
    }
  }
  if (workerId === undefined) {
    const created = await request(collection, "POST", JSON.stringify({ name: project.name }), { "content-type": "application/json" });
    if (!record(created) || !record(created.worker) || !identifier(created.worker.id)
        || created.worker.accountId !== account || created.worker.name !== project.name) throw new Error("invalid Worker creation response");
    workerId = created.worker.id;
  }
  const result = await request(`${collection}/${workerId}/deployments`, "POST", Buffer.from(artifact.bytes), {
    "content-type": "application/octet-stream", "x-open-compute-deployment-metadata": metadata,
  });
  if (!record(result) || result.promoted !== true || !record(result.deployment)
      || !identifier(result.deployment.id) || result.deployment.state !== "ready"
      || result.deployment.workerId !== workerId) {
    throw new Error("platform did not confirm a ready, promoted deployment");
  }
  const routeList = await request(`${collection}/${workerId}/routes`);
  if (!record(routeList) || !Array.isArray(routeList.routes)) throw new Error("invalid Worker routes response");
  const routes: readonly unknown[] = routeList.routes;
  const defaults = routes.filter(route => record(route) && route.kind === "platform_path");
  const route = defaults[0];
  if (defaults.length !== 1 || !record(route) || route.workerId !== workerId || route.accountId !== account
      || typeof route.pathPrefix !== "string" || !route.pathPrefix.startsWith("/") || route.pathPrefix.startsWith("//")) {
    throw new Error("default Worker route is unavailable");
  }
  const url = new URL(route.pathPrefix, endpoint);
  if (url.origin !== endpoint.origin || url.search || url.hash) throw new Error("invalid default Worker route");
  return { workerId, deploymentId: result.deployment.id, url: url.href, sha256: artifact.sha256 };
}
