import type Cloudflare from "cloudflare";
import type { BaseCloudflare } from "cloudflare/client";
import type {
  Backup,
  CacheStatus,
  Capabilities,
  DurableObjectNamespace,
  DurableObjectRecord,
  ImageCapacity,
  SchedulerStatus,
  SystemStatus,
  WorkerEndpoint,
} from "./generated.js";

export * from "./generated.js";

type RequestOptions = Cloudflare.RequestOptions;

interface V4Envelope<T> {
  readonly success: true;
  readonly result: T;
  readonly errors: readonly unknown[];
  readonly messages: readonly unknown[];
}

function get<T>(client: BaseCloudflare, path: string, options?: RequestOptions) {
  return client.get<V4Envelope<T>>(path, options)._thenUnwrap(envelope => envelope.result);
}

function post<T>(client: BaseCloudflare, path: string, options?: RequestOptions) {
  return client.post<V4Envelope<T>>(path, options)._thenUnwrap(envelope => envelope.result);
}

function segment(value: string): string {
  if (value.length === 0 || value === "." || value === "..") throw new Error("invalid extension path segment");
  return encodeURIComponent(value);
}

/**
 * Bind open-compute-only operations to an already configured official
 * Cloudflare client. Authentication, retries, fetch and v4 error parsing remain
 * owned by that client.
 */
export function createOpenComputeExtension(client: BaseCloudflare) {
  return {
    capabilities: {
      get: (options?: RequestOptions) => get<Capabilities>(client, "/open-compute/capabilities", options),
    },
    system: {
      status: (options?: RequestOptions) => get<SystemStatus>(client, "/open-compute/system/status", options),
    },
    scheduler: {
      get: (options?: RequestOptions) => get<SchedulerStatus>(client, "/open-compute/scheduler", options),
      pause: (options?: RequestOptions) => post<SchedulerStatus>(client, "/open-compute/scheduler/pause", options),
      resume: (options?: RequestOptions) => post<SchedulerStatus>(client, "/open-compute/scheduler/resume", options),
      repair: (options?: RequestOptions) => post<SchedulerStatus>(client, "/open-compute/scheduler/repair", options),
    },
    cache: {
      get: (options?: RequestOptions) => get<CacheStatus>(client, "/open-compute/cache", options),
      collectGarbage: (options?: RequestOptions) =>
        post<CacheStatus>(client, "/open-compute/cache/garbage-collection", options),
    },
    images: {
      capacity: (options?: RequestOptions) => get<ImageCapacity>(client, "/open-compute/images/capacity", options),
    },
    workers: {
      endpoints: (accountID: string, scriptName: string, options?: RequestOptions) =>
        get<readonly WorkerEndpoint[]>(client,
          `/accounts/${segment(accountID)}/open-compute/workers/${segment(scriptName)}/endpoints`,
          options,
        ),
    },
    durableObjects: {
      list: (accountID: string, options?: RequestOptions) =>
        get<readonly DurableObjectNamespace[]>(client,
          `/accounts/${segment(accountID)}/open-compute/durable-objects`,
          options,
        ),
      objects: (accountID: string, namespaceID: string, options?: RequestOptions) =>
        get<readonly DurableObjectRecord[]>(client,
          `/accounts/${segment(accountID)}/open-compute/durable-objects/${segment(namespaceID)}/objects`,
          options,
        ),
    },
    backups: {
      kv: {
        create: (accountID: string, namespaceID: string, options?: RequestOptions) =>
          post<Backup>(client,
            `/accounts/${segment(accountID)}/open-compute/kv/namespaces/${segment(namespaceID)}/backups`,
            options,
          ),
        list: (accountID: string, namespaceID: string, options?: RequestOptions) =>
          get<readonly Backup[]>(client,
            `/accounts/${segment(accountID)}/open-compute/kv/namespaces/${segment(namespaceID)}/backups`,
            options,
          ),
        restore: (accountID: string, backupID: string, options?: RequestOptions) =>
          post<Backup>(client,
            `/accounts/${segment(accountID)}/open-compute/kv/backups/${segment(backupID)}/restore`,
            options,
          ),
      },
      d1: {
        create: (accountID: string, databaseID: string, options?: RequestOptions) =>
          post<Backup>(client,
            `/accounts/${segment(accountID)}/open-compute/d1/databases/${segment(databaseID)}/backups`,
            options,
          ),
        list: (accountID: string, databaseID: string, options?: RequestOptions) =>
          get<readonly Backup[]>(client,
            `/accounts/${segment(accountID)}/open-compute/d1/databases/${segment(databaseID)}/backups`,
            options,
          ),
        restore: (accountID: string, backupID: string, options?: RequestOptions) =>
          post<Backup>(client,
            `/accounts/${segment(accountID)}/open-compute/d1/backups/${segment(backupID)}/restore`,
            options,
          ),
      },
    },
  } as const;
}

export type OpenComputeExtension = ReturnType<typeof createOpenComputeExtension>;
