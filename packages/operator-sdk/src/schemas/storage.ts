import { z } from "zod";
import { Sha256DigestSchema } from "./ids.js";

export const ResourceRecordSchema = z.object({
  id: z.string(),
  accountId: z.string(),
  kind: z.string(),
  name: z.string(),
  state: z.string(),
  availability: z.string(),
  availabilityCode: z.string().nullable().optional(),
  specGeneration: z.number().optional(),
  driverSchemaVersion: z.number().optional(),
  createdAtMs: z.number(),
  updatedAtMs: z.number(),
  deletedAtMs: z.number().nullable().optional(),
});

export type ResourceRecord = z.output<typeof ResourceRecordSchema>;

export const KvNamespaceRecordSchema = z.object({
  resource: ResourceRecordSchema,
  schemaVersion: z.number(),
  quotaBytes: z.number(),
  lastOpenedAtMs: z.number().nullable().optional(),
  lastQuickCheckMs: z.number().nullable().optional(),
  lastBackupAtMs: z.number().nullable().optional(),
});

export type KvNamespaceRecord = z.output<typeof KvNamespaceRecordSchema>;

/** Catalog row for KV namespace list responses. */
export type KvNamespace = KvNamespaceRecord;

export const KvNamespacesResponseSchema = z.object({
  namespaces: z.array(KvNamespaceRecordSchema),
  cursor: z.string().nullable().optional(),
  listComplete: z.boolean().optional(),
});

export const KvKeySchema = z.strictObject({
  name: z.string(),
  expiration: z.number().nullable().optional(),
  metadata: z.unknown().optional(),
});

export const KvKeysResponseSchema = z.strictObject({
  keys: z.array(KvKeySchema),
  cursor: z.string().nullable().optional(),
  listComplete: z.boolean().optional(),
});

export const KvValueResponseSchema = z.strictObject({
  value: z.string().nullable(),
  metadata: z.unknown().optional(),
});

export const KvMutationResponseSchema = z.strictObject({
  key: z.string(),
});

export const CreateResourceResultSchema = z.strictObject({
  resourceId: z.string(),
  state: z.string(),
});

export const KvNamespaceResponseSchema = z.strictObject({
  namespace: KvNamespaceRecordSchema,
});

export const KvRenameNamespaceResponseSchema = z.strictObject({
  namespace: ResourceRecordSchema,
});

export const KvDeleteNamespaceResponseSchema = z.strictObject({
  resourceId: z.string(),
  state: z.string(),
});

export const KvBackupRecordSchema = z.object({
  id: z.string(),
  sourceResourceId: z.string(),
  state: z.string(),
  sizeBytes: z.number().nullable().optional(),
  kvSchemaVersion: z.number(),
  createdAtMs: z.number(),
  completedAtMs: z.number().nullable().optional(),
  errorCode: z.string().nullable().optional(),
});

export type KvBackupRecord = z.output<typeof KvBackupRecordSchema>;

export const KvBackupResponseSchema = z.strictObject({
  backup: KvBackupRecordSchema,
});

export const KvBackupsResponseSchema = z.strictObject({
  backups: z.array(KvBackupRecordSchema),
});

export const DeleteResourceResponseSchema = KvDeleteNamespaceResponseSchema;

export const D1DatabaseRecordSchema = z.object({
  resource: ResourceRecordSchema,
  schemaVersion: z.number(),
  quotaBytes: z.number(),
  lastOpenedAtMs: z.number().nullable().optional(),
  lastQuickCheckMs: z.number().nullable().optional(),
  lastBackupAtMs: z.number().nullable().optional(),
});

export type D1DatabaseRecord = z.output<typeof D1DatabaseRecordSchema>;

/** Catalog row for D1 database list responses. */
export type D1Database = D1DatabaseRecord;

export const D1DatabasesResponseSchema = z.object({
  databases: z.array(D1DatabaseRecordSchema),
  cursor: z.string().nullable().optional(),
  listComplete: z.boolean().optional(),
});

export const D1TableSchema = z.strictObject({
  name: z.string(),
});

export const D1TablesResponseSchema = z.strictObject({
  tables: z.array(D1TableSchema),
});

export const D1QueryResponseSchema = z.strictObject({
  results: z.array(z.record(z.string(), z.unknown())),
  meta: z.strictObject({
    durationMs: z.number().optional(),
    rowsRead: z.number().optional(),
    rowsWritten: z.number().optional(),
  }).optional(),
});

export const D1MigrationRecordSchema = z.strictObject({
  id: z.number().int().positive(),
  name: z.string(),
  sha256: z.string(),
  appliedAtMs: z.number(),
});

export type D1MigrationRecord = z.output<typeof D1MigrationRecordSchema>;

export const D1MigrationsResponseSchema = z.strictObject({
  migrations: z.array(D1MigrationRecordSchema),
});

export const D1MigrationInputSchema = z.strictObject({
  id: z.number().int().positive(),
  name: z.string().min(1),
  sha256: Sha256DigestSchema,
  sql: z.string().min(1).max(65_536),
});

export const D1ApplyMigrationsResponseSchema = z.strictObject({
  migrations: z.array(D1MigrationRecordSchema),
});

export const D1BackupRecordSchema = z.object({
  id: z.string(),
  sourceResourceId: z.string(),
  state: z.string(),
  sizeBytes: z.number().nullable().optional(),
  d1SchemaVersion: z.number(),
  sqliteUserVersion: z.number(),
  createdAtMs: z.number(),
  completedAtMs: z.number().nullable().optional(),
  errorCode: z.string().nullable().optional(),
});

export type D1BackupRecord = z.output<typeof D1BackupRecordSchema>;

export const D1BackupResponseSchema = z.strictObject({
  backup: D1BackupRecordSchema,
});

export const D1BackupsResponseSchema = z.strictObject({
  backups: z.array(D1BackupRecordSchema),
});

export const R2BucketSchema = z.object({
  resourceId: z.string(),
  name: z.string(),
  state: z.string(),
  availability: z.string(),
  createdAtMs: z.number(),
  updatedAtMs: z.number(),
  maxObjectBytes: z.number(),
});

export type R2Bucket = z.output<typeof R2BucketSchema>;

export const D1DatabaseDetailResponseSchema = z.strictObject({
  database: D1DatabaseRecordSchema,
});

export const D1DatabaseResourceResponseSchema = z.strictObject({
  database: ResourceRecordSchema,
});

export const R2BucketResponseSchema = z.strictObject({
  bucket: R2BucketSchema,
});

export const R2BucketsResponseSchema = z.object({
  buckets: z.array(R2BucketSchema),
  cursor: z.string().nullable().optional(),
  listComplete: z.boolean().optional(),
});

export const R2ObjectSchema = z.strictObject({
  key: z.string(),
  size: z.number(),
  etag: z.string().optional(),
  uploaded: z.number().optional(),
});

export const R2ObjectsResponseSchema = z.strictObject({
  objects: z.array(R2ObjectSchema),
  cursor: z.string().nullable().optional(),
  truncated: z.boolean().optional(),
});

export const R2ObjectMutationResponseSchema = z.strictObject({
  key: z.string(),
  size: z.number().optional(),
  etag: z.string().optional(),
  uploaded: z.number().optional(),
});
