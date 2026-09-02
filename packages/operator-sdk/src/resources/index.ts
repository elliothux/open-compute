import type { OperatorTransport, OperatorRequestBody, RequestOptions } from "../transport.js";
import { requestOptions } from "../transport.js";
import type {
  AccountId,
  DeploymentId,
  DeploymentUploadId,
  DurableObjectId,
  PageCursor,
  QueueConsumerId,
  QueueId,
  ResourceId,
  RouteId,
  Sha256Digest,
  WorkerId,
  WorkflowId,
} from "../schemas/ids.js";
import type {
  CreateDeploymentUploadBody,
  FinalizeDeploymentUploadBody,
} from "../schemas/workers.js";
import { invokeBinaryOperation, invokeBodyJsonOperation, invokeJsonOperation } from "../operations/call.js";
import { operatorOperations } from "../operations/registry.js";

type WorkflowInstanceActionParams = {
  accountId: AccountId;
  workflowId: WorkflowId;
  instanceId: string;
  signal?: AbortSignal;
};

export function createSystemResource(transport: OperatorTransport) {
  const ops = operatorOperations.system;
  return {
    meta(options: RequestOptions = {}) {
      return invokeJsonOperation(transport, ops.meta, {}, options);
    },
    account(options: RequestOptions = {}) {
      return invokeJsonOperation(transport, ops.account, {}, options);
    },
    status(options: RequestOptions = {}) {
      return invokeJsonOperation(transport, ops.status, {}, options);
    },
  };
}

