import { z } from "zod";
import {
  AccountIdSchema,
  DeploymentIdSchema,
  DeploymentUploadIdSchema,
  RouteIdSchema,
  Sha256DigestSchema,
  WorkerIdSchema,
  type AccountId,
  type DeploymentId,
  type DeploymentUploadId,
  type WorkerId,
} from "./ids.js";

export const WorkerRecordSchema = z.strictObject({
  id: WorkerIdSchema,
  accountId: AccountIdSchema,
  name: z.string(),
  activeDeploymentId: DeploymentIdSchema.nullable().optional(),
  doStorageId: z.string().optional(),
  routeGeneration: z.number().optional(),
  ownership: z.enum(["tenant", "system"]).optional(),
  createdAtMs: z.number(),
  updatedAtMs: z.number().optional(),
  deletedAtMs: z.number().nullable().optional(),
  routeCount: z.number().int().nonnegative().optional(),
  primaryRoute: z.strictObject({
    id: RouteIdSchema,
    accountId: AccountIdSchema,
    workerId: WorkerIdSchema,
    kind: z.string(),
    hostnameAscii: z.string().nullable().optional(),
    pathPrefix: z.string(),
    entrypoint: z.string().nullable().optional(),
    generation: z.number().optional(),
  }).nullable().optional(),
  deploymentSource: z.enum(["operator_api"]).nullable().optional(),
  traffic: z.strictObject({
    requests: z.number().int().nonnegative(),
    errors: z.number().int().nonnegative(),
    averageLatencyMs: z.number().nonnegative(),
    lastStatus: z.number().int().nullable().optional(),
  }).optional(),
});

export type WorkerRecord = z.output<typeof WorkerRecordSchema>;

export const WorkersListResponseSchema = z.strictObject({
  workers: z.array(WorkerRecordSchema),
  cursor: z.string().nullable().optional(),
  listComplete: z.boolean().optional(),
});

export type WorkersListResponse = z.output<typeof WorkersListResponseSchema>;

export const DeploymentRecordSchema = z.strictObject({
  id: DeploymentIdSchema,
  workerId: WorkerIdSchema,
  versionNumber: z.number().optional(),
  contentKind: z.string().optional(),
  state: z.string(),
  artifactSha256: z.string().nullable().optional(),
  artifactSize: z.number().nullable().optional(),
  artifactSchemaVersion: z.number().nullable().optional(),
  mainModule: z.string().nullable().optional(),
  workerCodeSha256: z.string().optional(),
  loaderSchemaVersion: z.number().optional(),
  createdAtMs: z.number(),
  readyAtMs: z.number().nullable().optional(),
  rejectedAtMs: z.number().nullable().optional(),
  rejectionCode: z.string().nullable().optional(),
  deletedAtMs: z.number().nullable().optional(),
  contentDigest: z.string().optional(),
});

export type DeploymentRecord = z.output<typeof DeploymentRecordSchema>;

export const DeploymentsListResponseSchema = z.strictObject({
  deployments: z.array(DeploymentRecordSchema),
});

export type DeploymentsListResponse = z.output<typeof DeploymentsListResponseSchema>;

export const DeploymentResponseSchema = z.strictObject({
  deployment: DeploymentRecordSchema,
});

export type DeploymentResponse = z.output<typeof DeploymentResponseSchema>;

export const DeleteDeploymentResponseSchema = z.strictObject({
  deploymentId: DeploymentIdSchema,
  state: z.literal("tombstoned"),
});

export type DeleteDeploymentResponse = z.output<typeof DeleteDeploymentResponseSchema>;

export const RouteRecordSchema = z.strictObject({
  id: RouteIdSchema,
  accountId: AccountIdSchema,
  workerId: WorkerIdSchema,
  kind: z.string(),
  hostnameAscii: z.string().nullable().optional(),
  pathPrefix: z.string(),
  entrypoint: z.string().nullable().optional(),
  generation: z.number().optional(),
  deploymentId: DeploymentIdSchema.nullable().optional(),
});

export type RouteRecord = z.output<typeof RouteRecordSchema>;

export const RoutesListResponseSchema = z.strictObject({
  routes: z.array(RouteRecordSchema),
});

export type RoutesListResponse = z.output<typeof RoutesListResponseSchema>;

export const WorkerDetailResponseSchema = z.strictObject({
  worker: WorkerRecordSchema,
});

export type WorkerDetailResponse = z.output<typeof WorkerDetailResponseSchema>;

export const CreateWorkerResponseSchema = z.strictObject({
  worker: WorkerRecordSchema,
  defaultRoute: RouteRecordSchema.optional(),
});

export type CreateWorkerResponse = z.output<typeof CreateWorkerResponseSchema>;

export const CreateDeploymentResponseSchema = z.strictObject({
  deployment: DeploymentRecordSchema,
  promoted: z.boolean(),
});

export type CreateDeploymentResponse = z.output<typeof CreateDeploymentResponseSchema>;

export const DeploymentUploadObjectSchema = z.strictObject({
  sha256: Sha256DigestSchema,
  kind: z.string(),
  size: z.number(),
  verified: z.boolean(),
  verifiedAtMs: z.number().nullable().optional(),
});

export type DeploymentUploadObject = z.output<typeof DeploymentUploadObjectSchema>;

export const DeploymentUploadSessionSchema = z.strictObject({
  id: DeploymentUploadIdSchema,
  accountId: AccountIdSchema,
  workerId: WorkerIdSchema,
  contentKind: z.string(),
  status: z.string(),
  deploymentId: DeploymentIdSchema.nullable().optional(),
  errorCode: z.string().nullable().optional(),
  createdAtMs: z.number(),
  expiresAtMs: z.number(),
  updatedAtMs: z.number(),
  objects: z.array(DeploymentUploadObjectSchema),
});

export type DeploymentUploadSession = z.output<typeof DeploymentUploadSessionSchema>;

export const CreateDeploymentUploadBodySchema = z.strictObject({
  contentKind: z.enum(["worker", "assets_only"]),
  bundle: z
    .strictObject({
      sha256: Sha256DigestSchema,
      size: z.number().int().nonnegative(),
    })
    .optional(),
  manifest: z.unknown(),
  routing: z.unknown(),
});

export type CreateDeploymentUploadBody = z.output<typeof CreateDeploymentUploadBodySchema>;

export const FinalizeDeploymentUploadBodySchema = z.strictObject({
  mainModule: z.string().optional(),
  vars: z.record(z.string(), z.unknown()).optional(),
  secrets: z.record(z.string(), z.string()).optional(),
  bindings: z.record(z.string(), z.unknown()).optional(),
  services: z.record(z.string(), z.unknown()).optional(),
  cache: z.unknown().optional(),
  images: z.unknown().optional(),
  versionMetadata: z.unknown().optional(),
  promote: z.boolean().optional(),
});

export type FinalizeDeploymentUploadBody = z.output<typeof FinalizeDeploymentUploadBodySchema>;

export const CreateRouteResponseSchema = z.strictObject({
  route: RouteRecordSchema,
});

export type CreateRouteResponse = z.output<typeof CreateRouteResponseSchema>;

export const DeleteRouteResponseSchema = z.strictObject({
  routeId: RouteIdSchema,
  state: z.literal("tombstoned"),
});

export type DeleteRouteResponse = z.output<typeof DeleteRouteResponseSchema>;

export const DeleteWorkerResponseSchema = z.strictObject({
  workerId: WorkerIdSchema,
  state: z.literal("tombstoned"),
});

export type DeleteWorkerResponse = z.output<typeof DeleteWorkerResponseSchema>;
