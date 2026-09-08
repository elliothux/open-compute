/// <reference types="@cloudflare/workers-types" />

declare module "open-compute:ai" {
  /** Workers AI binding surface currently implemented by open-compute. */
  export type OpenComputeAi = Pick<Ai, "aiGatewayLogId" | "toMarkdown">;
}

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
