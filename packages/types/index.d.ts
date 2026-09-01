/// <reference types="@cloudflare/workers-types" />

declare module "open-compute:cache" {
  export interface CachePurgeOptions {
    tags?: string[];
    pathPrefixes?: string[];
    purgeEverything?: boolean;
  }

  export interface CachePurgeResult {
    readonly success: boolean;
    readonly deleted: number;
  }

  export interface ExecutionContextCache {
    purge(options?: CachePurgeOptions): Promise<CachePurgeResult>;
  }
}
