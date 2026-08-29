/** Static asset project input. The directory is local-only and never enters a manifest. */
export interface AssetsProject {
  readonly directory: string;
  readonly binding?: string;
  readonly runWorkerFirst: boolean | readonly string[];
  readonly htmlHandling: "auto-trailing-slash" | "force-trailing-slash" | "drop-trailing-slash" | "none";
  readonly notFoundHandling: "none" | "404-page" | "single-page-application";
  readonly publishSourceMaps: boolean;
}

/** Canonical immutable manifest entry accepted by the Rust authority. */
export interface AssetManifestEntry {
  readonly path: string;
  readonly sha256: string;
  readonly size: number;
  readonly contentType: string;
}

/** Canonical immutable static asset manifest. */
export interface AssetManifest {
  readonly schemaVersion: 1;
  readonly entries: readonly AssetManifestEntry[];
}

/** Parsed custom response-header operation. */
export interface AssetHeaderOperation {
  readonly name: string;
  readonly value: string | null;
}

/** Parsed custom response-header rule. */
export interface AssetHeaderRule {
  readonly pattern: string;
  readonly operations: readonly AssetHeaderOperation[];
}

/** Parsed redirect or same-origin rewrite rule. */
export interface AssetRedirectRule {
  readonly from: string;
  readonly to: string;
  readonly status: 200 | 301 | 302 | 303 | 307 | 308;
}

/** Canonical routing configuration frozen into the deployment descriptor. */
export interface AssetRoutingConfig {
  readonly schemaVersion: 1;
  readonly binding?: string;
  readonly runWorkerFirst: boolean | readonly string[];
  readonly htmlHandling: AssetsProject["htmlHandling"];
  readonly notFoundHandling: AssetsProject["notFoundHandling"];
  readonly headers: readonly AssetHeaderRule[];
  readonly redirects: readonly AssetRedirectRule[];
}

/** Private local object source; `filename` never enters the wire manifest. */
export interface AssetObjectSource {
  readonly filename: string;
  readonly sha256: string;
  readonly size: number;
}

/** Scanned immutable asset deployment input. */
export interface ScannedAssets {
  readonly manifest: AssetManifest;
  readonly routing: AssetRoutingConfig;
  readonly objects: ReadonlyMap<string, AssetObjectSource>;
}
