import type { DoPolicyEnv, DoRouterRpc } from "../durable-objects/protocol.js";
import type { QueueTransportEnv } from "../queues/protocol.js";

/** Private system services; this shape must never be copied into tenant env. */
export interface LoaderEnv extends QueueTransportEnv, DoPolicyEnv {
  LOADER: WorkerLoader;
  WORKER_LOADER_FACTORY: NativeWorkerLoaderFactory;
  HOST_EXTENSION_FACTORY: NativeHostExtensionFactory;
  RUNTIME_SOURCE: Fetcher;
  DO_ROUTER: DoRouterRpc;
  OUTBOUND_NETWORK: Fetcher;
  SYSTEM_COMPATIBILITY_DATE: string;
  SYSTEM_COMPATIBILITY_FLAGS: string[];
  OBSERVABILITY_BACKEND: Fetcher;
}

/** Host-only factory. Only the trusted loader can mint an extension session port. */
export interface NativeHostExtensionFactory {
  get(sessionIdentity: string): NativeHostExtensionPort;
}

/** Session-scoped opaque byte-call capability injected only into an extension facade. */
export interface NativeHostExtensionPort {
  call(method: number, payload: Uint8Array): Promise<Uint8Array>;
  stream(method: number, payload: Uint8Array): ReadableStream<Uint8Array>;
}

/** Host-only fork capability; it is never exposed to tenant isolates. */
export interface NativeWorkerLoaderFactory {
  readonly hostPolicyVersion: 1;
  getFacets(facets: DurableObjectFacets): NativeHostFacets;
  getContainer(
    facets: DurableObjectFacets,
    className: string,
    resolve: (
      start: boolean,
      stop: boolean,
      completed: boolean,
      startup: string,
      expectedLeaseKey: string,
      monitor: boolean,
    ) => Promise<NativeContainerLease | undefined>,
    capture: (
      expectedLeaseKey: string,
      name: string | undefined,
      directory: string | undefined,
    ) => Promise<NativeContainerSnapshot>,
    images: Readonly<Record<string, string>>,
  ): NativeHostContainer;
  get(namespaceKey: string): WorkerLoader;
  getPrivate(namespaceKey: string): NativePrivateWorkerLoader;
  revoke(namespaceKey: string): void;
  revokePrefix(namespacePrefix: string): void;
  getEntrypoint<T extends Rpc.WorkerEntrypointBranded | undefined = undefined>(
    stub: WorkerStub,
    tails: Fetcher[],
    name?: string,
    options?: WorkerStubEntrypointOptions,
  ): Fetcher<T>;
}

/** Non-delegatable host grant for the fork's single-use snapshot operation. */
export interface NativePrivateWorkerLoader extends WorkerLoader {
  preparePython(code: WorkerLoaderWorkerCode): Promise<Uint8Array<ArrayBuffer>>;
}

/** Private, revocable host grant retained only by the trusted DO wrapper. */
export interface NativeHostFacets {
  create(name: string, depth: number, id: string, actorClass: unknown): void;
  revoke(): void;
}

/** Process-local attachment grant; it cannot enter tenant env or cross RPC. */
export interface NativeHostContainer {
  start(): Promise<void>;
  revoke(): void;
}
/** Qualified execution identity consumed only by the native engine client. */
export interface NativeContainerLease {
  leaseKey: string;
  imageRef: string;
  providerContractSha256: string;
  controlToken: string;
  callbackToken: string;
  nativeStartupJson: string;
  inspection: {
    image: string;
    labels: Array<{ name: string; value: string }>;
  };
}

/** Verified public metadata returned by the private snapshot authority. */
export interface NativeContainerSnapshot {
  id: string;
  size: number;
  name?: string;
  dir?: string;
}
export interface RuntimeContainerClass {
  className: string;
  namespaceResourceId: string;
  applicationName: string;
  descriptorSha256: string;
  images: Readonly<Record<string, string>>;
}

