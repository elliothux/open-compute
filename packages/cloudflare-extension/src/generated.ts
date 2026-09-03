// Generated from ../../openapi/open-compute-extension.json. Do not edit.
export const OPEN_COMPUTE_EXTENSION_SCHEMA_SHA256 = "5e394ae27d3b743fa316444b48fc0cf8dd78619f398f5bf417d5bb43c2b87ee1";

export const OPEN_COMPUTE_EXTENSION_OPERATIONS = [
  {
    "method": "GET",
    "path": "/accounts/{account_id}/open-compute/d1/databases/{database_id}/backups",
    "operationId": "open-compute-get-accounts-account-id-open-compute-d1-databases-database-id-backups"
  },
  {
    "method": "GET",
    "path": "/accounts/{account_id}/open-compute/durable-objects",
    "operationId": "open-compute-get-accounts-account-id-open-compute-durable-objects"
  },
  {
    "method": "GET",
    "path": "/accounts/{account_id}/open-compute/durable-objects/{namespace_id}/objects",
    "operationId": "open-compute-get-accounts-account-id-open-compute-durable-objects-namespace-id-objects"
  },
  {
    "method": "GET",
    "path": "/accounts/{account_id}/open-compute/kv/namespaces/{namespace_id}/backups",
    "operationId": "open-compute-get-accounts-account-id-open-compute-kv-namespaces-namespace-id-backups"
  },
  {
    "method": "GET",
    "path": "/accounts/{account_id}/open-compute/workers/{script_name}/endpoints",
    "operationId": "open-compute-get-accounts-account-id-open-compute-workers-script-name-endpoints"
  },
  {
    "method": "GET",
    "path": "/open-compute/cache",
    "operationId": "open-compute-get-open-compute-cache"
  },
  {
    "method": "GET",
    "path": "/open-compute/capabilities",
    "operationId": "open-compute-get-open-compute-capabilities"
  },
  {
    "method": "GET",
    "path": "/open-compute/images/capacity",
    "operationId": "open-compute-get-open-compute-images-capacity"
  },
  {
    "method": "GET",
    "path": "/open-compute/scheduler",
    "operationId": "open-compute-get-open-compute-scheduler"
  },
  {
    "method": "GET",
    "path": "/open-compute/system/status",
    "operationId": "open-compute-get-open-compute-system-status"
  },
  {
    "method": "POST",
    "path": "/accounts/{account_id}/open-compute/d1/backups/{backup_id}/restore",
    "operationId": "open-compute-post-accounts-account-id-open-compute-d1-backups-backup-id-restore"
  },
  {
    "method": "POST",
    "path": "/accounts/{account_id}/open-compute/d1/databases/{database_id}/backups",
    "operationId": "open-compute-post-accounts-account-id-open-compute-d1-databases-database-id-backups"
  },
  {
    "method": "POST",
    "path": "/accounts/{account_id}/open-compute/kv/backups/{backup_id}/restore",
    "operationId": "open-compute-post-accounts-account-id-open-compute-kv-backups-backup-id-restore"
  },
  {
    "method": "POST",
    "path": "/accounts/{account_id}/open-compute/kv/namespaces/{namespace_id}/backups",
    "operationId": "open-compute-post-accounts-account-id-open-compute-kv-namespaces-namespace-id-backups"
  },
  {
    "method": "POST",
    "path": "/open-compute/cache/garbage-collection",
    "operationId": "open-compute-post-open-compute-cache-garbage-collection"
  },
  {
    "method": "POST",
    "path": "/open-compute/scheduler/pause",
    "operationId": "open-compute-post-open-compute-scheduler-pause"
  },
  {
    "method": "POST",
    "path": "/open-compute/scheduler/repair",
    "operationId": "open-compute-post-open-compute-scheduler-repair"
  },
  {
    "method": "POST",
    "path": "/open-compute/scheduler/resume",
    "operationId": "open-compute-post-open-compute-scheduler-resume"
  }
] as const;

