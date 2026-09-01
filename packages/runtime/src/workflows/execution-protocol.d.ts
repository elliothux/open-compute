/** Tenant-visible event data, separate from private run identity. */
export interface WorkflowEventWire {
  externalInstanceId: string;
  definitionName: string;
  createdAtMs: number;
  payloadBase64: string;
  rollback: boolean;
  schedule?: { cron: string; scheduledTime: number };
}
export interface WorkflowRunIdentity { instanceId: string; instanceGeneration: number; runToken: string }
export interface WorkflowActivation extends WorkflowEventWire, WorkflowRunIdentity {
  activationBudgetMs: number;
  versionDescriptorSha256: string;
}
export interface WorkflowClass {
  new(ctx: ExecutionContext, env: Record<string, unknown>): {
    run(event: { payload: unknown; timestamp: Date; instanceId: string; workflowName: string;
      schedule?: { cron: string; scheduledTime: number } }, step: object): Promise<unknown>;
  };
}
export type WorkflowRunResult =
  | { outcome: "complete"; outputBase64: string; finalOrdinal: number }
  | { outcome: "errored"; errorCode: string; error?: { name: string; message: string }; finalOrdinal: number }
  | { outcome: "terminated"; finalOrdinal: number }
  | { outcome: "suspended"; finalOrdinal: number };
export interface WorkflowProtocolError { errorCode: string; state?: never; steps?: never; ok?: never }
export interface WorkflowSuspended { state: "suspended"; errorCode?: never; steps?: never; ok?: never }
export type WorkflowVerdict = WorkflowProtocolError | WorkflowSuspended
  | { state: "complete"; outputBase64?: string | undefined; errorCode?: never }
  | { state: "event"; type: string; payloadBase64: string; timestampMs: number; errorCode?: never }
  | { state: "failed"; code: string; errorCode?: never }
  | { state: "resolve_delay"; attempt: number; code: string; config: WorkflowResolvedConfig; errorCode?: never };
export interface WorkflowResolvedConfig {
  retries: { limit: number; delay?: number; backoff: "constant" | "linear" | "exponential" };
  timeout: number;
  sensitive?: "output";
}
export interface WorkflowCallbackConfig {
  retries: { limit: number; delay?: number; backoff: "constant" | "linear" | "exponential" };
  timeout: number;
  sensitive?: "output";
}
export type WorkflowCallback<C = WorkflowCallbackConfig> = (context: {
  step: { name: string; count: number }; attempt: number; config: C;
}) => unknown;
export interface WorkflowDeclaration {
  ordinal: number;
  kind: "do" | "sleep" | "sleep_until" | "wait_event";
  name: string;
  nameCount: number;
  config: unknown;
  rollbackConfig?: unknown;
  rollbackStep: boolean;
  dependencies: number[];
}
export interface WorkflowClaimDeclaration extends WorkflowDeclaration { batchFirstOrdinal: number; batchSize: number }
export type WorkflowBatchStep = { ordinal: number } & (
  | { state: "run"; attempt: number; config: WorkflowResolvedConfig }
  | { state: "resolve_delay"; attempt: number; code: string; config: WorkflowResolvedConfig }
  | { state: "complete"; attempt?: number; config?: WorkflowResolvedConfig }
  | { state: "rollback_boundary"; rollbackOrdinal: number }
  | { state: "failed" | "suspended" }
);
export type WorkflowBatchReply = WorkflowProtocolError | WorkflowSuspended
  | { steps: WorkflowBatchStep[]; state?: never; errorCode?: never };
export type WorkflowDrainReply = WorkflowProtocolError | WorkflowSuspended | { ok: true; state?: never; errorCode?: never };
export interface WorkflowController {
  claimBatch(body: { steps: WorkflowClaimDeclaration[] }): Promise<WorkflowBatchReply>;
  success(body: { ordinal: number; outputBase64: string }): Promise<WorkflowVerdict>;
  failure(body: { ordinal: number; code: string; resolvedDelayMs?: number }): Promise<WorkflowVerdict>;
  resolveDelay(body: { ordinal: number; attempt: number; code: string; resolvedDelayMs?: number }): Promise<WorkflowVerdict>;
  result(ordinal: number): Promise<WorkflowVerdict>;
  drain(): Promise<WorkflowDrainReply>;
}
export interface LoadedWorkflow extends Rpc.WorkerEntrypointBranded {
  validate(): Promise<boolean>;
  execute(event: WorkflowEventWire, controller: WorkflowController): Promise<WorkflowRunResult>;
}