export function createWorkersResource(transport: OperatorTransport) {
  const ops = operatorOperations.workers;
  return {
    list(params: {
      accountId: AccountId;
      search?: string;
      deployed?: boolean;
      sort?: "name" | "createdAt" | "updatedAt";
      direction?: "asc" | "desc";
      cursor?: PageCursor;
      limit?: number;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.list, params, {
        ...requestOptions({ signal: params.signal }),
        query: {
          search: params.search,
          deployed: params.deployed,
          sort: params.sort,
          direction: params.direction,
          cursor: params.cursor,
          limit: params.limit,
        },
      });
    },
    create(params: {
      accountId: AccountId;
      name: string;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.create, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
        body: { name: params.name },
      });
    },
    get(params: { accountId: AccountId; workerId: WorkerId; signal?: AbortSignal }) {
      return invokeJsonOperation(transport, ops.get, params, requestOptions({ signal: params.signal }));
    },
    delete(params: {
      accountId: AccountId;
      workerId: WorkerId;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.delete, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
      });
    },
    listDeployments(params: { accountId: AccountId; workerId: WorkerId; signal?: AbortSignal }) {
      return invokeJsonOperation(transport, ops.listDeployments, params, requestOptions({ signal: params.signal }));
    },
    getDeployment(params: {
      accountId: AccountId;
      workerId: WorkerId;
      deploymentId: DeploymentId;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.getDeployment, params, requestOptions({ signal: params.signal }));
    },
    deleteDeployment(params: {
      accountId: AccountId;
      workerId: WorkerId;
      deploymentId: DeploymentId;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.deleteDeployment, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
      });
    },
    listRoutes(params: { accountId: AccountId; workerId: WorkerId; signal?: AbortSignal }) {
      return invokeJsonOperation(transport, ops.listRoutes, params, requestOptions({ signal: params.signal }));
    },
    createRoute(params: {
      accountId: AccountId;
      workerId: WorkerId;
      hostname: string;
      pathPrefix: string;
      entrypoint?: string;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.createRoute, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
        body: {
          hostname: params.hostname,
          pathPrefix: params.pathPrefix,
          ...(params.entrypoint !== undefined ? { entrypoint: params.entrypoint } : {}),
        },
      });
    },
    deleteRoute(params: {
      accountId: AccountId;
      workerId: WorkerId;
      routeId: RouteId;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.deleteRoute, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
      });
    },
    promote(params: {
      accountId: AccountId;
      workerId: WorkerId;
      targetDeploymentId: DeploymentId;
      expectedActiveDeploymentId?: DeploymentId | null;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(
        transport,
        ops.promote,
        {
          accountId: params.accountId,
          workerId: params.workerId,
          targetDeploymentId: params.targetDeploymentId,
          expectedActiveDeploymentId: params.expectedActiveDeploymentId ?? null,
          idempotencyKey: params.idempotencyKey,
        },
        {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
        body: {
          targetDeploymentId: params.targetDeploymentId,
          expectedActiveDeploymentId: params.expectedActiveDeploymentId ?? null,
        },
      });
    },
    rollback(params: {
      accountId: AccountId;
      workerId: WorkerId;
      targetDeploymentId: DeploymentId;
      expectedActiveDeploymentId?: DeploymentId | null;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(
        transport,
        ops.rollback,
        {
          accountId: params.accountId,
          workerId: params.workerId,
          targetDeploymentId: params.targetDeploymentId,
          expectedActiveDeploymentId: params.expectedActiveDeploymentId ?? null,
          idempotencyKey: params.idempotencyKey,
        },
        {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
        body: {
          targetDeploymentId: params.targetDeploymentId,
          expectedActiveDeploymentId: params.expectedActiveDeploymentId ?? null,
        },
      });
    },
    createDeployment(params: {
      accountId: AccountId;
      workerId: WorkerId;
      bundle: OperatorRequestBody;
      metadata: string;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeBodyJsonOperation(
        transport,
        ops.createDeployment,
        {
          accountId: params.accountId,
          workerId: params.workerId,
          idempotencyKey: params.idempotencyKey,
          metadata: params.metadata,
        },
        {
          ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
          body: params.bundle,
          contentType: "application/octet-stream",
          headers: { "x-open-compute-deployment-metadata": params.metadata },
        },
      );
    },
    createDeploymentUpload(params: {
      accountId: AccountId;
      workerId: WorkerId;
      body: CreateDeploymentUploadBody;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(
        transport,
        ops.createDeploymentUpload,
        {
          accountId: params.accountId,
          workerId: params.workerId,
          idempotencyKey: params.idempotencyKey,
        },
        {
          ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
          body: params.body,
        },
      );
    },
    getDeploymentUpload(params: {
      accountId: AccountId;
      workerId: WorkerId;
      uploadId: DeploymentUploadId;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(
        transport,
        ops.getDeploymentUpload,
        params,
        requestOptions({ signal: params.signal }),
      );
    },
    abortDeploymentUpload(params: {
      accountId: AccountId;
      workerId: WorkerId;
      uploadId: DeploymentUploadId;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(
        transport,
        ops.abortDeploymentUpload,
        params,
        requestOptions({ signal: params.signal }),
      );
    },
    putDeploymentUploadObject(params: {
      accountId: AccountId;
      workerId: WorkerId;
      uploadId: DeploymentUploadId;
      sha256: Sha256Digest;
      body: OperatorRequestBody;
      signal?: AbortSignal;
    }) {
      return invokeBodyJsonOperation(
        transport,
        ops.putDeploymentUploadObject,
        {
          accountId: params.accountId,
          workerId: params.workerId,
          uploadId: params.uploadId,
          sha256: params.sha256,
        },
        {
          ...requestOptions({ signal: params.signal }),
          body: params.body,
          contentType: "application/octet-stream",
        },
      );
    },
    finalizeDeploymentUpload(params: {
      accountId: AccountId;
      workerId: WorkerId;
      uploadId: DeploymentUploadId;
      body: FinalizeDeploymentUploadBody;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(
        transport,
        ops.finalizeDeploymentUpload,
        {
          accountId: params.accountId,
          workerId: params.workerId,
          uploadId: params.uploadId,
        },
        {
          ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
          body: params.body,
        },
      );
    },
  };
}

export function createKvResource(transport: OperatorTransport) {
  const ops = operatorOperations.kv;
  return {
    listNamespaces(params: {
      accountId: AccountId;
      search?: string;
      status?: string;
      sort?: "name" | "createdAt" | "updatedAt";
      direction?: "asc" | "desc";
      cursor?: PageCursor;
      limit?: number;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.listNamespaces, params, {
        ...requestOptions({ signal: params.signal }),
        query: { search: params.search, status: params.status, sort: params.sort, direction: params.direction, cursor: params.cursor, limit: params.limit },
      });
    },
    createNamespace(params: {
      accountId: AccountId;
      name: string;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.createNamespace, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
        body: { name: params.name },
      });
    },
    getNamespace(params: {
      accountId: AccountId;
      namespaceId: ResourceId;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.getNamespace, params, requestOptions({ signal: params.signal }));
    },
    renameNamespace(params: {
      accountId: AccountId;
      namespaceId: ResourceId;
      name: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.renameNamespace, params, {
        ...requestOptions({ signal: params.signal }),
        body: { name: params.name },
      });
    },
    deleteNamespace(params: {
      accountId: AccountId;
      namespaceId: ResourceId;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.deleteNamespace, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
      });
    },
    listKeys(params: {
      accountId: AccountId;
      namespaceId: ResourceId;
      prefix?: string;
      cursor?: PageCursor;
      limit?: number;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.listKeys, params, {
        ...requestOptions({ signal: params.signal }),
        query: { prefix: params.prefix, cursor: params.cursor, limit: params.limit },
      });
    },
    getValue(params: {
      accountId: AccountId;
      namespaceId: ResourceId;
      key: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.getValue, params, requestOptions({ signal: params.signal }));
    },
    putValue(params: {
      accountId: AccountId;
      namespaceId: ResourceId;
      key: string;
      value: string;
      metadata?: unknown;
      expiration?: number;
      expirationTtl?: number;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.putValue, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
        body: {
          value: params.value,
          ...(params.metadata !== undefined ? { metadata: params.metadata } : {}),
          ...(params.expiration !== undefined ? { expiration: params.expiration } : {}),
          ...(params.expirationTtl !== undefined ? { expirationTtl: params.expirationTtl } : {}),
        },
      });
    },
    deleteValue(params: {
      accountId: AccountId;
      namespaceId: ResourceId;
      key: string;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.deleteValue, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
      });
    },
    listBackups(params: { accountId: AccountId; signal?: AbortSignal }) {
      return invokeJsonOperation(transport, ops.listBackups, params, requestOptions({ signal: params.signal }));
    },
    createBackup(params: {
      accountId: AccountId;
      namespaceId: ResourceId;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.createBackup, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
      });
    },
    restoreNamespace(params: {
      accountId: AccountId;
      backupId: string;
      newName: string;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.restoreNamespace, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
        body: { backupId: params.backupId, newName: params.newName },
      });
    },
    deleteBackup(params: {
      accountId: AccountId;
      backupId: string;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.deleteBackup, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
      });
    },
  };
}

