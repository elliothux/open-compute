import type { RuntimeBinding } from "../loader/protocol.js";

/** Host-only construction wire. workerd validates the opaque Fetcher capability. */
export type NativeBinding =
  | { kind: "kvNamespace" | "r2Bucket" | "service" | "queue"; fetcher: unknown }
  | {
      kind: "wrapped";
      fetcher: unknown;
      wrapperModule:
        | "cloudflare-internal:d1-api"
        | "cloudflare-internal:open-compute-vectorize"
        | "cloudflare-internal:open-compute-browser"
        | "cloudflare-internal:open-compute-images"
        | "cloudflare-internal:open-compute-ai"
        | "cloudflare-internal:open-compute-artifacts"
        | "cloudflare-internal:open-compute-assets"
        | "cloudflare-internal:open-compute-ai-search-namespace"
        | "cloudflare-internal:open-compute-ai-search-instance";
    };

/** Select the single native implementation for a validated product descriptor. */
export function nativeBinding(
  kind:
    RuntimeBinding["kind"] | "images" | "ai" | "assets" | "service" | "browser",
  fetcher: unknown,
): NativeBinding | undefined {
  switch (kind) {
    case "service":
      return { kind: "service", fetcher };
    case "queue_producer":
      return { kind: "queue", fetcher };
    case "kv_namespace":
      return { kind: "kvNamespace", fetcher };
    case "r2_bucket":
      return { kind: "r2Bucket", fetcher };
    case "d1_database":
      return {
        kind: "wrapped",
        fetcher,
        wrapperModule: "cloudflare-internal:d1-api",
      };
    case "vectorize_index":
      return {
        kind: "wrapped",
        fetcher,
        wrapperModule: "cloudflare-internal:open-compute-vectorize",
      };
    case "browser":
      return {
        kind: "wrapped",
        fetcher,
        wrapperModule: "cloudflare-internal:open-compute-browser",
      };
    case "images":
      return {
        kind: "wrapped",
        fetcher,
        wrapperModule: "cloudflare-internal:open-compute-images",
      };
    case "ai":
      return {
        kind: "wrapped",
        fetcher,
        wrapperModule: "cloudflare-internal:open-compute-ai",
      };
    case "artifacts_namespace":
      return {
        kind: "wrapped",
        fetcher,
        wrapperModule: "cloudflare-internal:open-compute-artifacts",
      };
    case "assets":
      return {
        kind: "wrapped",
        fetcher,
        wrapperModule: "cloudflare-internal:open-compute-assets",
      };
    case "ai_search_namespace":
      return {
        kind: "wrapped",
        fetcher,
        wrapperModule: "cloudflare-internal:open-compute-ai-search-namespace",
      };
    case "ai_search_instance":
      return {
        kind: "wrapped",
        fetcher,
        wrapperModule: "cloudflare-internal:open-compute-ai-search-instance",
      };
    default:
      return undefined;
  }
}
