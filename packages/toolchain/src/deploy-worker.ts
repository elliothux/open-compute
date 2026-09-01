import { createHash, randomUUID } from "node:crypto";
import { readAssetObject } from "./assets/scan.ts";
import type { ScannedAssets } from "./assets/types.ts";
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
  readonly sha256?: string;
}

interface ResolvedService {
  readonly targetWorkerId: string;
  readonly entrypoint?: string;
}

function deploymentBindings(bindings: WorkerProject["bindings"]): Record<string, unknown> {
  return Object.fromEntries(Object.entries(bindings).map(([name, binding]) => [name, {
    type: binding.type,
    id: binding.id,
    ...(binding.permissions === undefined ? {} : { permissions: binding.permissions }),
    ...(binding.type !== "workflow" || binding.schedules === undefined ? {} : {
      config: { workflowSchedules: binding.schedules },
    }),
  }]));
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
  return deployProject(project, artifact, undefined, options);
}

/** Deploy Worker code, static assets, or both through one immutable deployment model. */
export async function deployProject(
  project: WorkerProject,
  artifact: WorkerArtifact | undefined,
  assets: ScannedAssets | undefined,
  options: DeployOptions,
): Promise<WorkerDeployment> {
  if (artifact === undefined && assets === undefined) throw new Error("deployment has no Worker or static assets");
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
  const request = async (path: string, method = "GET", body?: RequestInit["body"],
    extraHeaders?: Record<string, string>, mutationKey?: string): Promise<unknown> => {
    const headers = new Headers(extraHeaders);
    if (options.token !== undefined) headers.set("authorization", `Bearer ${options.token}`);
    if (method === "POST") headers.set("idempotency-key", mutationKey ?? randomUUID());
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
  const workersByName = new Map<string, string[]>();
  let workerId: string | undefined;
  for (const item of workers) {
    if (!record(item) || !identifier(item.id) || item.accountId !== account || typeof item.name !== "string") throw new Error("invalid Worker list entry");
    if (item.deletedAtMs === null) {
      const named = workersByName.get(item.name) ?? [];
      named.push(item.id);
      workersByName.set(item.name, named);
    }
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
    workersByName.set(project.name, [workerId]);
  }
  const services: Record<string, ResolvedService> = {};
  for (const [binding, declaration] of Object.entries(project.services)) {
    const targets = workersByName.get(declaration.service) ?? [];
    if (targets.length !== 1) {
      throw new Error(targets.length === 0
        ? `Service target Worker does not exist: ${declaration.service}`
        : `Service target Worker name is ambiguous: ${declaration.service}`);
    }
    Object.defineProperty(services, binding, {
      value: {
        targetWorkerId: targets[0]!,
        ...(declaration.entrypoint === undefined ? {} : { entrypoint: declaration.entrypoint }),
      },
      enumerable: true,
    });
  }
  // HTTP field values are ASCII. Escaping UTF-16 code units preserves JSON values,
  // including astral characters, without putting credentials in URLs or artifacts.
  const metadata = JSON.stringify({
    ...(artifact === undefined ? {} : { mainModule: artifact.mainModule }),
    vars: project.vars, secrets,
    bindings: deploymentBindings(project.bindings), services,
    cache: project.runtimeFeatures.cache,
    ...(project.runtimeFeatures.images === undefined ? {} : { images: project.runtimeFeatures.images }),
    ...(project.runtimeFeatures.versionMetadata === undefined ? {} : {
      versionMetadata: project.runtimeFeatures.versionMetadata,
    }),
    promote: true,
  }).replace(/[^\x20-\x7e]/g, value => `\\u${value.charCodeAt(0).toString(16).padStart(4, "0")}`);
  if (metadata.length > 1024 * 1024) throw new Error("deployment metadata exceeds 1 MiB");
  let result: unknown;
  if (assets === undefined) {
    if (artifact === undefined) throw new Error("Worker deployment is missing its bundle");
    result = await request(`${collection}/${workerId}/deployments`, "POST", Buffer.from(artifact.bytes), {
      "content-type": "application/octet-stream", "x-open-compute-deployment-metadata": metadata,
    });
  } else {
    result = await deployAssets(request, collection, workerId, project, artifact, assets, secrets, services);
  }
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
  return {
    workerId,
    deploymentId: result.deployment.id,
    url: url.href,
    ...(artifact === undefined ? {} : { sha256: artifact.sha256 }),
  };
}

async function deployAssets(
  request: (path: string, method?: string, body?: RequestInit["body"],
    headers?: Record<string, string>, mutationKey?: string) => Promise<unknown>,
  collection: string,
  workerId: string,
  project: WorkerProject,
  artifact: WorkerArtifact | undefined,
  assets: ScannedAssets,
  secrets: Record<string, string>,
  services: Record<string, ResolvedService>,
): Promise<unknown> {
  const createBody = JSON.stringify({
    contentKind: artifact === undefined ? "assets_only" : "worker",
    ...(artifact === undefined ? {} : { bundle: { sha256: artifact.sha256, size: artifact.bytes.byteLength } }),
    manifest: assets.manifest,
    routing: assets.routing,
  });
  // This input-derived key survives a CLI process restart without storing credentials.
  // The server scopes it to the account and Worker and rejects any fingerprint drift.
  const createKey = `oc-assets-${createHash("sha256").update(createBody).digest("hex")}`;
  const created = await request(
    `${collection}/${workerId}/deployment-uploads`,
    "POST",
    createBody,
    { "content-type": "application/json" },
    createKey,
  );
  if (!record(created) || !identifier(created.id) || created.workerId !== workerId
      || !Array.isArray(created.objects)) throw new Error("invalid deployment upload response");
  const uploadId = created.id;
  for (const raw of created.objects as readonly unknown[]) {
    if (!record(raw) || typeof raw.sha256 !== "string" || !/^[0-9a-f]{64}$/.test(raw.sha256)
        || typeof raw.size !== "number" || !Number.isSafeInteger(raw.size) || raw.size < 0
        || typeof raw.verified !== "boolean" || typeof raw.kind !== "string") {
      throw new Error("invalid deployment upload inventory");
    }
    if (raw.verified) continue;
    let bytes: Uint8Array;
    if (raw.kind === "bundle") {
      if (!artifact || raw.sha256 !== artifact.sha256 || raw.size !== artifact.bytes.byteLength) {
        throw new Error("deployment upload bundle inventory changed");
      }
      bytes = artifact.bytes;
    } else if (raw.kind === "asset_blob") {
      const source = assets.objects.get(raw.sha256);
      if (!source || source.size !== raw.size) throw new Error("deployment upload asset inventory changed");
      bytes = await readAssetObject(source);
    } else if (raw.kind === "asset_manifest") {
      // The server verifies and confirms the canonical manifest submitted at session creation.
      throw new Error("platform did not confirm the submitted asset manifest");
    } else throw new Error("invalid deployment upload object kind");
    await request(
      `${collection}/${workerId}/deployment-uploads/${uploadId}/objects/${raw.sha256}`,
      "PUT",
      Buffer.from(bytes),
      { "content-type": "application/octet-stream", "content-length": String(bytes.byteLength) },
    );
  }
  return request(
    `${collection}/${workerId}/deployment-uploads/${uploadId}/finalize`,
    "POST",
    JSON.stringify({
      ...(artifact === undefined ? {} : { mainModule: artifact.mainModule }),
      vars: project.vars,
      secrets,
      bindings: deploymentBindings(project.bindings),
      services,
      cache: project.runtimeFeatures.cache,
      ...(project.runtimeFeatures.images === undefined ? {} : { images: project.runtimeFeatures.images }),
      ...(project.runtimeFeatures.versionMetadata === undefined ? {} : {
        versionMetadata: project.runtimeFeatures.versionMetadata,
      }),
      promote: true,
    }),
    { "content-type": "application/json" },
    `oc-assets-finalize-${uploadId}`,
  );
}