export function createD1Resource(transport: OperatorTransport) {
  const ops = operatorOperations.d1;
  return {
    listDatabases(params: {
      accountId: AccountId;
      search?: string;
      status?: string;
      sort?: "name" | "createdAt" | "updatedAt";
      direction?: "asc" | "desc";
      cursor?: PageCursor;
      limit?: number;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.listDatabases, params, {
        ...requestOptions({ signal: params.signal }),
        query: { search: params.search, status: params.status, sort: params.sort, direction: params.direction, cursor: params.cursor, limit: params.limit },
      });
    },
    createDatabase(params: {
      accountId: AccountId;
      name: string;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.createDatabase, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
        body: { name: params.name },
      });
    },
    getDatabase(params: {
      accountId: AccountId;
      databaseId: ResourceId;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.getDatabase, params, requestOptions({ signal: params.signal }));
    },
    renameDatabase(params: {
      accountId: AccountId;
      databaseId: ResourceId;
      name: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.renameDatabase, params, {
        ...requestOptions({ signal: params.signal }),
        body: { name: params.name },
      });
    },
    deleteDatabase(params: {
      accountId: AccountId;
      databaseId: ResourceId;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.deleteDatabase, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
      });
    },
    listTables(params: {
      accountId: AccountId;
      databaseId: ResourceId;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.listTables, params, requestOptions({ signal: params.signal }));
    },
    query(params: {
      accountId: AccountId;
      databaseId: ResourceId;
      sql: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.query, params, {
        ...requestOptions({ signal: params.signal }),
        body: { sql: params.sql },
      });
    },
    listMigrations(params: {
      accountId: AccountId;
      databaseId: ResourceId;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.listMigrations, params, requestOptions({ signal: params.signal }));
    },
    applyMigrations(params: {
      accountId: AccountId;
      databaseId: ResourceId;
      migrations: Array<{ id: number; name: string; sha256: Sha256Digest; sql: string }>;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.applyMigrations, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
        body: { migrations: params.migrations },
      });
    },
    listBackups(params: {
      accountId: AccountId;
      databaseId: ResourceId;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.listBackups, params, requestOptions({ signal: params.signal }));
    },
    createBackup(params: {
      accountId: AccountId;
      databaseId: ResourceId;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.createBackup, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
      });
    },
    restoreDatabase(params: {
      accountId: AccountId;
      backupId: string;
      newName: string;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.restoreDatabase, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
        body: { backupId: params.backupId, newName: params.newName },
      });
    },
  };
}

