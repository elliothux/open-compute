import type { BindingEnv } from "../bindings/protocol.js";
import type { DoPolicyEnv } from "../durable-objects/protocol.js";
/** Private system services; this shape must never be copied into tenant env. */
export interface LoaderEnv extends BindingEnv, DoPolicyEnv {
  LOADER: WorkerLoader;
  RUNTIME_SOURCE: Fetcher;
  INTERNAL_TOKEN: string;
  DO_ROUTER: Fetcher;
}
interface RuntimeBindingBase {
  name: string;
  bindingId: string;
  descriptorSha256: string;
  capabilityVersion: number;
}
export interface RuntimeResourceBinding extends RuntimeBindingBase {
  kind: "kv_namespace" | "r2_bucket" | "d1_database" | "do_namespace";
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
}
export type RuntimeBinding = RuntimeResourceBinding | RuntimeQueueBinding | RuntimeWorkflowBinding;
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
  contentKind: "worker" | "assets_only";
  mainModule?: string;
  compatibilityDate: string;
  compatibilityFlags: string[];
  modules: RuntimeModule[];
  env: Record<string, unknown>;
  bindings: RuntimeBinding[];
  assetBinding?: { name: string };
  assets?: RuntimeAssets;
  limits: unknown;
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