export type PathSegment = string;

export type Error = {
  readonly code: number;
  readonly message: string;
  readonly source?: {
    readonly pointer?: string;
  };
};

export type Message = {
  readonly code: number;
  readonly message: string;
};

export type Capabilities = {
  readonly release: string;
  readonly wrangler_version: "4.127.1";
  readonly compatibility_date: {
    readonly minimum: string;
    readonly maximum: string;
  };
  readonly compatibility_flags: readonly string[];
  readonly endpoints: Record<string, "supported" | "supported_with_deviation" | "unsupported">;
  readonly deviations: readonly string[];
};

export type SystemStatus = {
  readonly state: string;
  readonly version: string;
  readonly components: readonly {
    readonly name: string;
    readonly state: string;
    readonly message?: string;
  }[];
};

export type SchedulerStatus = {
  readonly state: string;
  readonly pending: number;
  readonly running: number;
};

export type CacheStatus = {
  readonly entries: number;
  readonly bytes: number;
};

export type ImageCapacity = {
  readonly queued: number;
  readonly running: number;
  readonly capacity: number;
};

export type WorkerEndpoint = {
  readonly id: string;
  readonly path: string;
  readonly created_on: string;
};

export type DurableObjectNamespace = {
  readonly id: string;
  readonly script_name: string;
  readonly class_name: string;
};

export type DurableObjectRecord = {
  readonly id: string;
  readonly namespace_id: string;
  readonly created_on: string;
};

export type Backup = {
  readonly id: string;
  readonly created_on: string;
  readonly state: string;
  readonly size?: number;
};

export type RestoreRequest = {
  readonly name: string;
};

export type RestoredResource = {
  readonly id: string;
  readonly name: string;
  readonly kind: "kv_namespace" | "d1_database";
  readonly created_on: string;
};

export type ErrorEnvelope = {
  readonly success: false;
  readonly result: null;
  readonly errors: readonly Error[];
  readonly messages: readonly Message[];
};

export type CapabilitiesResponse = {
  readonly success: true;
  readonly result: Capabilities;
  readonly errors: readonly Error[];
  readonly messages: readonly Message[];
};

export type SystemStatusResponse = {
  readonly success: true;
  readonly result: SystemStatus;
  readonly errors: readonly Error[];
  readonly messages: readonly Message[];
};

export type SchedulerStatusResponse = {
  readonly success: true;
  readonly result: SchedulerStatus;
  readonly errors: readonly Error[];
  readonly messages: readonly Message[];
};

export type CacheStatusResponse = {
  readonly success: true;
  readonly result: CacheStatus;
  readonly errors: readonly Error[];
  readonly messages: readonly Message[];
};

export type ImageCapacityResponse = {
  readonly success: true;
  readonly result: ImageCapacity;
  readonly errors: readonly Error[];
  readonly messages: readonly Message[];
};

export type WorkerEndpointsResponse = {
  readonly success: true;
  readonly result: readonly WorkerEndpoint[];
  readonly errors: readonly Error[];
  readonly messages: readonly Message[];
};

export type DurableObjectNamespacesResponse = {
  readonly success: true;
  readonly result: readonly DurableObjectNamespace[];
  readonly errors: readonly Error[];
  readonly messages: readonly Message[];
};

export type DurableObjectRecordsResponse = {
  readonly success: true;
  readonly result: readonly DurableObjectRecord[];
  readonly errors: readonly Error[];
  readonly messages: readonly Message[];
};

export type BackupResponse = {
  readonly success: true;
  readonly result: Backup;
  readonly errors: readonly Error[];
  readonly messages: readonly Message[];
};

export type BackupsResponse = {
  readonly success: true;
  readonly result: readonly Backup[];
  readonly errors: readonly Error[];
  readonly messages: readonly Message[];
};

export type RestoredResourceResponse = {
  readonly success: true;
  readonly result: RestoredResource;
  readonly errors: readonly Error[];
  readonly messages: readonly Message[];
};
