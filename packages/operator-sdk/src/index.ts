import { OperatorTransport, type OperatorTransportOptions } from "./transport.js";
import {
  createD1Resource,
  createDurableObjectsResource,
  createKvResource,
  createPlatformResource,
  createQueuesResource,
  createR2Resource,
  createSystemResource,
  createWorkersResource,
  createWorkflowsResource,
} from "./resources/index.js";

export interface OperatorClientOptions extends OperatorTransportOptions {}

export function createOperatorClient(options: OperatorClientOptions) {
  const transport = new OperatorTransport(options);
  return {
    system: createSystemResource(transport),
    workers: createWorkersResource(transport),
    kv: createKvResource(transport),
    d1: createD1Resource(transport),
    r2: createR2Resource(transport),
    durableObjects: createDurableObjectsResource(transport),
    queues: createQueuesResource(transport),
    workflows: createWorkflowsResource(transport),
    platform: createPlatformResource(transport),
  };
}

export type OperatorClient = ReturnType<typeof createOperatorClient>;

export { OperatorApiError, OperatorProtocolError, readBoundedStreamBytes } from "./error.js";
export type { OperatorBinaryResponse, OperatorRequestBody, OperatorTransportOptions } from "./transport.js";
export * from "./schemas/ids.js";
export * from "./schemas/inputs.js";
export * from "./schemas/common.js";
export * from "./schemas/system.js";
export * from "./schemas/workers.js";
export * from "./schemas/storage.js";
export * from "./schemas/platform.js";
