/** Tenant-visible event data, separate from private run identity. */
export interface WorkflowEventWire {
  externalInstanceId: string;
  definitionName: string;
  createdAtMs: number;
  payloadJson: string;
}
export interface WorkflowRunIdentity { instanceId: string; instanceGeneration: number; runToken: string }
export interface WorkflowActivation extends WorkflowEventWire, WorkflowRunIdentity {
  activationBudgetMs: number;
  versionDescriptorSha256?: string;
}
export interface WorkflowClass {
  new(ctx: ExecutionContext, env: Record<string, unknown>): {
    run(event: { payload: unknown; timestamp: Date; instanceId: string; workflowName: string }, step: object): Promise<unknown>;
  };
}
export type WorkflowRunResult =
  | { outcome: "complete"; outputJson: string; finalOrdinal: number }
  | { outcome: "errored"; errorCode: string; error?: { name: string; message: string }; finalOrdinal: number }
  | { outcome: "suspended"; finalOrdinal: number };
export interface WorkflowProtocolError { errorCode: string; state?: never; steps?: never; ok?: never }
export interface WorkflowSuspended { state: "suspended"; errorCode?: never; steps?: never; ok?: never }
export type WorkflowVerdict = WorkflowProtocolError | WorkflowSuspended
  | { state: "complete"; outputJson?: string | undefined; errorCode?: never }
  | { state: "failed"; code: string; errorCode?: never };
export interface WorkflowResolvedConfig {
  retries: { limit: number; delay: number; backoff: "constant" | "linear" | "exponential" };
  timeout: number;
}
export type WorkflowCallback<C = WorkflowResolvedConfig> = (context: {
  step: { name: string; count: number }; attempt: number; config: C;
}) => unknown;
export interface WorkflowDeclaration {
  ordinal: number;
  kind: "do" | "sleep" | "sleep_until" | "wait_event";
  name: string;
  nameCount: number;
  config: unknown;
  dependencies: number[];
}
export interface WorkflowClaimDeclaration extends WorkflowDeclaration { batchFirstOrdinal: number; batchSize: number }
export type WorkflowBatchStep = { ordinal: number } & (
  | { state: "run"; attempt: number; config: WorkflowResolvedConfig }
  | { state: "complete" | "failed" | "suspended" }
);
export type WorkflowBatchReply = WorkflowProtocolError | WorkflowSuspended
  | { steps: WorkflowBatchStep[]; state?: never; errorCode?: never };
export type WorkflowDrainReply = WorkflowProtocolError | WorkflowSuspended | { ok: true; state?: never; errorCode?: never };
export interface WorkflowControllerV2 {
  claimBatch(body: { steps: WorkflowClaimDeclaration[] }): Promise<WorkflowBatchReply>;
  success(body: { ordinal: number; outputJson: string }): Promise<WorkflowVerdict>;
  failure(body: { ordinal: number; code: string }): Promise<WorkflowVerdict>;
  result(ordinal: number): Promise<WorkflowVerdict>;
  drain(): Promise<WorkflowDrainReply>;
  registerWait(body: WorkflowClaimDeclaration): Promise<WorkflowVerdict>;
}
export interface WorkflowClaimV1 { ordinal: number; name: string; nameCount: number; configJson: "null" }
export type WorkflowClaimReplyV1 = WorkflowProtocolError
  | { state: "run"; errorCode?: never }
  | { state: "complete"; outputJson: string; errorCode?: never }
  | { state: "failed"; errorCode?: string | undefined; error?: { name: string; message: string } | undefined };
export type WorkflowCommitReplyV1 = WorkflowProtocolError | { ok: true; errorCode?: never };
export interface WorkflowFailureV1 { ordinal: number; error: { name: string; message: string }; errorCode?: string }
export interface WorkflowControllerV1 {
  claim(body: WorkflowClaimV1): Promise<WorkflowClaimReplyV1>;
  success(body: { ordinal: number; outputJson: string }): Promise<WorkflowCommitReplyV1>;
  failure(body: WorkflowFailureV1): Promise<WorkflowCommitReplyV1>;
}
export interface LoadedWorkflow extends Rpc.WorkerEntrypointBranded {
  validate(): Promise<boolean>;
  execute(event: WorkflowEventWire, controller: WorkflowControllerV1 | WorkflowControllerV2): Promise<WorkflowRunResult>;
}
