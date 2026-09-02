import type { D1DatabaseRecord, DoNamespace, KvNamespaceRecord, Queue, Workflow } from "@open-compute/operator-sdk";

export function catalogResourceRow(record: KvNamespaceRecord | D1DatabaseRecord) {
  return {
    id: record.resource.id,
    name: record.resource.name,
    state: record.resource.state,
    createdAtMs: record.resource.createdAtMs,
  };
}

export function doNamespaceRow(namespace: DoNamespace) {
  return {
    id: namespace.resourceId,
    name: namespace.name,
    className: namespace.className,
    ownerWorkerId: namespace.ownerWorkerId,
    state: namespace.state,
    createdAtMs: namespace.createdAtMs,
  };
}

export function queueRow(queue: Queue) {
  return {
    id: queue.id,
    name: queue.name,
    state: queue.state,
    configGeneration: queue.configGeneration,
    lifecycleGeneration: queue.lifecycleGeneration,
    createdAtMs: queue.createdAtMs,
  };
}

export function workflowRow(workflow: Workflow) {
  return {
    id: workflow.id,
    name: workflow.name,
    state: workflow.state,
    createdAtMs: workflow.createdAtMs,
  };
}
