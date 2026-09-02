import { z } from "zod";
import { AccountIdSchema } from "./ids.js";

export const MetaResponseSchema = z.strictObject({
  release: z.string(),
  apiVersion: z.literal("v1"),
  dashboardAssetsSha256: z.string().optional(),
  capabilities: z.array(z.string()),
});

export type MetaResponse = z.output<typeof MetaResponseSchema>;

export const AccountResponseSchema = z.strictObject({
  accountId: AccountIdSchema,
});

export type AccountResponse = z.output<typeof AccountResponseSchema>;

export const ReadinessReasonSchema = z.enum([
  "ready",
  "starting",
  "draining",
  "stopped",
  "degraded",
  "unavailable",
]);

export const ComponentStatusSchema = z.strictObject({
  name: z.string(),
  state: z.string(),
  reason: z.string().optional(),
});

export const SupervisorStatusSchema = z.strictObject({
  state: z.string(),
  reason: z.string(),
  attempt: z.number(),
});

export const SystemStatusResponseSchema = z.strictObject({
  readiness: z.string(),
  components: z.array(ComponentStatusSchema),
  supervisor: SupervisorStatusSchema.nullable().optional(),
});

export type SystemStatusResponse = z.output<typeof SystemStatusResponseSchema>;
