import { createHash, randomUUID } from "node:crypto";
import { readAssetObject } from "./assets/scan.ts";
import type { ScannedAssets } from "./assets/types.ts";
import type { WorkerArtifact } from "./bundle-worker.ts";
import type { WorkerProject } from "./project.ts";
import {
  createOperatorClient,
  parseAccountId,
  parseDeploymentUploadId,
  parseSha256Digest,
  parseWorkerId,
} from "@open-compute/operator-sdk";

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
  const platformFetch: typeof fetch = (input, init) => fetch(input, {
    ...init,
    redirect: "error",
    signal: init?.signal ?? AbortSignal.timeout(120_000),
  });
  const client = createOperatorClient({
    baseUrl: new URL("/operator/api/v1/", endpoint),
    getAccessToken: () => options.token ?? "",
    fetch: platformFetch,
  });

  let account = options.accountId;
  if (account === undefined) {
    const identity = await client.system.account();
    account = identity.accountId;
  }
  if (!identifier(account)) throw new Error("invalid account ID");
  const parsedAccount = parseAccountId(account);
  const listed = await client.workers.list({ accountId: parsedAccount });
  const workers = listed.workers;
  const workersByName = new Map<string, string[]>();
  let workerId: string | undefined;
  for (const item of workers) {
    if (item.deletedAtMs !== null && item.deletedAtMs !== undefined) continue;
    const named = workersByName.get(item.name) ?? [];
    named.push(item.id);
    workersByName.set(item.name, named);
    if (item.name === project.name) {
      if (workerId !== undefined) throw new Error("ambiguous Worker name");
      workerId = item.id;
    }
  }
  if (workerId === undefined) {
    const created = await client.workers.create({
      accountId: parsedAccount,
      name: project.name,
      idempotencyKey: randomUUID(),
    });
    workerId = created.worker.id;
    workersByName.set(project.name, [workerId]);
  }
  if (workerId === undefined) {
    throw new Error("Worker resolution failed");
  }
  const resolvedWorkerId = workerId;
  const parsedWorkerId = parseWorkerId(resolvedWorkerId);
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
  let result;
  if (assets === undefined) {
    if (artifact === undefined) throw new Error("Worker deployment is missing its bundle");
    result = await client.workers.createDeployment({
      accountId: parsedAccount,
      workerId: parsedWorkerId,
      bundle: Buffer.from(artifact.bytes),
      metadata,
      idempotencyKey: randomUUID(),
    });
  } else {
    result = await deployAssets(client, parsedAccount, parsedWorkerId, project, artifact, assets, secrets, services);
  }
  if (!result.promoted || result.deployment.state !== "ready" || result.deployment.workerId !== resolvedWorkerId) {
    throw new Error("platform did not confirm a ready, promoted deployment");
  }
  const routeList = await client.workers.listRoutes({
    accountId: parsedAccount,
    workerId: parsedWorkerId,
  });
  const defaults = routeList.routes.filter(route => route.kind === "platform_path");
  const route = defaults[0];
  if (defaults.length !== 1 || route === undefined || route.workerId !== resolvedWorkerId
      || route.accountId !== parsedAccount
      || !route.pathPrefix.startsWith("/") || route.pathPrefix.startsWith("//")) {
    throw new Error("default Worker route is unavailable");
  }
  const url = new URL(route.pathPrefix, endpoint);
  if (url.origin !== endpoint.origin || url.search || url.hash) throw new Error("invalid default Worker route");
  return {
    workerId: resolvedWorkerId,
    deploymentId: result.deployment.id,
    url: url.href,
    ...(artifact === undefined ? {} : { sha256: artifact.sha256 }),
  };
}

async function deployAssets(
  client: ReturnType<typeof createOperatorClient>,
  accountId: ReturnType<typeof parseAccountId>,
  workerId: ReturnType<typeof parseWorkerId>,
  project: WorkerProject,
  artifact: WorkerArtifact | undefined,
  assets: ScannedAssets,
  secrets: Record<string, string>,
  services: Record<string, ResolvedService>,
) {
  const createBody = {
    contentKind: artifact === undefined ? "assets_only" as const : "worker" as const,
    ...(artifact === undefined ? {} : { bundle: { sha256: parseSha256Digest(artifact.sha256), size: artifact.bytes.byteLength } }),
    manifest: assets.manifest,
    routing: assets.routing,
  };
  const createKey = `oc-assets-${createHash("sha256").update(JSON.stringify(createBody)).digest("hex")}`;
  const created = await client.workers.createDeploymentUpload({
    accountId,
    workerId,
    body: createBody,
    idempotencyKey: createKey,
  });
  const uploadId = parseDeploymentUploadId(created.id);
  for (const object of created.objects) {
    if (object.verified) continue;
    let bytes: Uint8Array;
    if (object.kind === "bundle") {
      if (!artifact || object.sha256 !== artifact.sha256 || object.size !== artifact.bytes.byteLength) {
        throw new Error("deployment upload bundle inventory changed");
      }
      bytes = artifact.bytes;
    } else if (object.kind === "asset_blob") {
      const source = assets.objects.get(object.sha256);
      if (!source || source.size !== object.size) throw new Error("deployment upload asset inventory changed");
      bytes = await readAssetObject(source);
    } else if (object.kind === "asset_manifest") {
      throw new Error("platform did not confirm the submitted asset manifest");
    } else {
      throw new Error("invalid deployment upload object kind");
    }
    await client.workers.putDeploymentUploadObject({
      accountId,
      workerId,
      uploadId,
      sha256: object.sha256,
      body: Buffer.from(bytes),
    });
  }
  return client.workers.finalizeDeploymentUpload({
    accountId,
    workerId,
    uploadId,
    idempotencyKey: `oc-assets-finalize-${created.id}`,
    body: {
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
    },
  });
}