export function createR2Resource(transport: OperatorTransport) {
  const ops = operatorOperations.r2;
  return {
    listBuckets(params: {
      accountId: AccountId;
      search?: string;
      status?: string;
      sort?: "name" | "createdAt" | "updatedAt";
      direction?: "asc" | "desc";
      cursor?: PageCursor;
      limit?: number;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.listBuckets, params, {
        ...requestOptions({ signal: params.signal }),
        query: { search: params.search, status: params.status, sort: params.sort, direction: params.direction, cursor: params.cursor, limit: params.limit },
      });
    },
    createBucket(params: {
      accountId: AccountId;
      name: string;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.createBucket, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
        body: { name: params.name },
      });
    },
    getBucket(params: {
      accountId: AccountId;
      bucketId: ResourceId;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.getBucket, params, requestOptions({ signal: params.signal }));
    },
    renameBucket(params: {
      accountId: AccountId;
      bucketId: ResourceId;
      name: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.renameBucket, params, {
        ...requestOptions({ signal: params.signal }),
        body: { name: params.name },
      });
    },
    deleteBucket(params: {
      accountId: AccountId;
      bucketId: ResourceId;
      idempotencyKey: string;
      force?: boolean;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.deleteBucket, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
        ...(params.force ? { query: { force: true } } : {}),
      });
    },
    listObjects(params: {
      accountId: AccountId;
      bucketId: ResourceId;
      prefix?: string;
      cursor?: string;
      limit?: number;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.listObjects, params, {
        ...requestOptions({ signal: params.signal }),
        query: { prefix: params.prefix, cursor: params.cursor, limit: params.limit },
      });
    },
    headObject(params: {
      accountId: AccountId;
      bucketId: ResourceId;
      key: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.headObject, params, {
        ...requestOptions({ signal: params.signal }),
        query: { metadata: true },
      });
    },
    async getObject(params: {
      accountId: AccountId;
      bucketId: ResourceId;
      key: string;
      maxBytes?: number;
      signal?: AbortSignal;
    }) {
      const callOptions = {
        ...requestOptions({ signal: params.signal }),
        ...(params.maxBytes !== undefined ? { maxBytes: params.maxBytes } : {}),
      };
      const download = await invokeBinaryOperation(
        transport,
        ops.getObject,
        { accountId: params.accountId, bucketId: params.bucketId, key: params.key },
        callOptions,
      );
      return {
        key: params.key,
        contentLength: download.contentLength,
        etag: download.headers.get("etag") ?? undefined,
        body: download.body,
      };
    },
    putObject(params: {
      accountId: AccountId;
      bucketId: ResourceId;
      key: string;
      body: OperatorRequestBody;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeBodyJsonOperation(transport, ops.putObject, {
        accountId: params.accountId,
        bucketId: params.bucketId,
        key: params.key,
      }, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
        body: params.body,
        contentType: "application/octet-stream",
      });
    },
    deleteObject(params: {
      accountId: AccountId;
      bucketId: ResourceId;
      key: string;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.deleteObject, {
        accountId: params.accountId,
        bucketId: params.bucketId,
        key: params.key,
      }, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
      });
    },
  };
}

