import type { BindingEnv } from "../bindings/protocol.js";
import type { DoPolicyEnv, DoRouterRpc } from "../durable-objects/protocol.js";
/** Private system services; this shape must never be copied into tenant env. */
export interface LoaderEnv extends BindingEnv, DoPolicyEnv {
  LOADER: WorkerLoader;
  RUNTIME_SOURCE: Fetcher;
  INTERNAL_TOKEN: string;
  DO_ROUTER: DoRouterRpc;
  PUBLIC_NETWORK: Fetcher;
  COMPATIBILITY_DATE: string;
  REQUIRED_COMPATIBILITY_FLAGS: string[];
}
interface RuntimeBindingBase {
  name: string;
  bindingId: string;
  descriptorSha256: string;
  capabilityVersion: number;
}
export interface RuntimeResourceBinding extends RuntimeBindingBase {
  kind: "kv_namespace" | "r2_bucket" | "d1_database" | "do_namespace" | "vectorize_index"
    | "ai_search_namespace" | "ai_search_instance";
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
export type RuntimeBinding = RuntimeResourceBinding | RuntimeQueueBinding | RuntimeWorkflowBinding;
export interface RuntimeServiceBinding {
  schemaVersion: 1;
  name: string;
  targetWorkerId: string;
  entrypoint?: string;
  policyVersion: 1;
  descriptorSha256: string;
}
export interface RuntimeModule {
  name: string;
  type: "esModule" | "commonJsModule" | "text" | "json" | "data" | "wasm";
  bytesBase64: string;
}
/** Wire projection produced only by Rust RuntimeSource::internal_payload. */
export interface RuntimeSnapshot {
  schemaVersion: 1;
  loaderKey: string;
  workerCodeSha256: string;
  routeGeneration: number;
  compatibilityDate: string;
  compatibilityFlags: string[];
  contentKind: "worker" | "assets_only";
  mainModule?: string;
  modules: RuntimeModule[];
  env: Record<string, unknown>;
  bindings: RuntimeBinding[];
  scheduledTargets: RuntimeScheduledTarget[];
  services: RuntimeServiceBinding[];
  cachePolicy: {
    enabled: boolean;
    crossVersionCache: boolean;
    failOpen: boolean;
    entrypoints: Record<string, { enabled: boolean; crossVersionCache: boolean }>;
  };
  imagesBinding?: { name: string; descriptorSha256: string };
  aiBinding?: { name: string; descriptorSha256: string };
  versionMetadataBinding?: {
    name: string; id: string; tag?: string; timestampMs: number; descriptorSha256: string;
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
export interface RuntimeAssetManifest { schemaVersion: 1; entries: RuntimeAssetEntry[] }
export interface RuntimeAssetRouting {
  schemaVersion: 1;
  binding?: string;
  runWorkerFirst: boolean | string[];
  htmlHandling: "auto-trailing-slash" | "force-trailing-slash" | "drop-trailing-slash" | "none";
  notFoundHandling: "none" | "404-page" | "single-page-application";
  headers: unknown[];
  redirects: Array<{ from: string; to: string; status: number }>;
}
export interface RuntimeAssets { manifest: RuntimeAssetManifest; routing: RuntimeAssetRouting }
export interface RuntimeEnvelope { loaderKey: string; expected: string }
export interface DispatchEnvelope extends RuntimeEnvelope { routeGeneration: number; runtimeKey: string }
export type BindingContext = Pick<ExecutionContext, "exports">;
