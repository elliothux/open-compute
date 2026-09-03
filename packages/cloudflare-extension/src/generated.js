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
];