export function createDurableObjectsResource(transport: OperatorTransport) {
  const ops = operatorOperations.durableObjects;
  return {
    listNamespaces(params: {
      accountId: AccountId;
      search?: string;
      status?: string;
      sort?: "name" | "createdAt" | "updatedAt";
      direction?: "asc" | "desc";
      cursor?: PageCursor;
      limit?: number;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.listNamespaces, params, {
        ...requestOptions({ signal: params.signal }),
        query: { search: params.search, status: params.status, sort: params.sort, direction: params.direction, cursor: params.cursor, limit: params.limit },
      });
    },
    createNamespace(params: {
      accountId: AccountId;
      name: string;
      workerId: WorkerId;
      className: string;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.createNamespace, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
        body: { name: params.name, workerId: params.workerId, className: params.className },
      });
    },
    getNamespace(params: { accountId: AccountId; namespaceId: ResourceId; signal?: AbortSignal }) {
      return invokeJsonOperation(transport, ops.getNamespace, params, requestOptions({ signal: params.signal }));
    },
    renameNamespace(params: {
      accountId: AccountId;
      namespaceId: ResourceId;
      name: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.renameNamespace, params, {
        ...requestOptions({ signal: params.signal }),
        body: { name: params.name },
      });
    },
    deleteNamespace(params: {
      accountId: AccountId;
      namespaceId: ResourceId;
      idempotencyKey: string;
      force?: boolean;
      signal?: AbortSignal;
    }) {
      const callOptions = requestOptions({
        signal: params.signal,
        idempotencyKey: params.idempotencyKey,
      });
      if (params.force) {
        return invokeJsonOperation(transport, ops.deleteNamespace, params, {
          ...callOptions,
          query: { force: true },
        });
      }
      return invokeJsonOperation(transport, ops.deleteNamespace, params, callOptions);
    },
    listObjects(params: {
      accountId: AccountId;
      namespaceId: ResourceId;
      cursor?: string;
      limit?: number;
      search?: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.listObjects, params, {
        ...requestOptions({ signal: params.signal }),
        query: { cursor: params.cursor, limit: params.limit, search: params.search },
      });
    },
    getObject(params: {
      accountId: AccountId;
      namespaceId: ResourceId;
      objectId: DurableObjectId;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.getObject, params, requestOptions({ signal: params.signal }));
    },
    deleteObject(params: {
      accountId: AccountId;
      namespaceId: ResourceId;
      objectId: DurableObjectId;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.deleteObject, params, requestOptions({ signal: params.signal }));
    },
  };
}

export function createQueuesResource(transport: OperatorTransport) {
  const ops = operatorOperations.queues;
  return {
    list(params: {
      accountId: AccountId;
      search?: string;
      status?: string;
      sort?: "name" | "createdAt" | "updatedAt";
      direction?: "asc" | "desc";
      cursor?: PageCursor;
      limit?: number;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.list, params, {
        ...requestOptions({ signal: params.signal }),
        query: { search: params.search, status: params.status, sort: params.sort, direction: params.direction, cursor: params.cursor, limit: params.limit },
      });
    },
    create(params: {
      accountId: AccountId;
      name: string;
      idempotencyKey: string;
      deliveryDelaySeconds?: number;
      retentionSeconds?: number;
      maxBacklogBytes?: number;
      signal?: AbortSignal;
    }) {
      const body: Record<string, unknown> = { name: params.name };
      if (params.deliveryDelaySeconds !== undefined) body.deliveryDelaySeconds = params.deliveryDelaySeconds;
      if (params.retentionSeconds !== undefined) body.retentionSeconds = params.retentionSeconds;
      if (params.maxBacklogBytes !== undefined) body.maxBacklogBytes = params.maxBacklogBytes;
      return invokeJsonOperation(transport, ops.create, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
        body,
      });
    },
    get(params: { accountId: AccountId; queueId: QueueId; signal?: AbortSignal }) {
      return invokeJsonOperation(transport, ops.get, params, requestOptions({ signal: params.signal }));
    },
    rename(params: {
      accountId: AccountId;
      queueId: QueueId;
      name: string;
      expectedConfigGeneration: number;
      idempotencyKey: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.rename, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
        body: { name: params.name, expectedConfigGeneration: params.expectedConfigGeneration },
      });
    },
    updateConfig(params: {
      accountId: AccountId;
      queueId: QueueId;
      expectedConfigGeneration: number;
      idempotencyKey: string;
      deliveryDelaySeconds?: number;
      retentionSeconds?: number;
      maxBacklogBytes?: number;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.updateConfig, params, {
        ...requestOptions({ signal: params.signal, idempotencyKey: params.idempotencyKey }),
        body: {
          expectedConfigGeneration: params.expectedConfigGeneration,
          ...(params.deliveryDelaySeconds !== undefined ? { deliveryDelaySeconds: params.deliveryDelaySeconds } : {}),
          ...(params.retentionSeconds !== undefined ? { retentionSeconds: params.retentionSeconds } : {}),
          ...(params.maxBacklogBytes !== undefined ? { maxBacklogBytes: params.maxBacklogBytes } : {}),
        },
      });
    },
    delete(params: {
      accountId: AccountId;
      queueId: QueueId;
      idempotencyKey: string;
      expectedLifecycleGeneration: number;
      force?: boolean;
      signal?: AbortSignal;
    }) {
      const callOptions = requestOptions({
        signal: params.signal,
        idempotencyKey: params.idempotencyKey,
        headers: {
          "x-open-compute-expected-lifecycle-generation": String(params.expectedLifecycleGeneration),
        },
      });
      if (params.force) {
        return invokeJsonOperation(transport, ops.delete, params, {
          ...callOptions,
          query: { force: true },
        });
      }
      return invokeJsonOperation(transport, ops.delete, params, callOptions);
    },
  };
}

