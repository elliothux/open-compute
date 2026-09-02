import { z } from "zod";

export const AccountIdSchema = z.string().min(1).brand("AccountId");
export type AccountId = z.output<typeof AccountIdSchema>;

export const WorkerIdSchema = z.string().min(1).brand("WorkerId");
export type WorkerId = z.output<typeof WorkerIdSchema>;

export const DeploymentIdSchema = z.string().min(1).brand("DeploymentId");
export type DeploymentId = z.output<typeof DeploymentIdSchema>;

export const DeploymentUploadIdSchema = z.string().min(1).brand("DeploymentUploadId");
export type DeploymentUploadId = z.output<typeof DeploymentUploadIdSchema>;

export const ResourceIdSchema = z.string().min(1).brand("ResourceId");
export type ResourceId = z.output<typeof ResourceIdSchema>;

export const RouteIdSchema = z.string().min(1).brand("RouteId");
export type RouteId = z.output<typeof RouteIdSchema>;

export const DurableObjectIdSchema = z.string().min(1).brand("DurableObjectId");
export type DurableObjectId = z.output<typeof DurableObjectIdSchema>;

export const PageCursorSchema = z.string().min(1).brand("PageCursor");
export type PageCursor = z.output<typeof PageCursorSchema>;

export const QueueIdSchema = z.string().min(1).brand("QueueId");
export type QueueId = z.output<typeof QueueIdSchema>;

export const QueueConsumerIdSchema = z.string().min(1).brand("QueueConsumerId");
export type QueueConsumerId = z.output<typeof QueueConsumerIdSchema>;

export const WorkflowIdSchema = z.string().min(1).brand("WorkflowId");
export type WorkflowId = z.output<typeof WorkflowIdSchema>;

export const Sha256DigestSchema = z.string().regex(/^[a-f0-9]{64}$/).brand("Sha256Digest");
export type Sha256Digest = z.output<typeof Sha256DigestSchema>;

export function parseAccountId(value: string): AccountId {
  return AccountIdSchema.parse(value);
}

export function parseWorkerId(value: string): WorkerId {
  return WorkerIdSchema.parse(value);
}

export function parseResourceId(value: string): ResourceId {
  return ResourceIdSchema.parse(value);
}

export function parseDurableObjectId(value: string): DurableObjectId {
  return DurableObjectIdSchema.parse(value);
}

export function parseDeploymentId(value: string): DeploymentId {
  return DeploymentIdSchema.parse(value);
}

export function parseDeploymentUploadId(value: string): DeploymentUploadId {
  return DeploymentUploadIdSchema.parse(value);
}

export function parseSha256Digest(value: string): Sha256Digest {
  return Sha256DigestSchema.parse(value);
}

export function parseQueueId(value: string): QueueId {
  return QueueIdSchema.parse(value);
}

export function parseRouteId(value: string): RouteId {
  return RouteIdSchema.parse(value);
}

export function parseQueueConsumerId(value: string): QueueConsumerId {
  return QueueConsumerIdSchema.parse(value);
}

export function parseWorkflowId(value: string): WorkflowId {
  return WorkflowIdSchema.parse(value);
}

export function parsePageCursor(value: string): PageCursor {
  return PageCursorSchema.parse(value);
}
