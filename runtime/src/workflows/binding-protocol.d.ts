import type { BindingProps } from "../bindings/protocol.js";

/** Workflow caller capability and its Durable Object output-gate restriction. */
export interface WorkflowBindingProps extends BindingProps { durableObject: boolean }

/** Sanitized instance status returned by the authoritative Workflow service. */
export interface WorkflowStatus {
  status: string;
  output?: unknown;
  error?: { name: string; message: string };
}

/** Tenant-facing capability-one transport; it never carries a private token. */
export interface WorkflowTransport {
  create(body: { id: string | undefined; payloadJson: string }): Promise<{ id: string }>;
  get(id: string): Promise<unknown>;
  status(id: string): Promise<WorkflowStatus>;
}

/** Instance-scoped capability-two RPC object; only the system isolate knows its UUID. */
export interface WorkflowHandle {
  status(): Promise<WorkflowStatus>;
  pause(): Promise<unknown>;
  resume(): Promise<unknown>;
  terminate(): Promise<unknown>;
  restart(): Promise<unknown>;
  sendEvent(body: { type: string; payloadJson: string }): Promise<unknown>;
}

/** Validated binding result without generation credentials. */
export interface WorkflowResolvedInstance {
  id: string;
  handle: WorkflowHandle;
}

/** Tenant-facing durable Workflow binding transport. */
export interface WorkflowTransportV2 {
  create(body: { id: string | undefined; payloadJson: string; retention: unknown }): Promise<WorkflowResolvedInstance>;
  get(id: string): Promise<WorkflowResolvedInstance>;
}
