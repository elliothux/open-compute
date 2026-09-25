import type { BindingProps } from "../bindings/protocol.js";

/** Queue authority pinned by a validated version descriptor. */
export interface QueueBindingProps extends BindingProps {
  instanceId: string;
  workerId: string;
  queueId: string;
  queueLifecycleGeneration: number;
}
