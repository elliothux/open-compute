import { z } from "zod";
import {
  AccountIdSchema,
  DeploymentIdSchema,
  DeploymentUploadIdSchema,
  DurableObjectIdSchema,
  PageCursorSchema,
  QueueConsumerIdSchema,
  QueueIdSchema,
  ResourceIdSchema,
  RouteIdSchema,
  Sha256DigestSchema,
  WorkerIdSchema,
  WorkflowIdSchema,
} from "./ids.js";
import { D1MigrationInputSchema } from "./storage.js";

export const AccountScopeSchema = z.strictObject({
  accountId: AccountIdSchema,
});

export const ResourceScopeSchema = z.strictObject({
  accountId: AccountIdSchema,
  resourceId: ResourceIdSchema,
});

export const KvNamespaceScopeSchema = z.strictObject({
  accountId: AccountIdSchema,
  namespaceId: ResourceIdSchema,
});

export const D1DatabaseScopeSchema = z.strictObject({
  accountId: AccountIdSchema,
  databaseId: ResourceIdSchema,
});

export const R2BucketScopeSchema = z.strictObject({
  accountId: AccountIdSchema,
  bucketId: ResourceIdSchema,
});

export const DoNamespaceScopeSchema = z.strictObject({
  accountId: AccountIdSchema,
  namespaceId: ResourceIdSchema,
});

export const DoObjectScopeSchema = z.strictObject({
  accountId: AccountIdSchema,
  namespaceId: ResourceIdSchema,
  objectId: DurableObjectIdSchema,
});

export const ListPageQuerySchema = z.strictObject({
  cursor: PageCursorSchema.optional(),
  limit: z.number().int().min(1).max(1000).optional(),
});

export const CatalogSortSchema = z.enum(["name", "createdAt", "updatedAt"]);
export const CatalogDirectionSchema = z.enum(["asc", "desc"]);
export const CatalogStatusSchema = z.enum(["creating", "ready", "deleting"]);

export const CatalogListParamsSchema = AccountScopeSchema.extend({
  search: z.string().min(1).max(128).optional(),
  status: CatalogStatusSchema.optional(),
  sort: CatalogSortSchema.optional(),
  direction: CatalogDirectionSchema.optional(),
  cursor: PageCursorSchema.optional(),
  limit: z.number().int().min(1).max(1000).optional(),
});

export const WorkerCatalogListParamsSchema = CatalogListParamsSchema.extend({
  deployed: z.boolean().optional(),
});

export const R2ListObjectsParamsSchema = R2BucketScopeSchema.extend({
  prefix: z.string().optional(),
  cursor: PageCursorSchema.optional(),
  limit: z.number().int().min(1).max(1000).optional(),
});

export const CreateKvNamespaceParamsSchema = AccountScopeSchema.extend({
  name: z.string().min(1).max(64),
  idempotencyKey: z.string().min(1),
});

export const KvNamespaceScopeParamsSchema = KvNamespaceScopeSchema;

export const RenameKvNamespaceParamsSchema = KvNamespaceScopeSchema.extend({
  name: z.string().min(1).max(64),
});

export const DeleteKvNamespaceParamsSchema = KvNamespaceScopeSchema.extend({
  idempotencyKey: z.string().min(1),
});

export const CreateKvBackupParamsSchema = KvNamespaceScopeSchema.extend({
  idempotencyKey: z.string().min(1),
});

export const RestoreKvNamespaceParamsSchema = AccountScopeSchema.extend({
  backupId: z.string().min(1),
  newName: z.string().min(1).max(64),
  idempotencyKey: z.string().min(1),
});

export const DeleteKvBackupParamsSchema = AccountScopeSchema.extend({
  backupId: z.string().min(1),
  idempotencyKey: z.string().min(1),
});

export const CreateD1DatabaseParamsSchema = AccountScopeSchema.extend({
  name: z.string().min(1).max(64),
  idempotencyKey: z.string().min(1),
});

export const D1DatabaseScopeParamsSchema = D1DatabaseScopeSchema;

export const RenameD1DatabaseParamsSchema = D1DatabaseScopeSchema.extend({
  name: z.string().min(1).max(64),
});

export const DeleteD1DatabaseParamsSchema = D1DatabaseScopeSchema.extend({
  idempotencyKey: z.string().min(1),
});

export const CreateR2BucketParamsSchema = AccountScopeSchema.extend({
  name: z.string().min(1).max(64),
  idempotencyKey: z.string().min(1),
});

export const R2BucketScopeParamsSchema = R2BucketScopeSchema;