export function createWorkflowsResource(transport: OperatorTransport) {
  const ops = operatorOperations.workflows;
  return {
    list(params: {
      accountId: AccountId;
      search?: string;
      status?: string;
      sort?: "name" | "createdAt" | "updatedAt";
      direction?: "asc" | "desc";
      cursor?: PageCursor;
      limit?: number;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.list, params, {
        ...requestOptions({ signal: params.signal }),
        query: { search: params.search, status: params.status, sort: params.sort, direction: params.direction, cursor: params.cursor, limit: params.limit },
      });
    },
    create(params: { accountId: AccountId; name: string; signal?: AbortSignal }) {
      return invokeJsonOperation(transport, ops.create, params, {
        ...requestOptions({ signal: params.signal }),
        body: { name: params.name },
      });
    },
    get(params: { accountId: AccountId; workflowId: WorkflowId; signal?: AbortSignal }) {
      return invokeJsonOperation(transport, ops.get, params, requestOptions({ signal: params.signal }));
    },
    rename(params: {
      accountId: AccountId;
      workflowId: WorkflowId;
      name: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.rename, params, {
        ...requestOptions({ signal: params.signal }),
        body: { name: params.name },
      });
    },
    delete(params: { accountId: AccountId; workflowId: WorkflowId; signal?: AbortSignal }) {
      return invokeJsonOperation(transport, ops.delete, params, requestOptions({ signal: params.signal }));
    },
    listVersions(params: {
      accountId: AccountId;
      workflowId: WorkflowId;
      after?: number;
      limit?: number;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.listVersions, params, {
        ...requestOptions({ signal: params.signal }),
        query: { after: params.after, limit: params.limit },
      });
    },
    createVersion(params: {
      accountId: AccountId;
      workflowId: WorkflowId;
      deploymentId: DeploymentId;
      className: string;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.createVersion, params, {
        ...requestOptions({ signal: params.signal }),
        body: { deploymentId: params.deploymentId, className: params.className },
      });
    },
    listInstances(params: {
      accountId: AccountId;
      workflowId: WorkflowId;
      after?: string;
      limit?: number;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.listInstances, params, {
        ...requestOptions({ signal: params.signal }),
        query: { after: params.after, limit: params.limit },
      });
    },
    getInstance(params: WorkflowInstanceActionParams) {
      return invokeJsonOperation(transport, ops.getInstance, params, requestOptions({ signal: params.signal }));
    },
    listSteps(params: WorkflowInstanceActionParams & { after?: number; limit?: number }) {
      return invokeJsonOperation(transport, ops.listSteps, params, {
        ...requestOptions({ signal: params.signal }),
        query: { after: params.after, limit: params.limit },
      });
    },
    pauseInstance(params: WorkflowInstanceActionParams) {
      return invokeJsonOperation(transport, ops.pauseInstance, params, {
        ...requestOptions({ signal: params.signal }),
        body: {},
      });
    },
    resumeInstance(params: WorkflowInstanceActionParams) {
      return invokeJsonOperation(transport, ops.resumeInstance, params, {
        ...requestOptions({ signal: params.signal }),
        body: {},
      });
    },
    terminateInstance(params: WorkflowInstanceActionParams) {
      return invokeJsonOperation(transport, ops.terminateInstance, params, {
        ...requestOptions({ signal: params.signal }),
        body: {},
      });
    },
    restartInstance(params: WorkflowInstanceActionParams) {
      return invokeJsonOperation(transport, ops.restartInstance, params, {
        ...requestOptions({ signal: params.signal }),
        body: {},
      });
    },
    sendEvent(params: WorkflowInstanceActionParams & { eventType: string; payloadBase64: string }) {
      return invokeJsonOperation(transport, ops.sendEvent, params, {
        ...requestOptions({ signal: params.signal }),
        body: { type: params.eventType, payloadBase64: params.payloadBase64 },
      });
    },
    reconcile(options: RequestOptions = {}) {
      return invokeJsonOperation(transport, ops.reconcile, {}, options);
    },
  };
}

