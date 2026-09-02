export const queryKeys = {
  meta: ["operator", "meta"] as const,
  account: ["operator", "account"] as const,
  status: ["operator", "status"] as const,
  overview: {
    workers: (accountId: string) => ["operator", "overview", "workers", accountId] as const,
    kvNamespaces: (accountId: string) => ["operator", "overview", "kv", accountId] as const,
    d1Databases: (accountId: string) => ["operator", "overview", "d1", accountId] as const,
    r2Buckets: (accountId: string) => ["operator", "overview", "r2", accountId] as const,
    doNamespaces: (accountId: string) => ["operator", "overview", "durable-objects", accountId] as const,
    queues: (accountId: string) => ["operator", "overview", "queues", accountId] as const,
    workflows: (accountId: string) => ["operator", "overview", "workflows", accountId] as const,
  },
  workers: (accountId: string, search?: string) =>
    ["operator", "workers", accountId, search ?? ""] as const,
  worker: (accountId: string, workerId: string) => ["operator", "worker", accountId, workerId] as const,
  workerCache: (accountId: string, workerId: string) => ["operator", "worker", accountId, workerId, "cache"] as const,
  deployments: (accountId: string, workerId: string) => ["operator", "deployments", accountId, workerId] as const,
  routes: (accountId: string, workerId: string) => ["operator", "routes", accountId, workerId] as const,
  kvNamespaces: (accountId: string, search?: string) =>
    ["operator", "kv", accountId, "namespaces", search ?? ""] as const,
  kvKeys: (accountId: string, namespaceId: string, prefix?: string) =>
    ["operator", "kv", accountId, namespaceId, "keys", prefix ?? ""] as const,
  kvValue: (accountId: string, namespaceId: string, key: string) =>
    ["operator", "kv", accountId, namespaceId, "value", key] as const,
  kvBackups: (accountId: string) => ["operator", "kv", accountId, "backups"] as const,
  d1Databases: (accountId: string, search?: string) =>
    ["operator", "d1", accountId, "databases", search ?? ""] as const,
  d1Tables: (accountId: string, databaseId: string) => ["operator", "d1", accountId, databaseId, "tables"] as const,
  d1Migrations: (accountId: string, databaseId: string) => ["operator", "d1", accountId, databaseId, "migrations"] as const,
  d1Backups: (accountId: string, databaseId: string) => ["operator", "d1", accountId, databaseId, "backups"] as const,
  r2Buckets: (accountId: string, search?: string) =>
    ["operator", "r2", accountId, "buckets", search ?? ""] as const,
  r2Objects: (accountId: string, bucketId: string, prefix?: string) =>
    ["operator", "r2", accountId, bucketId, "objects", prefix ?? ""] as const,
  doNamespaces: (accountId: string, search?: string) =>
    ["operator", "do", accountId, "namespaces", search ?? ""] as const,
  doObjects: (accountId: string, namespaceId: string) => ["operator", "do", accountId, namespaceId, "objects"] as const,
  queues: (accountId: string, search?: string) =>
    ["operator", "queues", accountId, search ?? ""] as const,
  queue: (accountId: string, queueId: string) => ["operator", "queue", accountId, queueId] as const,
  workflows: (accountId: string, search?: string) =>
    ["operator", "workflows", accountId, search ?? ""] as const,
  workflow: (accountId: string, workflowId: string) => ["operator", "workflow", accountId, workflowId] as const,
  workflowVersions: (accountId: string, workflowId: string) =>
    ["operator", "workflow", accountId, workflowId, "versions"] as const,
  workflowInstances: (accountId: string, workflowId: string) =>
    ["operator", "workflow", accountId, workflowId, "instances"] as const,
  workflowInstance: (accountId: string, workflowId: string, instanceId: string) =>
    ["operator", "workflow", accountId, workflowId, "instance", instanceId] as const,
  workflowSteps: (accountId: string, workflowId: string, instanceId: string) =>
    ["operator", "workflow", accountId, workflowId, "instance", instanceId, "steps"] as const,
  scheduler: ["operator", "platform", "scheduler"] as const,
  queueConsumers: ["operator", "platform", "queue-consumers"] as const,
  cronActivations: ["operator", "platform", "cron-activations"] as const,
  cache: ["operator", "platform", "cache"] as const,
  imagesCapacity: ["operator", "platform", "images"] as const,
};
