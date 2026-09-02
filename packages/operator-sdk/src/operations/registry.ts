import type { AccountId, DeploymentId, DeploymentUploadId, DurableObjectId, PageCursor, QueueConsumerId, QueueId, ResourceId, RouteId, Sha256Digest, WorkerId, WorkflowId } from "../schemas/ids.js";
import {
  AccountScopeSchema,
  CatalogListParamsSchema,
  WorkerCatalogListParamsSchema,
  CreateDeploymentParamsSchema,
  CreateDeploymentUploadParamsSchema,
  CreateDoNamespaceParamsSchema,
  CreateD1BackupParamsSchema,
  RestoreD1DatabaseParamsSchema,
  CreateD1DatabaseParamsSchema,
  CreateKvBackupParamsSchema,
  CreateKvNamespaceParamsSchema,
  DeleteKvBackupParamsSchema,
  RestoreKvNamespaceParamsSchema,
  CreateQueueParamsSchema,
  CreateR2BucketParamsSchema,
  CreateRouteParamsSchema,
  CreateWorkerParamsSchema,
  CreateWorkflowParamsSchema,
  CreateWorkflowVersionParamsSchema,
  DeleteD1DatabaseParamsSchema,
  DeleteDoNamespaceParamsSchema,
  DeleteKvNamespaceParamsSchema,
  DeleteQueueParamsSchema,
  DeleteR2BucketParamsSchema,
  DeleteRouteParamsSchema,
  DeleteWorkerParamsSchema,
  DeleteDeploymentParamsSchema,
  DeploymentScopeParamsSchema,
  DeploymentUploadScopeParamsSchema,
  D1ApplyMigrationsParamsSchema,
  D1DatabaseScopeParamsSchema,
  D1QueryParamsSchema,
  DoListObjectsParamsSchema,
  DoNamespaceScopeParamsSchema,
  DoObjectScopeSchema,
  KvDeleteValueParamsSchema,
  KvNamespaceScopeParamsSchema,
  FinalizeDeploymentUploadParamsSchema,
  KvKeyScopeParamsSchema,
  KvListKeysParamsSchema,
  KvPutValueParamsSchema,
  PromoteWorkerParamsSchema,
  PutDeploymentUploadObjectParamsSchema,
  QueuesListParamsSchema,
  QueueConsumerMutationParamsSchema,
  R2ObjectScopeParamsSchema,
  RenameD1DatabaseParamsSchema,
  RenameDoNamespaceParamsSchema,
  RenameKvNamespaceParamsSchema,
  RenameQueueParamsSchema,
  RenameR2BucketParamsSchema,
  RenameWorkflowParamsSchema,
  UpdateQueueConfigParamsSchema,
  RollbackWorkerParamsSchema,
  R2BucketScopeParamsSchema,
  R2ListObjectsParamsSchema,
  QueueScopeParamsSchema,
  SchedulerMutationParamsSchema,
  WorkerScopeParamsSchema,
  WorkflowsListParamsSchema,
  WorkflowScopeParamsSchema,
  WorkflowEventParamsSchema,
  WorkflowInstanceScopeParamsSchema,
  WorkflowInstancesListParamsSchema,
  WorkflowStepsParamsSchema,
  WorkflowVersionsListParamsSchema,
} from "../schemas/inputs.js";
import {
  AccountResponseSchema,
  MetaResponseSchema,
  SystemStatusResponseSchema,
} from "../schemas/system.js";
import { EmptyResponseSchema } from "../schemas/common.js";
import {
  CreateDeploymentResponseSchema,
  CreateWorkerResponseSchema,
  CreateRouteResponseSchema,
  DeleteRouteResponseSchema,
  DeleteWorkerResponseSchema,
  DeploymentUploadSessionSchema,
  DeploymentResponseSchema,
  DeleteDeploymentResponseSchema,
  DeploymentsListResponseSchema,
  RoutesListResponseSchema,
  WorkerDetailResponseSchema,
  WorkersListResponseSchema,
} from "../schemas/workers.js";
import {
  CreateResourceResultSchema,
  D1DatabaseDetailResponseSchema,
  D1DatabaseResourceResponseSchema,
  D1DatabasesResponseSchema,
  D1ApplyMigrationsResponseSchema,
  D1BackupResponseSchema,
  D1BackupsResponseSchema,
  D1MigrationsResponseSchema,
  D1QueryResponseSchema,
  D1TablesResponseSchema,
  DeleteResourceResponseSchema,
  KvBackupResponseSchema,
  KvBackupsResponseSchema,
  KvDeleteNamespaceResponseSchema,
  KvKeysResponseSchema,
  KvMutationResponseSchema,
  KvNamespaceResponseSchema,
  KvRenameNamespaceResponseSchema,
  KvNamespacesResponseSchema,
  KvValueResponseSchema,
  R2BucketResponseSchema,
  R2BucketsResponseSchema,
  R2ObjectMutationResponseSchema,
  R2ObjectSchema,
  R2ObjectsResponseSchema,
  ResourceRecordSchema,
} from "../schemas/storage.js";
import {
  CacheSummarySchema,
  CacheGcResponseSchema,
  CachePurgeResponseSchema,
  DoObjectSchema,
  DoNamespaceResponseSchema,
  DoNamespacesResponseSchema,
  DoObjectsResponseSchema,
  ImagesCapacitySchema,
  QueueDeleteResponseSchema,
  QueueDetailResponseSchema,
  QueueResponseSchema,
  QueuesResponseSchema,
  SchedulerRepairResponseSchema,
  SchedulerSummarySchema,
  WorkflowDetailResponseSchema,
  WorkflowInstancesResponseSchema,
  WorkflowInstanceSchema,
  WorkflowMutationResponseSchema,
  WorkflowReconcileResponseSchema,
  WorkflowSchema,
  WorkflowStepsResponseSchema,
  WorkflowVersionSchema,
  WorkflowVersionsResponseSchema,
  WorkflowsResponseSchema,
} from "../schemas/platform.js";
import type { BinaryOperationDef, BodyJsonOperationDef, JsonOperationDef } from "./types.js";

type WorkflowInstanceMutationParams = {
  accountId: AccountId;
  workflowId: WorkflowId;
  instanceId: string;
};