export function createPlatformResource(transport: OperatorTransport) {
  const ops = operatorOperations.platform;
  return {
    scheduler(options: RequestOptions = {}) {
      return invokeJsonOperation(transport, ops.scheduler, {}, options);
    },
    queueConsumers(options: RequestOptions = {}) {
      return invokeJsonOperation(transport, ops.queueConsumers, {}, options);
    },
    cronActivations(options: RequestOptions = {}) {
      return invokeJsonOperation(transport, ops.cronActivations, {}, options);
    },
    cache(options: RequestOptions = {}) {
      return invokeJsonOperation(transport, ops.cache, {}, options);
    },
    cacheGc(options: RequestOptions = {}) {
      return invokeJsonOperation(transport, ops.cacheGc, {}, options);
    },
    workerCache(params: { accountId: AccountId; workerId: WorkerId; signal?: AbortSignal }) {
      return invokeJsonOperation(transport, ops.workerCache, params, requestOptions({ signal: params.signal }));
    },
    purgeWorkerCache(params: { accountId: AccountId; workerId: WorkerId; signal?: AbortSignal }) {
      return invokeJsonOperation(transport, ops.purgeWorkerCache, params, requestOptions({ signal: params.signal }));
    },
    imagesCapacity(options: RequestOptions = {}) {
      return invokeJsonOperation(transport, ops.imagesCapacity, {}, options);
    },
    pauseScheduler(params: { kind?: "queue" | "cron" | "workflow"; signal?: AbortSignal } = {}) {
      return invokeJsonOperation(transport, ops.pauseScheduler, params, {
        ...requestOptions({ signal: params.signal }),
        ...(params.kind !== undefined ? { query: { kind: params.kind } } : {}),
      });
    },
    resumeScheduler(params: { kind?: "queue" | "cron" | "workflow"; signal?: AbortSignal } = {}) {
      return invokeJsonOperation(transport, ops.resumeScheduler, params, {
        ...requestOptions({ signal: params.signal }),
        ...(params.kind !== undefined ? { query: { kind: params.kind } } : {}),
      });
    },
    repairScheduler(options: RequestOptions = {}) {
      return invokeJsonOperation(transport, ops.repairScheduler, {}, options);
    },
    pauseQueueConsumer(params: {
      consumerId: QueueConsumerId;
      consumerGeneration: number;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.pauseQueueConsumer, params, {
        ...requestOptions({ signal: params.signal }),
        body: { consumerGeneration: params.consumerGeneration },
      });
    },
    resumeQueueConsumer(params: {
      consumerId: QueueConsumerId;
      consumerGeneration: number;
      signal?: AbortSignal;
    }) {
      return invokeJsonOperation(transport, ops.resumeQueueConsumer, params, {
        ...requestOptions({ signal: params.signal }),
        body: { consumerGeneration: params.consumerGeneration },
      });
    },
  };
}