export interface RuntimeObservabilityIdentity {
  schemaVersion: 1;
  instanceId: string;
  workerId: string;
  scriptName: string;
  versionId: string;
  deploymentId?: string;
  routeGeneration: number;
  observabilityGeneration: number;
  enabled: boolean;
  logsEnabled: boolean;
  headSamplingRate: number;
  invocationLogs: boolean;
  persist: boolean;
}
interface RuntimeBindingBase {
  name: string;
  bindingId: string;
  descriptorSha256: string;
  capabilityVersion: number;
}
export interface RuntimeResourceBinding extends RuntimeBindingBase {
  kind:
    | "kv_namespace"
    | "r2_bucket"
    | "d1_database"
    | "do_namespace"
    | "vectorize_index"
    | "ai_search_namespace"
    | "ai_search_instance"
    | "artifacts_namespace";
  resourceId: string;
  resourceSpecGeneration: number;
  permissions: { read: boolean; write: boolean };
  namespacePrefix?: string;
  namespaceNameKey?: string;
}
export interface RuntimeQueueBinding extends RuntimeBindingBase {
  kind: "queue_producer";
  queueId: string;
  queueLifecycleGeneration: number;
}
export interface RuntimeWorkflowBinding extends RuntimeBindingBase {
  kind: "workflow";
  schedules: string[];
}
export interface RuntimeScheduledTarget {
  cron: string;
  scheduledHandler: boolean;
  workflowBindings: string[];
}
export type RuntimeBinding =
  RuntimeResourceBinding | RuntimeQueueBinding | RuntimeWorkflowBinding;
export interface RuntimeServiceBinding {
  schemaVersion: 2;
  name: string;
  target:
    { kind: "worker"; workerId: string } | { kind: "extension"; name: string };
  entrypoint?: string;
  props?: Record<string, unknown>;
  policyVersion: 1;
  descriptorSha256: string;
}
export interface RuntimeModule {
  name: string;
  type:
    | "esModule"
    | "commonJsModule"
    | "python"
    | "text"
    | "json"
    | "data"
    | "wasm";
  bytesBase64: string;
}
export interface RuntimeModuleBinding {
  name: string;
  type: "text" | "data" | "wasm";
  bytesBase64: string;
}
/** Exact private RuntimeSource authority operation selected by the host caller. */
export type RuntimeSourceScope =
  "runtime" | "validation" | "probe" | "preparation";

/** Wire projection produced only by Rust RuntimeSource::internal_payload. */
export interface RuntimeSnapshot {
  pythonPreparedSha256?: string;
  schemaVersion: 1;
  loaderKey: string;
  workerCodeSha256: string;
  routeGeneration: number;
  observability?: RuntimeObservabilityIdentity;
  compatibilityDate: string;
  compatibilityFlags: string[];
  limits: { cpuMs: number; subRequests: number };
  contentKind: "worker" | "assets_only";
  mainModule?: string;
  modules: RuntimeModule[];
  moduleBindings: RuntimeModuleBinding[];
  workerLoaders: { name: string; namespaceKey: string }[];
  env: Record<string, unknown>;
  bindings: RuntimeBinding[];
  scheduledTargets: RuntimeScheduledTarget[];
  services: RuntimeServiceBinding[];
  containerClasses: RuntimeContainerClass[];
  cachePolicy: {
    enabled: boolean;
    crossVersionCache: boolean;
    failOpen: boolean;
    entrypoints: Record<
      string,
      { enabled: boolean; crossVersionCache: boolean }
    >;
  };
  imagesBinding?: { name: string; descriptorSha256: string };
  browserBindings: { name: string; descriptorSha256: string }[];
  aiBinding?: { name: string; descriptorSha256: string };
  versionMetadataBinding?: {
    name: string;
    id: string;
    tag?: string;
    timestampMs: number;
    descriptorSha256: string;
  };
  assetBinding?: { name: string };
  assets?: RuntimeAssets;
}
export interface RuntimeAssetEntry {
  path: string;
  sha256: string;
  size: number;
  contentType: string;
}
export interface RuntimeAssetManifest {
  schemaVersion: 1;
  entries: RuntimeAssetEntry[];
}
export interface RuntimeAssetRouting {
  schemaVersion: 1;
  binding?: string;
  runWorkerFirst: boolean | string[];
  htmlHandling:
    | "auto-trailing-slash"
    | "force-trailing-slash"
    | "drop-trailing-slash"
    | "none";
  notFoundHandling: "none" | "404-page" | "single-page-application";
  headers: unknown[];
  redirects: Array<{ from: string; to: string; status: number }>;
}
export interface RuntimeAssets {
  manifest: RuntimeAssetManifest;
  routing: RuntimeAssetRouting;
}
export interface RuntimeEnvelope {
  loaderKey: string;
  expected: string;
  routeGeneration?: number;
}
export interface DispatchEnvelope extends RuntimeEnvelope {
  routeGeneration: number;
  runtimeKey: string;
}
export type BindingContext = Pick<ExecutionContext, "exports">;