export const RenameR2BucketParamsSchema = R2BucketScopeSchema.extend({
  name: z.string().min(1).max(64),
});

export const DeleteR2BucketParamsSchema = R2BucketScopeSchema.extend({
  idempotencyKey: z.string().min(1),
  force: z.boolean().optional(),
});

export const DoListObjectsParamsSchema = DoNamespaceScopeSchema.extend({
  search: z.string().min(1).optional(),
  cursor: PageCursorSchema.optional(),
  limit: z.number().int().min(1).max(1000).optional(),
});

export const CreateWorkerParamsSchema = AccountScopeSchema.extend({
  name: z.string().min(1).max(64),
  idempotencyKey: z.string().min(1),
});

export const CreateDoNamespaceParamsSchema = AccountScopeSchema.extend({
  name: z.string().min(1).max(64),
  workerId: WorkerIdSchema,
  className: z.string().min(1).max(256),
  idempotencyKey: z.string().min(1),
});

export const DoNamespaceScopeParamsSchema = DoNamespaceScopeSchema;

export const RenameDoNamespaceParamsSchema = DoNamespaceScopeSchema.extend({
  name: z.string().min(1).max(64),
});

export const DeleteDoNamespaceParamsSchema = DoNamespaceScopeSchema.extend({
  idempotencyKey: z.string().min(1),
  force: z.boolean().optional(),
});

export const QueuesListParamsSchema = CatalogListParamsSchema;

export const CreateQueueParamsSchema = AccountScopeSchema.extend({
  name: z.string().min(1).max(64),
  idempotencyKey: z.string().min(1),
  deliveryDelaySeconds: z.number().int().nonnegative().optional(),
  retentionSeconds: z.number().int().positive().optional(),
  maxBacklogBytes: z.number().int().positive().optional(),
});

export const QueueScopeSchema = z.strictObject({
  accountId: AccountIdSchema,
  queueId: QueueIdSchema,
});

export const QueueScopeParamsSchema = QueueScopeSchema;

export const RenameQueueParamsSchema = QueueScopeSchema.extend({
  name: z.string().min(1).max(64),
  expectedConfigGeneration: z.number().int().positive(),
  idempotencyKey: z.string().min(1),
});

export const UpdateQueueConfigParamsSchema = QueueScopeSchema.extend({
  expectedConfigGeneration: z.number().int().positive(),
  idempotencyKey: z.string().min(1),
  deliveryDelaySeconds: z.number().int().nonnegative().optional(),
  retentionSeconds: z.number().int().positive().optional(),
  maxBacklogBytes: z.number().int().positive().optional(),
}).refine(
  value => value.deliveryDelaySeconds !== undefined
    || value.retentionSeconds !== undefined
    || value.maxBacklogBytes !== undefined,
  { message: "at least one queue configuration field is required" },
);

export const DeleteQueueParamsSchema = QueueScopeSchema.extend({
  idempotencyKey: z.string().min(1),
  expectedLifecycleGeneration: z.number().int().positive(),
  force: z.boolean().optional(),
});

export const CreateWorkflowParamsSchema = AccountScopeSchema.extend({
  name: z.string().min(1).max(64),
});

export const WorkflowsListParamsSchema = CatalogListParamsSchema;

export const WorkflowScopeSchema = z.strictObject({
  accountId: AccountIdSchema,
  workflowId: WorkflowIdSchema,
});

export const WorkflowScopeParamsSchema = WorkflowScopeSchema;

export const RenameWorkflowParamsSchema = WorkflowScopeSchema.extend({
  name: z.string().min(1).max(64),
});

export const CreateWorkflowVersionParamsSchema = WorkflowScopeSchema.extend({
  deploymentId: DeploymentIdSchema,
  className: z.string().min(1).max(256),
});

export const WorkflowInstanceScopeParamsSchema = WorkflowScopeSchema.extend({
  instanceId: z.string().min(1),
});

export const WorkflowVersionsListParamsSchema = WorkflowScopeSchema.extend({
  after: z.number().int().nonnegative().optional(),
  limit: z.number().int().min(1).max(1000).optional(),
});

export const WorkflowInstancesListParamsSchema = WorkflowScopeSchema.extend({
  after: z.string().min(1).optional(),
  limit: z.number().int().min(1).max(1000).optional(),
});

export const WorkflowStepsParamsSchema = WorkflowInstanceScopeParamsSchema.extend({
  after: z.number().int().nonnegative().optional(),
  limit: z.number().int().min(1).max(1000).optional(),
});

