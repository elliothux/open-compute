import type { BindingProps } from "../bindings/protocol.js";

/** Workflow caller capability and its Durable Object output-gate restriction. */
export interface WorkflowBindingProps extends BindingProps { durableObject: boolean }

/** Sanitized instance status returned by the authoritative Workflow service. */
export interface WorkflowStatus {
  status: string;
  output?: unknown;
  error?: { name: string; message: string };
}

/** Stable create envelope. Tenant values are durable-value bytes, never JSON. */
export interface WorkflowCreateWire {
  id?: string;
  payloadBase64: string;
  retention?: unknown;
  locationHint?: string;
  schedule?: { cron: string; scheduledTime: number };
}

/** Instance-scoped RPC object; only the system isolate knows its UUID. */
export interface WorkflowHandle {
  status(): Promise<WorkflowStatus>;
  pause(operationId: string): Promise<unknown>;
  resume(operationId: string): Promise<unknown>;
  terminate(options: { rollback?: boolean }, operationId: string): Promise<unknown>;
  restart(options: { from?: { name: string; count?: number; type?: "do" | "sleep" | "waitForEvent" } }, operationId: string): Promise<unknown>;
  delete(operationId: string): Promise<unknown>;
  sendEvent(body: { type: string; payloadBase64: string }, operationId: string): Promise<unknown>;
}

/** Validated binding result without generation credentials. */
export interface WorkflowResolvedInstance {
  id: string;
  handle: WorkflowHandle;
}

/** Tenant-facing durable Workflow binding transport. */
export interface WorkflowTransport {
  resolve(id: string): WorkflowResolvedInstance;
  create(body: WorkflowCreateWire, operationId: string): Promise<WorkflowResolvedInstance>;
  get(id: string): Promise<WorkflowResolvedInstance>;
  createBatch(body: WorkflowCreateWire[], operationId: string): Promise<WorkflowResolvedInstance[]>;
  deleteBatch(instanceIds: string[], operationId: string): Promise<{
    deleted: { id: string }[];
    errors: { id: string; code: number; message: string }[];
  }>;
}