function workflowInstanceMutation(
  action: "pause" | "resume" | "terminate" | "restart",
): JsonOperationDef<WorkflowInstanceMutationParams, unknown> {
  return {
    id: `workflows.${action}Instance`,
    method: "POST",
    path: params =>
      `accounts/${params.accountId}/workflows/${params.workflowId}/instances/${params.instanceId}/${action}`,
    successSchema: WorkflowMutationResponseSchema,
    paramsSchema: WorkflowInstanceScopeParamsSchema,
  };
}

export const operatorOperations = {
  system: {
    meta: {
      id: "system.meta",
      method: "GET",
      path: () => "meta",
      successSchema: MetaResponseSchema,
    } satisfies JsonOperationDef<Record<string, never>, unknown>,
    account: {
      id: "system.account",
      method: "GET",
      path: () => "account",
      successSchema: AccountResponseSchema,
    } satisfies JsonOperationDef<Record<string, never>, unknown>,
    status: {
      id: "system.status",
      method: "GET",
      path: () => "system/status",
      successSchema: SystemStatusResponseSchema,
    } satisfies JsonOperationDef<Record<string, never>, unknown>,
  },
  workers: {
    list: {
      id: "workers.list",
      method: "GET",
      path: (params: { accountId: AccountId }) => `accounts/${params.accountId}/workers`,
      successSchema: WorkersListResponseSchema,
      paramsSchema: WorkerCatalogListParamsSchema,
    } satisfies JsonOperationDef<
      {
        accountId: AccountId;
        search?: string | undefined;
        deployed?: boolean | undefined;
        sort?: "name" | "createdAt" | "updatedAt" | undefined;
        direction?: "asc" | "desc" | undefined;
        cursor?: PageCursor | undefined;
        limit?: number | undefined;
      },
      unknown
    >,
    create: {
      id: "workers.create",
      method: "POST",
      path: (params: { accountId: AccountId }) => `accounts/${params.accountId}/workers`,
      successSchema: CreateWorkerResponseSchema,
      paramsSchema: CreateWorkerParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<{ accountId: AccountId; name: string; idempotencyKey: string }, unknown>,
    get: {
      id: "workers.get",
      method: "GET",
      path: (params: { accountId: AccountId; workerId: WorkerId }) =>
        `accounts/${params.accountId}/workers/${params.workerId}`,
      successSchema: WorkerDetailResponseSchema,
      paramsSchema: WorkerScopeParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; workerId: WorkerId }, unknown>,
    delete: {
      id: "workers.delete",
      method: "DELETE",
      path: (params: { accountId: AccountId; workerId: WorkerId }) =>
        `accounts/${params.accountId}/workers/${params.workerId}`,
      successSchema: DeleteWorkerResponseSchema,
      paramsSchema: DeleteWorkerParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<{ accountId: AccountId; workerId: WorkerId; idempotencyKey: string }, unknown>,
    listDeployments: {
      id: "workers.listDeployments",
      method: "GET",
      path: (params: { accountId: AccountId; workerId: WorkerId }) =>
        `accounts/${params.accountId}/workers/${params.workerId}/deployments`,
      successSchema: DeploymentsListResponseSchema,
      paramsSchema: WorkerScopeParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; workerId: WorkerId }, unknown>,
    getDeployment: {
      id: "workers.getDeployment",
      method: "GET",
      path: (params: { accountId: AccountId; workerId: WorkerId; deploymentId: DeploymentId }) =>
        `accounts/${params.accountId}/workers/${params.workerId}/deployments/${params.deploymentId}`,
      successSchema: DeploymentResponseSchema,
      paramsSchema: DeploymentScopeParamsSchema,
    } satisfies JsonOperationDef<
      { accountId: AccountId; workerId: WorkerId; deploymentId: DeploymentId },
      unknown
    >,
    deleteDeployment: {
      id: "workers.deleteDeployment",
      method: "DELETE",
      path: (params: { accountId: AccountId; workerId: WorkerId; deploymentId: DeploymentId }) =>
        `accounts/${params.accountId}/workers/${params.workerId}/deployments/${params.deploymentId}`,
      successSchema: DeleteDeploymentResponseSchema,
      paramsSchema: DeleteDeploymentParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<
      {
        accountId: AccountId;
        workerId: WorkerId;
        deploymentId: DeploymentId;
        idempotencyKey: string;
      },
      unknown
    >,
    listRoutes: {
      id: "workers.listRoutes",
      method: "GET",
      path: (params: { accountId: AccountId; workerId: WorkerId }) =>
        `accounts/${params.accountId}/workers/${params.workerId}/routes`,
      successSchema: RoutesListResponseSchema,
      paramsSchema: WorkerScopeParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; workerId: WorkerId }, unknown>,
    createRoute: {
      id: "workers.createRoute",
      method: "POST",
      path: (params: { accountId: AccountId; workerId: WorkerId }) =>
        `accounts/${params.accountId}/workers/${params.workerId}/routes`,
      successSchema: CreateRouteResponseSchema,
      paramsSchema: CreateRouteParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<
      {
        accountId: AccountId;
        workerId: WorkerId;
        hostname: string;
        pathPrefix: string;
        entrypoint?: string | undefined;
        idempotencyKey: string;
      },
      unknown
    >,
    deleteRoute: {
      id: "workers.deleteRoute",
      method: "DELETE",
      path: (params: { accountId: AccountId; workerId: WorkerId; routeId: RouteId }) =>
        `accounts/${params.accountId}/workers/${params.workerId}/routes/${params.routeId}`,
      successSchema: DeleteRouteResponseSchema,
      paramsSchema: DeleteRouteParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<
      { accountId: AccountId; workerId: WorkerId; routeId: RouteId; idempotencyKey: string },
      unknown
    >,
    promote: {
      id: "workers.promote",
      method: "POST",
      path: (params: { accountId: AccountId; workerId: WorkerId }) =>
        `accounts/${params.accountId}/workers/${params.workerId}/promotions`,
      successSchema: WorkerDetailResponseSchema,
      paramsSchema: PromoteWorkerParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<
      {
        accountId: AccountId;
        workerId: WorkerId;
        targetDeploymentId: DeploymentId;
        expectedActiveDeploymentId: DeploymentId | null;
        idempotencyKey: string;
      },
      unknown
    >,
    rollback: {
      id: "workers.rollback",
      method: "POST",
      path: (params: { accountId: AccountId; workerId: WorkerId }) =>
        `accounts/${params.accountId}/workers/${params.workerId}/rollbacks`,
      successSchema: WorkerDetailResponseSchema,
      paramsSchema: RollbackWorkerParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<
      {
        accountId: AccountId;
        workerId: WorkerId;
        targetDeploymentId: DeploymentId;
        expectedActiveDeploymentId: DeploymentId | null;
        idempotencyKey: string;
      },
      unknown
    >,
    createDeployment: {
      id: "workers.createDeployment",
      method: "POST",
      path: (params: { accountId: AccountId; workerId: WorkerId }) =>
        `accounts/${params.accountId}/workers/${params.workerId}/deployments`,
      successSchema: CreateDeploymentResponseSchema,
      paramsSchema: CreateDeploymentParamsSchema,
      idempotent: true,
    } satisfies BodyJsonOperationDef<
      { accountId: AccountId; workerId: WorkerId; idempotencyKey: string; metadata: string },
      unknown
    >,
    createDeploymentUpload: {
      id: "workers.createDeploymentUpload",
      method: "POST",
      path: (params: { accountId: AccountId; workerId: WorkerId }) =>
        `accounts/${params.accountId}/workers/${params.workerId}/deployment-uploads`,
      successSchema: DeploymentUploadSessionSchema,
      paramsSchema: CreateDeploymentUploadParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<{ accountId: AccountId; workerId: WorkerId; idempotencyKey: string }, unknown>,
    getDeploymentUpload: {
      id: "workers.getDeploymentUpload",
      method: "GET",
      path: (params: { accountId: AccountId; workerId: WorkerId; uploadId: DeploymentUploadId }) =>
        `accounts/${params.accountId}/workers/${params.workerId}/deployment-uploads/${params.uploadId}`,
      successSchema: DeploymentUploadSessionSchema,
      paramsSchema: DeploymentUploadScopeParamsSchema,
    } satisfies JsonOperationDef<
      { accountId: AccountId; workerId: WorkerId; uploadId: DeploymentUploadId },
      unknown
    >,
    abortDeploymentUpload: {
      id: "workers.abortDeploymentUpload",
      method: "DELETE",
      path: (params: { accountId: AccountId; workerId: WorkerId; uploadId: DeploymentUploadId }) =>
        `accounts/${params.accountId}/workers/${params.workerId}/deployment-uploads/${params.uploadId}`,
      successSchema: DeploymentUploadSessionSchema,
      paramsSchema: DeploymentUploadScopeParamsSchema,
    } satisfies JsonOperationDef<
      { accountId: AccountId; workerId: WorkerId; uploadId: DeploymentUploadId },
      unknown
    >,
    putDeploymentUploadObject: {
      id: "workers.putDeploymentUploadObject",
      method: "PUT",
      path: (params: {
        accountId: AccountId;
        workerId: WorkerId;
        uploadId: DeploymentUploadId;
        sha256: Sha256Digest;
      }) =>
        `accounts/${params.accountId}/workers/${params.workerId}/deployment-uploads/${params.uploadId}/objects/${params.sha256}`,
      successSchema: DeploymentUploadSessionSchema,
      paramsSchema: PutDeploymentUploadObjectParamsSchema,
      idempotent: true,
    } satisfies BodyJsonOperationDef<
      { accountId: AccountId; workerId: WorkerId; uploadId: DeploymentUploadId; sha256: Sha256Digest },
      unknown
    >,
    finalizeDeploymentUpload: {
      id: "workers.finalizeDeploymentUpload",
      method: "POST",
      path: (params: {
        accountId: AccountId;
        workerId: WorkerId;
        uploadId: DeploymentUploadId;
      }) =>
        `accounts/${params.accountId}/workers/${params.workerId}/deployment-uploads/${params.uploadId}/finalize`,
      successSchema: CreateDeploymentResponseSchema,
      paramsSchema: FinalizeDeploymentUploadParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<
      { accountId: AccountId; workerId: WorkerId; uploadId: DeploymentUploadId; idempotencyKey: string },
      unknown
    >,
  },
  kv: {
    listNamespaces: {
      id: "kv.listNamespaces",
      method: "GET",
      path: (params: { accountId: AccountId }) => `accounts/${params.accountId}/kv/namespaces`,
      successSchema: KvNamespacesResponseSchema,
      paramsSchema: CatalogListParamsSchema,
    } satisfies JsonOperationDef<
      {
        accountId: AccountId;
        search?: string | undefined;
        status?: string | undefined;
        sort?: "name" | "createdAt" | "updatedAt" | undefined;
        direction?: "asc" | "desc" | undefined;
        cursor?: PageCursor | undefined;
        limit?: number | undefined;
      },
      unknown
    >,
    createNamespace: {
      id: "kv.createNamespace",
      method: "POST",
      path: (params: { accountId: AccountId }) => `accounts/${params.accountId}/kv/namespaces`,
      successSchema: CreateResourceResultSchema,
      paramsSchema: CreateKvNamespaceParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<{ accountId: AccountId; name: string; idempotencyKey: string }, unknown>,
    getNamespace: {
      id: "kv.getNamespace",
      method: "GET",
      path: (params: { accountId: AccountId; namespaceId: ResourceId }) =>
        `accounts/${params.accountId}/kv/namespaces/${params.namespaceId}`,
      successSchema: KvNamespaceResponseSchema,
      paramsSchema: KvNamespaceScopeParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; namespaceId: ResourceId }, unknown>,
    renameNamespace: {
      id: "kv.renameNamespace",
      method: "PATCH",
      path: (params: { accountId: AccountId; namespaceId: ResourceId }) =>
        `accounts/${params.accountId}/kv/namespaces/${params.namespaceId}`,
      successSchema: KvRenameNamespaceResponseSchema,
      paramsSchema: RenameKvNamespaceParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; namespaceId: ResourceId; name: string }, unknown>,
    deleteNamespace: {
      id: "kv.deleteNamespace",
      method: "DELETE",
      path: (params: { accountId: AccountId; namespaceId: ResourceId }) =>
        `accounts/${params.accountId}/kv/namespaces/${params.namespaceId}`,
      successSchema: KvDeleteNamespaceResponseSchema,
      paramsSchema: DeleteKvNamespaceParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<{ accountId: AccountId; namespaceId: ResourceId; idempotencyKey: string }, unknown>,
    listKeys: {
      id: "kv.listKeys",
      method: "GET",
      path: (params: { accountId: AccountId; namespaceId: ResourceId }) =>
        `accounts/${params.accountId}/kv/namespaces/${params.namespaceId}/keys`,
      successSchema: KvKeysResponseSchema,
      paramsSchema: KvListKeysParamsSchema,
    } satisfies JsonOperationDef<
      {
        accountId: AccountId;
        namespaceId: ResourceId;
        prefix?: string | undefined;
        cursor?: PageCursor | undefined;
        limit?: number | undefined;
      },
      unknown
    >,
    getValue: {
      id: "kv.getValue",
      method: "GET",
      path: (params: { accountId: AccountId; namespaceId: ResourceId; key: string }) =>
        `accounts/${params.accountId}/kv/namespaces/${params.namespaceId}/values/${encodeURIComponent(params.key)}`,
      successSchema: KvValueResponseSchema,
      paramsSchema: KvKeyScopeParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; namespaceId: ResourceId; key: string }, unknown>,
    putValue: {
      id: "kv.putValue",
      method: "PUT",
      path: (params: { accountId: AccountId; namespaceId: ResourceId; key: string }) =>
        `accounts/${params.accountId}/kv/namespaces/${params.namespaceId}/values/${encodeURIComponent(params.key)}`,
      successSchema: KvMutationResponseSchema,
      paramsSchema: KvPutValueParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<{ accountId: AccountId; namespaceId: ResourceId; key: string }, unknown>,
    deleteValue: {
      id: "kv.deleteValue",
      method: "DELETE",
      path: (params: { accountId: AccountId; namespaceId: ResourceId; key: string }) =>
        `accounts/${params.accountId}/kv/namespaces/${params.namespaceId}/values/${encodeURIComponent(params.key)}`,
      successSchema: KvMutationResponseSchema,
      paramsSchema: KvDeleteValueParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<{ accountId: AccountId; namespaceId: ResourceId; key: string }, unknown>,
    listBackups: {
      id: "kv.listBackups",
      method: "GET",
      path: (params: { accountId: AccountId }) => `accounts/${params.accountId}/kv/backups`,
      successSchema: KvBackupsResponseSchema,
      paramsSchema: AccountScopeSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId }, unknown>,
    createBackup: {
      id: "kv.createBackup",
      method: "POST",
      path: (params: { accountId: AccountId; namespaceId: ResourceId }) =>
        `accounts/${params.accountId}/kv/namespaces/${params.namespaceId}/backups`,
      successSchema: KvBackupResponseSchema,
      paramsSchema: CreateKvBackupParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<{ accountId: AccountId; namespaceId: ResourceId; idempotencyKey: string }, unknown>,
    restoreNamespace: {
      id: "kv.restoreNamespace",
      method: "POST",
      path: (params: { accountId: AccountId }) => `accounts/${params.accountId}/kv/namespaces:restore`,
      successSchema: CreateResourceResultSchema,
      paramsSchema: RestoreKvNamespaceParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<
      { accountId: AccountId; backupId: string; newName: string; idempotencyKey: string },
      unknown
    >,
    deleteBackup: {
      id: "kv.deleteBackup",
      method: "DELETE",
      path: (params: { accountId: AccountId; backupId: string }) =>
        `accounts/${params.accountId}/kv/backups/${params.backupId}`,
      successSchema: KvBackupResponseSchema,
      paramsSchema: DeleteKvBackupParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<{ accountId: AccountId; backupId: string; idempotencyKey: string }, unknown>,
  },
  d1: {
    listDatabases: {
      id: "d1.listDatabases",
      method: "GET",
      path: (params: { accountId: AccountId }) => `accounts/${params.accountId}/d1/databases`,
      successSchema: D1DatabasesResponseSchema,
      paramsSchema: CatalogListParamsSchema,
    } satisfies JsonOperationDef<
      {
        accountId: AccountId;
        search?: string | undefined;
        status?: string | undefined;
        sort?: "name" | "createdAt" | "updatedAt" | undefined;
        direction?: "asc" | "desc" | undefined;
        cursor?: PageCursor | undefined;
        limit?: number | undefined;
      },
      unknown
    >,
    createDatabase: {
      id: "d1.createDatabase",
      method: "POST",
      path: (params: { accountId: AccountId }) => `accounts/${params.accountId}/d1/databases`,
      successSchema: CreateResourceResultSchema,
      paramsSchema: CreateD1DatabaseParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<{ accountId: AccountId; name: string; idempotencyKey: string }, unknown>,
    getDatabase: {
      id: "d1.getDatabase",
      method: "GET",
      path: (params: { accountId: AccountId; databaseId: ResourceId }) =>
        `accounts/${params.accountId}/d1/databases/${params.databaseId}`,
      successSchema: D1DatabaseDetailResponseSchema,
      paramsSchema: D1DatabaseScopeParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; databaseId: ResourceId }, unknown>,
    renameDatabase: {
      id: "d1.renameDatabase",
      method: "PATCH",
      path: (params: { accountId: AccountId; databaseId: ResourceId }) =>
        `accounts/${params.accountId}/d1/databases/${params.databaseId}`,
      successSchema: D1DatabaseResourceResponseSchema,
      paramsSchema: RenameD1DatabaseParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; databaseId: ResourceId; name: string }, unknown>,
    deleteDatabase: {
      id: "d1.deleteDatabase",
      method: "DELETE",
      path: (params: { accountId: AccountId; databaseId: ResourceId }) =>
        `accounts/${params.accountId}/d1/databases/${params.databaseId}`,
      successSchema: DeleteResourceResponseSchema,
      paramsSchema: DeleteD1DatabaseParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<{ accountId: AccountId; databaseId: ResourceId; idempotencyKey: string }, unknown>,
    listTables: {
      id: "d1.listTables",
      method: "GET",
      path: (params: { accountId: AccountId; databaseId: ResourceId }) =>
        `accounts/${params.accountId}/d1/databases/${params.databaseId}/tables`,
      successSchema: D1TablesResponseSchema,
      paramsSchema: D1DatabaseScopeParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; databaseId: ResourceId }, unknown>,
    query: {
      id: "d1.query",
      method: "POST",
      path: (params: { accountId: AccountId; databaseId: ResourceId }) =>
        `accounts/${params.accountId}/d1/databases/${params.databaseId}/query`,
      successSchema: D1QueryResponseSchema,
      paramsSchema: D1QueryParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; databaseId: ResourceId; sql: string }, unknown>,
    listMigrations: {
      id: "d1.listMigrations",
      method: "GET",
      path: (params: { accountId: AccountId; databaseId: ResourceId }) =>
        `accounts/${params.accountId}/d1/databases/${params.databaseId}/migrations`,
      successSchema: D1MigrationsResponseSchema,
      paramsSchema: D1DatabaseScopeParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; databaseId: ResourceId }, unknown>,
    applyMigrations: {
      id: "d1.applyMigrations",
      method: "POST",
      path: (params: { accountId: AccountId; databaseId: ResourceId }) =>
        `accounts/${params.accountId}/d1/databases/${params.databaseId}/migrations/apply`,
      successSchema: D1ApplyMigrationsResponseSchema,
      paramsSchema: D1ApplyMigrationsParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<
      {
        accountId: AccountId;
        databaseId: ResourceId;
        idempotencyKey: string;
        migrations: Array<{ id: number; name: string; sha256: Sha256Digest; sql: string }>;
      },
      unknown
    >,
    listBackups: {
      id: "d1.listBackups",
      method: "GET",
      path: (params: { accountId: AccountId; databaseId: ResourceId }) =>
        `accounts/${params.accountId}/d1/databases/${params.databaseId}/backups`,
      successSchema: D1BackupsResponseSchema,
      paramsSchema: D1DatabaseScopeParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; databaseId: ResourceId }, unknown>,
    createBackup: {
      id: "d1.createBackup",
      method: "POST",
      path: (params: { accountId: AccountId; databaseId: ResourceId }) =>
        `accounts/${params.accountId}/d1/databases/${params.databaseId}/backups`,
      successSchema: D1BackupResponseSchema,
      paramsSchema: CreateD1BackupParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<{ accountId: AccountId; databaseId: ResourceId; idempotencyKey: string }, unknown>,
    restoreDatabase: {
      id: "d1.restoreDatabase",
      method: "POST",
      path: (params: { accountId: AccountId }) => `accounts/${params.accountId}/d1/databases:restore`,
      successSchema: CreateResourceResultSchema,
      paramsSchema: RestoreD1DatabaseParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<
      { accountId: AccountId; backupId: string; newName: string; idempotencyKey: string },
      unknown
    >,
  },
  r2: {
    listBuckets: {
      id: "r2.listBuckets",
      method: "GET",
      path: (params: { accountId: AccountId }) => `accounts/${params.accountId}/r2/buckets`,
      successSchema: R2BucketsResponseSchema,
      paramsSchema: CatalogListParamsSchema,
    } satisfies JsonOperationDef<
      {
        accountId: AccountId;
        search?: string | undefined;
        status?: string | undefined;
        sort?: "name" | "createdAt" | "updatedAt" | undefined;
        direction?: "asc" | "desc" | undefined;
        cursor?: PageCursor | undefined;
        limit?: number | undefined;
      },
      unknown
    >,
    createBucket: {
      id: "r2.createBucket",
      method: "POST",
      path: (params: { accountId: AccountId }) => `accounts/${params.accountId}/r2/buckets`,
      successSchema: R2BucketResponseSchema,
      paramsSchema: CreateR2BucketParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<{ accountId: AccountId; name: string; idempotencyKey: string }, unknown>,
    getBucket: {
      id: "r2.getBucket",
      method: "GET",
      path: (params: { accountId: AccountId; bucketId: ResourceId }) =>
        `accounts/${params.accountId}/r2/buckets/${params.bucketId}`,
      successSchema: R2BucketResponseSchema,
      paramsSchema: R2BucketScopeParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; bucketId: ResourceId }, unknown>,
    renameBucket: {
      id: "r2.renameBucket",
      method: "PATCH",
      path: (params: { accountId: AccountId; bucketId: ResourceId }) =>
        `accounts/${params.accountId}/r2/buckets/${params.bucketId}`,
      successSchema: R2BucketResponseSchema,
      paramsSchema: RenameR2BucketParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; bucketId: ResourceId; name: string }, unknown>,
    deleteBucket: {
      id: "r2.deleteBucket",
      method: "DELETE",
      path: (params: { accountId: AccountId; bucketId: ResourceId }) =>
        `accounts/${params.accountId}/r2/buckets/${params.bucketId}`,
      successSchema: DeleteResourceResponseSchema,
      paramsSchema: DeleteR2BucketParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<
      { accountId: AccountId; bucketId: ResourceId; idempotencyKey: string; force?: boolean | undefined },
      unknown
    >,
    listObjects: {
      id: "r2.listObjects",
      method: "GET",
      path: (params: { accountId: AccountId; bucketId: ResourceId }) =>
        `accounts/${params.accountId}/r2/buckets/${params.bucketId}/objects`,
      successSchema: R2ObjectsResponseSchema,
      paramsSchema: R2ListObjectsParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; bucketId: ResourceId }, unknown>,
    headObject: {
      id: "r2.headObject",
      method: "GET",
      path: (params: { accountId: AccountId; bucketId: ResourceId; key: string }) =>
        `accounts/${params.accountId}/r2/buckets/${params.bucketId}/objects/${encodeURIComponent(params.key)}`,
      successSchema: R2ObjectSchema,
      paramsSchema: R2ObjectScopeParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; bucketId: ResourceId; key: string }, unknown>,
    getObject: {
      id: "r2.getObject",
      method: "GET",
      path: (params: { accountId: AccountId; bucketId: ResourceId; key: string }) =>
        `accounts/${params.accountId}/r2/buckets/${params.bucketId}/objects/${encodeURIComponent(params.key)}`,
      paramsSchema: R2ObjectScopeParamsSchema,
    } satisfies BinaryOperationDef<{ accountId: AccountId; bucketId: ResourceId; key: string }>,
    putObject: {
      id: "r2.putObject",
      method: "PUT",
      path: (params: { accountId: AccountId; bucketId: ResourceId; key: string }) =>
        `accounts/${params.accountId}/r2/buckets/${params.bucketId}/objects/${encodeURIComponent(params.key)}`,
      successSchema: R2ObjectMutationResponseSchema,
      paramsSchema: R2ObjectScopeParamsSchema,
      idempotent: true,
    } satisfies BodyJsonOperationDef<{ accountId: AccountId; bucketId: ResourceId; key: string }, unknown>,
    deleteObject: {
      id: "r2.deleteObject",
      method: "DELETE",
      path: (params: { accountId: AccountId; bucketId: ResourceId; key: string }) =>
        `accounts/${params.accountId}/r2/buckets/${params.bucketId}/objects/${encodeURIComponent(params.key)}`,
      successSchema: R2ObjectMutationResponseSchema,
      paramsSchema: R2ObjectScopeParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<{ accountId: AccountId; bucketId: ResourceId; key: string }, unknown>,
  },
  durableObjects: {
    listNamespaces: {
      id: "durableObjects.listNamespaces",
      method: "GET",
      path: (params: { accountId: AccountId }) =>
        `accounts/${params.accountId}/durable-objects/namespaces`,
      successSchema: DoNamespacesResponseSchema,
      paramsSchema: CatalogListParamsSchema,
    } satisfies JsonOperationDef<
      {
        accountId: AccountId;
        search?: string | undefined;
        status?: string | undefined;
        sort?: "name" | "createdAt" | "updatedAt" | undefined;
        direction?: "asc" | "desc" | undefined;
        cursor?: PageCursor | undefined;
        limit?: number | undefined;
      },
      unknown
    >,
    createNamespace: {
      id: "durableObjects.createNamespace",
      method: "POST",
      path: (params: { accountId: AccountId }) =>
        `accounts/${params.accountId}/durable-objects/namespaces`,
      successSchema: CreateResourceResultSchema,
      paramsSchema: CreateDoNamespaceParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<
      { accountId: AccountId; name: string; workerId: WorkerId; className: string; idempotencyKey: string },
      unknown
    >,
    getNamespace: {
      id: "durableObjects.getNamespace",
      method: "GET",
      path: (params: { accountId: AccountId; namespaceId: ResourceId }) =>
        `accounts/${params.accountId}/durable-objects/namespaces/${params.namespaceId}`,
      successSchema: DoNamespaceResponseSchema,
      paramsSchema: DoNamespaceScopeParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; namespaceId: ResourceId }, unknown>,
    getObject: {
      id: "durableObjects.getObject",
      method: "GET",
      path: (params: { accountId: AccountId; namespaceId: ResourceId; objectId: DurableObjectId }) =>
        `accounts/${params.accountId}/durable-objects/namespaces/${params.namespaceId}/objects/${params.objectId}`,
      successSchema: DoObjectSchema,
      paramsSchema: DoObjectScopeSchema,
    } satisfies JsonOperationDef<
      { accountId: AccountId; namespaceId: ResourceId; objectId: DurableObjectId },
      unknown
    >,
    deleteObject: {
      id: "durableObjects.deleteObject",
      method: "DELETE",
      path: (params: { accountId: AccountId; namespaceId: ResourceId; objectId: DurableObjectId }) =>
        `accounts/${params.accountId}/durable-objects/namespaces/${params.namespaceId}/objects/${params.objectId}`,
      successSchema: EmptyResponseSchema,
      paramsSchema: DoObjectScopeSchema,
    } satisfies JsonOperationDef<
      { accountId: AccountId; namespaceId: ResourceId; objectId: DurableObjectId },
      unknown
    >,
    renameNamespace: {
      id: "durableObjects.renameNamespace",
      method: "PATCH",
      path: (params: { accountId: AccountId; namespaceId: ResourceId }) =>
        `accounts/${params.accountId}/durable-objects/namespaces/${params.namespaceId}`,
      successSchema: ResourceRecordSchema,
      paramsSchema: RenameDoNamespaceParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; namespaceId: ResourceId; name: string }, unknown>,
    deleteNamespace: {
      id: "durableObjects.deleteNamespace",
      method: "DELETE",
      path: (params: { accountId: AccountId; namespaceId: ResourceId }) =>
        `accounts/${params.accountId}/durable-objects/namespaces/${params.namespaceId}`,
      successSchema: EmptyResponseSchema,
      paramsSchema: DeleteDoNamespaceParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<
      { accountId: AccountId; namespaceId: ResourceId; idempotencyKey: string; force?: boolean | undefined },
      unknown
    >,
    listObjects: {
      id: "durableObjects.listObjects",
      method: "GET",
      path: (params: { accountId: AccountId; namespaceId: ResourceId }) =>
        `accounts/${params.accountId}/durable-objects/namespaces/${params.namespaceId}/objects`,
      successSchema: DoObjectsResponseSchema,
      paramsSchema: DoListObjectsParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; namespaceId: ResourceId }, unknown>,
  },
  queues: {
    list: {
      id: "queues.list",
      method: "GET",
      path: (params: { accountId: AccountId }) => `accounts/${params.accountId}/queues`,
      successSchema: QueuesResponseSchema,
      paramsSchema: QueuesListParamsSchema,
    } satisfies JsonOperationDef<
      {
        accountId: AccountId;
        search?: string | undefined;
        status?: string | undefined;
        sort?: "name" | "createdAt" | "updatedAt" | undefined;
        direction?: "asc" | "desc" | undefined;
        cursor?: PageCursor | undefined;
        limit?: number | undefined;
      },
      unknown
    >,
    create: {
      id: "queues.create",
      method: "POST",
      path: (params: { accountId: AccountId }) => `accounts/${params.accountId}/queues`,
      successSchema: QueueResponseSchema,
      paramsSchema: CreateQueueParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<
      {
        accountId: AccountId;
        name: string;
        idempotencyKey: string;
        deliveryDelaySeconds?: number | undefined;
        retentionSeconds?: number | undefined;
        maxBacklogBytes?: number | undefined;
      },
      unknown
    >,
    get: {
      id: "queues.get",
      method: "GET",
      path: (params: { accountId: AccountId; queueId: QueueId }) =>
        `accounts/${params.accountId}/queues/${params.queueId}`,
      successSchema: QueueDetailResponseSchema,
      paramsSchema: QueueScopeParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; queueId: QueueId }, unknown>,
    rename: {
      id: "queues.rename",
      method: "PATCH",
      path: (params: { accountId: AccountId; queueId: QueueId }) =>
        `accounts/${params.accountId}/queues/${params.queueId}`,
      successSchema: QueueResponseSchema,
      paramsSchema: RenameQueueParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<
      {
        accountId: AccountId;
        queueId: QueueId;
        name: string;
        expectedConfigGeneration: number;
        idempotencyKey: string;
      },
      unknown
    >,
    updateConfig: {
      id: "queues.updateConfig",
      method: "PATCH",
      path: (params: { accountId: AccountId; queueId: QueueId }) =>
        `accounts/${params.accountId}/queues/${params.queueId}`,
      successSchema: QueueResponseSchema,
      paramsSchema: UpdateQueueConfigParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<
      {
        accountId: AccountId;
        queueId: QueueId;
        expectedConfigGeneration: number;
        idempotencyKey: string;
        deliveryDelaySeconds?: number | undefined;
        retentionSeconds?: number | undefined;
        maxBacklogBytes?: number | undefined;
      },
      unknown
    >,
    delete: {
      id: "queues.delete",
      method: "DELETE",
      path: (params: { accountId: AccountId; queueId: QueueId }) =>
        `accounts/${params.accountId}/queues/${params.queueId}`,
      successSchema: QueueDeleteResponseSchema,
      paramsSchema: DeleteQueueParamsSchema,
      idempotent: true,
    } satisfies JsonOperationDef<
      {
        accountId: AccountId;
        queueId: QueueId;
        idempotencyKey: string;
        expectedLifecycleGeneration: number;
        force?: boolean | undefined;
      },
      unknown
    >,
  },
  workflows: {
    list: {
      id: "workflows.list",
      method: "GET",
      path: (params: { accountId: AccountId }) => `accounts/${params.accountId}/workflows`,
      successSchema: WorkflowsResponseSchema,
      paramsSchema: WorkflowsListParamsSchema,
    } satisfies JsonOperationDef<
      {
        accountId: AccountId;
        search?: string | undefined;
        status?: string | undefined;
        sort?: "name" | "createdAt" | "updatedAt" | undefined;
        direction?: "asc" | "desc" | undefined;
        cursor?: PageCursor | undefined;
        limit?: number | undefined;
      },
      unknown
    >,
    create: {
      id: "workflows.create",
      method: "POST",
      path: (params: { accountId: AccountId }) => `accounts/${params.accountId}/workflows`,
      successSchema: WorkflowSchema,
      paramsSchema: CreateWorkflowParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; name: string }, unknown>,
    get: {
      id: "workflows.get",
      method: "GET",
      path: (params: { accountId: AccountId; workflowId: WorkflowId }) =>
        `accounts/${params.accountId}/workflows/${params.workflowId}`,
      successSchema: WorkflowDetailResponseSchema,
      paramsSchema: WorkflowScopeParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; workflowId: WorkflowId }, unknown>,
    rename: {
      id: "workflows.rename",
      method: "PATCH",
      path: (params: { accountId: AccountId; workflowId: WorkflowId }) =>
        `accounts/${params.accountId}/workflows/${params.workflowId}`,
      successSchema: WorkflowSchema,
      paramsSchema: RenameWorkflowParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; workflowId: WorkflowId; name: string }, unknown>,
    delete: {
      id: "workflows.delete",
      method: "DELETE",
      path: (params: { accountId: AccountId; workflowId: WorkflowId }) =>
        `accounts/${params.accountId}/workflows/${params.workflowId}`,
      successSchema: WorkflowSchema,
      paramsSchema: WorkflowScopeParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; workflowId: WorkflowId }, unknown>,
    listVersions: {
      id: "workflows.listVersions",
      method: "GET",
      path: (params: { accountId: AccountId; workflowId: WorkflowId }) =>
        `accounts/${params.accountId}/workflows/${params.workflowId}/versions`,
      successSchema: WorkflowVersionsResponseSchema,
      paramsSchema: WorkflowVersionsListParamsSchema,
    } satisfies JsonOperationDef<
      { accountId: AccountId; workflowId: WorkflowId; after?: number | undefined; limit?: number | undefined },
      unknown
    >,
    createVersion: {
      id: "workflows.createVersion",
      method: "POST",
      path: (params: { accountId: AccountId; workflowId: WorkflowId }) =>
        `accounts/${params.accountId}/workflows/${params.workflowId}/versions`,
      successSchema: WorkflowVersionSchema,
      paramsSchema: CreateWorkflowVersionParamsSchema,
    } satisfies JsonOperationDef<
      { accountId: AccountId; workflowId: WorkflowId; deploymentId: DeploymentId; className: string },
      unknown
    >,
    listInstances: {
      id: "workflows.listInstances",
      method: "GET",
      path: (params: { accountId: AccountId; workflowId: WorkflowId }) =>
        `accounts/${params.accountId}/workflows/${params.workflowId}/instances`,
      successSchema: WorkflowInstancesResponseSchema,
      paramsSchema: WorkflowInstancesListParamsSchema,
    } satisfies JsonOperationDef<
      { accountId: AccountId; workflowId: WorkflowId; after?: string | undefined; limit?: number | undefined },
      unknown
    >,
    getInstance: {
      id: "workflows.getInstance",
      method: "GET",
      path: (params: { accountId: AccountId; workflowId: WorkflowId; instanceId: string }) =>
        `accounts/${params.accountId}/workflows/${params.workflowId}/instances/${params.instanceId}`,
      successSchema: WorkflowInstanceSchema,
      paramsSchema: WorkflowInstanceScopeParamsSchema,
    } satisfies JsonOperationDef<
      { accountId: AccountId; workflowId: WorkflowId; instanceId: string },
      unknown
    >,
    listSteps: {
      id: "workflows.listSteps",
      method: "GET",
      path: (params: { accountId: AccountId; workflowId: WorkflowId; instanceId: string }) =>
        `accounts/${params.accountId}/workflows/${params.workflowId}/instances/${params.instanceId}/steps`,
      successSchema: WorkflowStepsResponseSchema,
      paramsSchema: WorkflowStepsParamsSchema,
    } satisfies JsonOperationDef<
      { accountId: AccountId; workflowId: WorkflowId; instanceId: string; after?: number | undefined; limit?: number | undefined },
      unknown
    >,
    pauseInstance: workflowInstanceMutation("pause"),
    resumeInstance: workflowInstanceMutation("resume"),
    terminateInstance: workflowInstanceMutation("terminate"),
    restartInstance: workflowInstanceMutation("restart"),
    sendEvent: {
      id: "workflows.sendEvent",
      method: "POST",
      path: (params: { accountId: AccountId; workflowId: WorkflowId; instanceId: string }) =>
        `accounts/${params.accountId}/workflows/${params.workflowId}/instances/${params.instanceId}/events`,
      successSchema: WorkflowMutationResponseSchema,
      paramsSchema: WorkflowEventParamsSchema,
    } satisfies JsonOperationDef<
      { accountId: AccountId; workflowId: WorkflowId; instanceId: string; eventType: string; payloadBase64: string },
      unknown
    >,
    reconcile: {
      id: "workflows.reconcile",
      method: "POST",
      path: () => "workflows/reconcile",
      successSchema: WorkflowReconcileResponseSchema,
    } satisfies JsonOperationDef<Record<string, never>, unknown>,
  },
  platform: {
    scheduler: {
      id: "platform.scheduler",
      method: "GET",
      path: () => "scheduler",
      successSchema: SchedulerSummarySchema,
    } satisfies JsonOperationDef<Record<string, never>, unknown>,
    queueConsumers: {
      id: "platform.queueConsumers",
      method: "GET",
      path: () => "queue-consumers",
      successSchema: SchedulerSummarySchema,
    } satisfies JsonOperationDef<Record<string, never>, unknown>,
    cronActivations: {
      id: "platform.cronActivations",
      method: "GET",
      path: () => "cron-activations",
      successSchema: SchedulerSummarySchema,
    } satisfies JsonOperationDef<Record<string, never>, unknown>,
    cache: {
      id: "platform.cache",
      method: "GET",
      path: () => "cache",
      successSchema: CacheSummarySchema,
    } satisfies JsonOperationDef<Record<string, never>, unknown>,
    cacheGc: {
      id: "platform.cacheGc",
      method: "POST",
      path: () => "cache/gc",
      successSchema: CacheGcResponseSchema,
    } satisfies JsonOperationDef<Record<string, never>, unknown>,
    workerCache: {
      id: "platform.workerCache",
      method: "GET",
      path: (params: { accountId: AccountId; workerId: WorkerId }) =>
        `accounts/${params.accountId}/workers/${params.workerId}/cache`,
      successSchema: CacheSummarySchema,
      paramsSchema: WorkerScopeParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; workerId: WorkerId }, unknown>,
    purgeWorkerCache: {
      id: "platform.purgeWorkerCache",
      method: "POST",
      path: (params: { accountId: AccountId; workerId: WorkerId }) =>
        `accounts/${params.accountId}/workers/${params.workerId}/cache/purge`,
      successSchema: CachePurgeResponseSchema,
      paramsSchema: WorkerScopeParamsSchema,
    } satisfies JsonOperationDef<{ accountId: AccountId; workerId: WorkerId }, unknown>,
    imagesCapacity: {
      id: "platform.imagesCapacity",
      method: "GET",
      path: () => "images/capacity",
      successSchema: ImagesCapacitySchema,
    } satisfies JsonOperationDef<Record<string, never>, unknown>,
    pauseScheduler: {
      id: "platform.pauseScheduler",
      method: "POST",
      path: () => "scheduler/pause",
      successSchema: EmptyResponseSchema,
      paramsSchema: SchedulerMutationParamsSchema,
    } satisfies JsonOperationDef<{ kind?: "queue" | "cron" | "workflow" | undefined }, unknown>,
    resumeScheduler: {
      id: "platform.resumeScheduler",
      method: "POST",
      path: () => "scheduler/resume",
      successSchema: EmptyResponseSchema,
      paramsSchema: SchedulerMutationParamsSchema,
    } satisfies JsonOperationDef<{ kind?: "queue" | "cron" | "workflow" | undefined }, unknown>,
    repairScheduler: {
      id: "platform.repairScheduler",
      method: "POST",
      path: () => "scheduler/repair",
      successSchema: SchedulerRepairResponseSchema,
    } satisfies JsonOperationDef<Record<string, never>, unknown>,
    pauseQueueConsumer: {
      id: "platform.pauseQueueConsumer",
      method: "POST",
      path: (params: { consumerId: QueueConsumerId }) => `queue-consumers/${params.consumerId}/pause`,
      successSchema: EmptyResponseSchema,
      paramsSchema: QueueConsumerMutationParamsSchema,
    } satisfies JsonOperationDef<{ consumerId: QueueConsumerId; consumerGeneration: number }, unknown>,
    resumeQueueConsumer: {
      id: "platform.resumeQueueConsumer",
      method: "POST",
      path: (params: { consumerId: QueueConsumerId }) => `queue-consumers/${params.consumerId}/resume`,
      successSchema: EmptyResponseSchema,
      paramsSchema: QueueConsumerMutationParamsSchema,
    } satisfies JsonOperationDef<{ consumerId: QueueConsumerId; consumerGeneration: number }, unknown>,
  },
} as const;

export type OperatorOperations = typeof operatorOperations;

export function listOperationIds(): string[] {
  const ids: string[] = [];
  for (const group of Object.values(operatorOperations)) {
    for (const operation of Object.values(group)) {
      ids.push(operation.id);
    }
  }
  return ids.sort();
}
