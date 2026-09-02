import { z } from "zod";

export const DoNamespaceSchema = z.object({
  resourceId: z.string(),
  name: z.string(),
  className: z.string(),
  ownerWorkerId: z.string(),
  state: z.string(),
  schemaVersion: z.number(),
  createdAtMs: z.number(),
});

export type DoNamespace = z.output<typeof DoNamespaceSchema>;

export const DoNamespacesResponseSchema = z.object({
  namespaces: z.array(DoNamespaceSchema),
  cursor: z.string().nullable().optional(),
  listComplete: z.boolean().optional(),
});

export const DoNamespaceResponseSchema = z.strictObject({
  namespace: DoNamespaceSchema,
});

export type DoNamespaceResponse = z.output<typeof DoNamespaceResponseSchema>;

export const DoObjectSchema = z.strictObject({
  id: z.string(),
  generation: z.number(),
  lifecycle: z.string(),
  createdAtMs: z.number(),
  updatedAtMs: z.number().optional(),
});

export type DoObject = z.output<typeof DoObjectSchema>;

export const DoObjectsResponseSchema = z.strictObject({
  objects: z.array(DoObjectSchema),
  cursor: z.string().nullable().optional(),
});

export const DoObjectMutationResponseSchema = z.void();

export const QueueSchema = z.object({
  id: z.string(),
  accountId: z.string(),
  name: z.string(),
  state: z.string(),
  availability: z.string(),
  availabilityCode: z.string().nullable().optional(),
  lifecycleGeneration: z.number(),
  configGeneration: z.number(),
  deliveryDelaySeconds: z.number(),
  retentionSeconds: z.number(),
  maxMessageBytes: z.number(),
  maxBatchMessages: z.number(),
  maxBatchBytes: z.number(),
  maxBacklogBytes: z.number(),
  createdAtMs: z.number(),
  updatedAtMs: z.number(),
  deletedAtMs: z.number().nullable().optional(),
});

export type Queue = z.output<typeof QueueSchema>;

export const QueuesResponseSchema = z.object({
  queues: z.array(QueueSchema),
  nextCursor: z.string().nullable().optional(),
});

export const WorkflowSchema = z.object({
  id: z.string(),
  accountId: z.string(),
  name: z.string(),
  state: z.string(),
  availability: z.string(),
  availabilityCode: z.string().nullable().optional(),
  lifecycleGeneration: z.number(),
  currentVersionId: z.string().nullable().optional(),
  createdAtMs: z.number(),
  updatedAtMs: z.number(),
});

export type Workflow = z.output<typeof WorkflowSchema>;

export const WorkflowsResponseSchema = z.object({
  workflows: z.array(WorkflowSchema),
  nextCursor: z.string().nullable().optional(),
});

export const QueueResponseSchema = z.strictObject({
  queue: QueueSchema,
});

export type QueueResponse = z.output<typeof QueueResponseSchema>;

export const QueueDeleteResponseSchema = z.strictObject({
  queue: QueueSchema,
  purgedMessages: z.number(),
  purgedBytes: z.number(),
});

export type QueueDeleteResponse = z.output<typeof QueueDeleteResponseSchema>;

export const QueueDetailResponseSchema = z.strictObject({
  queue: QueueSchema,
  metrics: z.record(z.string(), z.unknown()).nullable().optional(),
});

export type QueueDetailResponse = z.output<typeof QueueDetailResponseSchema>;

export const WorkflowDetailResponseSchema = z.strictObject({
  definition: WorkflowSchema,
  referrerCount: z.number(),
});

export type WorkflowDetailResponse = z.output<typeof WorkflowDetailResponseSchema>;

export const WorkflowVersionSchema = z.object({
  target: z.object({
    versionId: z.string(),
    className: z.string(),
    deploymentId: z.string(),
    workerId: z.string(),
    definitionId: z.string(),
  }).passthrough(),
  versionNumber: z.number(),
  state: z.string(),
  createdAtMs: z.number(),
  rejectionCode: z.string().nullable().optional(),
});

export type WorkflowVersion = z.output<typeof WorkflowVersionSchema>;

export const WorkflowVersionsResponseSchema = z.array(WorkflowVersionSchema);

export const WorkflowInstanceSchema = z.object({
  id: z.string(),
  externalInstanceId: z.string(),
  versionId: z.string(),
  deploymentId: z.string(),
  className: z.string(),
  generation: z.number(),
  status: z.string(),
  completedStepCount: z.number().optional(),
  stepCount: z.number().optional(),
  stateBytes: z.number().optional(),
  createdAtMs: z.number(),
  terminalAtMs: z.number().nullable().optional(),
  errorCode: z.string().nullable().optional(),
  capabilityVersion: z.number().optional(),
  durable: z.record(z.string(), z.unknown()).optional(),
});

export type WorkflowInstance = z.output<typeof WorkflowInstanceSchema>;

export const WorkflowInstancesResponseSchema = z.array(WorkflowInstanceSchema);

export const WorkflowStepSchema = z.object({
  instanceId: z.string(),
  ordinal: z.number(),
  name: z.string(),
  nameCount: z.number(),
  state: z.string(),
  outputBytes: z.number(),
  errorCode: z.string().nullable().optional(),
  kind: z.string(),
  attempt: z.number(),
  attemptDeadlineAtMs: z.number().nullable().optional(),
  dueAtMs: z.number().nullable().optional(),
  batchFirstOrdinal: z.number().nullable().optional(),
  batchSize: z.number().nullable().optional(),
});

export type WorkflowStep = z.output<typeof WorkflowStepSchema>;

export const WorkflowStepsResponseSchema = z.array(WorkflowStepSchema);

export const WorkflowMutationResponseSchema = z.strictObject({ ok: z.literal(true) });

/** The workflow reconcile endpoint serializes Rust's unit value as JSON null. */
export const WorkflowReconcileResponseSchema = z.null();

export const CacheGcResponseSchema = z.strictObject({ deleted: z.number() });

export const CachePurgeResponseSchema = z.strictObject({
  success: z.literal(true),
  deleted: z.number(),
});

export const SchedulerSummarySchema = z.record(z.string(), z.unknown());
export type SchedulerSummary = z.output<typeof SchedulerSummarySchema>;

export const CacheSummarySchema = z.record(z.string(), z.unknown());
export type CacheSummary = z.output<typeof CacheSummarySchema>;

export const ImagesCapacitySchema = z.record(z.string(), z.unknown());
export type ImagesCapacity = z.output<typeof ImagesCapacitySchema>;

export const SchedulerRepairResponseSchema = z.strictObject({
  repaired: z.number(),
  alarmRepaired: z.number(),
  productRepaired: z.number(),
});

export type SchedulerRepairResponse = z.output<typeof SchedulerRepairResponseSchema>;