export const WorkflowEventParamsSchema = WorkflowInstanceScopeParamsSchema.extend({
  eventType: z.string().min(1).max(100),
  payloadBase64: z.string().min(1),
});

export const WorkerScopeParamsSchema = z.strictObject({
  accountId: AccountIdSchema,
  workerId: WorkerIdSchema,
});

export const DeploymentScopeParamsSchema = WorkerScopeParamsSchema.extend({
  deploymentId: DeploymentIdSchema,
});

export const DeleteDeploymentParamsSchema = DeploymentScopeParamsSchema.extend({
  idempotencyKey: z.string().min(1),
});

export const PromoteWorkerParamsSchema = WorkerScopeParamsSchema.extend({
  targetDeploymentId: DeploymentIdSchema,
  expectedActiveDeploymentId: DeploymentIdSchema.nullable(),
  idempotencyKey: z.string().min(1),
});

export const RollbackWorkerParamsSchema = PromoteWorkerParamsSchema;

export const DeploymentUploadScopeParamsSchema = WorkerScopeParamsSchema.extend({
  uploadId: DeploymentUploadIdSchema,
});

export const PutDeploymentUploadObjectParamsSchema = DeploymentUploadScopeParamsSchema.extend({
  sha256: Sha256DigestSchema,
});

export const FinalizeDeploymentUploadParamsSchema = DeploymentUploadScopeParamsSchema.extend({
  idempotencyKey: z.string().min(1),
});

export const KvListKeysParamsSchema = KvNamespaceScopeSchema.extend({
  prefix: z.string().optional(),
  cursor: PageCursorSchema.optional(),
  limit: z.number().int().min(1).max(1000).optional(),
});

export const KvKeyScopeParamsSchema = KvNamespaceScopeSchema.extend({
  key: z.string().min(1),
});

export const KvPutValueParamsSchema = KvKeyScopeParamsSchema.extend({
  value: z.string(),
  metadata: z.unknown().optional(),
  expiration: z.number().int().nonnegative().optional(),
  expirationTtl: z.number().int().min(60).optional(),
  idempotencyKey: z.string().min(1),
}).refine(params => params.expiration === undefined || params.expirationTtl === undefined, {
  message: "expiration and expirationTtl are mutually exclusive",
});

export const KvDeleteValueParamsSchema = KvKeyScopeParamsSchema.extend({
  idempotencyKey: z.string().min(1),
});

export const R2ObjectScopeParamsSchema = R2BucketScopeSchema.extend({
  key: z.string().min(1),
});

export const D1QueryParamsSchema = D1DatabaseScopeSchema.extend({
  sql: z.string().min(1).max(65_536),
});

export const D1ApplyMigrationsParamsSchema = D1DatabaseScopeSchema.extend({
  idempotencyKey: z.string().min(1),
  migrations: z.array(D1MigrationInputSchema).min(1).max(64),
});

export const CreateD1BackupParamsSchema = D1DatabaseScopeSchema.extend({
  idempotencyKey: z.string().min(1),
});

export const RestoreD1DatabaseParamsSchema = AccountScopeSchema.extend({
  backupId: z.string().min(1),
  newName: z.string().min(1).max(64),
  idempotencyKey: z.string().min(1),
});

export const DeleteWorkerParamsSchema = WorkerScopeParamsSchema.extend({
  idempotencyKey: z.string().min(1),
});

export const CreateDeploymentParamsSchema = WorkerScopeParamsSchema.extend({
  idempotencyKey: z.string().min(1),
  metadata: z.string().min(1).max(1024 * 1024),
});

export const CreateDeploymentUploadParamsSchema = WorkerScopeParamsSchema.extend({
  idempotencyKey: z.string().min(1),
});

export const CreateRouteParamsSchema = WorkerScopeParamsSchema.extend({
  hostname: z.string().min(1).max(253),
  pathPrefix: z.string().min(1).max(1024),
  entrypoint: z.string().min(1).max(256).optional(),
  idempotencyKey: z.string().min(1),
});

export const DeleteRouteParamsSchema = WorkerScopeParamsSchema.extend({
  routeId: RouteIdSchema,
  idempotencyKey: z.string().min(1),
});

export const SchedulerMutationParamsSchema = z.strictObject({
  kind: z.enum(["queue", "cron", "workflow"]).optional(),
});

export const QueueConsumerMutationParamsSchema = z.strictObject({
  consumerId: QueueConsumerIdSchema,
  consumerGeneration: z.number().int().positive(),
});
