import { queryOptions, skipToken } from "@tanstack/react-query";
import type { ManagementClient } from "./cloudflare";

export const capabilityQuery = (
  client: ManagementClient | null,
  instanceId: string | null,
) =>
  queryOptions({
    queryKey: ["open-compute", "capabilities", instanceId] as const,
    queryFn:
      client && instanceId
        ? ({ signal }) =>
            client.openCompute.capabilities.getForAccount(instanceId, {
              signal,
            })
        : skipToken,
  });

export const r2BucketQuery = (
  client: ManagementClient | null,
  instanceId: string | null,
  bucketId: string,
) =>
  queryOptions({
    queryKey: ["cloudflare-v4", "r2", instanceId, bucketId] as const,
    queryFn:
      client && instanceId
        ? ({ signal }) =>
            client.r2.buckets.get(
              bucketId,
              { account_id: instanceId },
              { signal },
            )
        : skipToken,
  });

export const workerDeploymentsQuery = (
  client: ManagementClient | null,
  instanceId: string | null,
  workerId: string,
) =>
  queryOptions({
    queryKey: [
      "cloudflare-v4",
      "workers",
      instanceId,
      workerId,
      "deployments",
    ] as const,
    queryFn:
      client && instanceId
        ? ({ signal }) =>
            client.workers.scripts.deployments.list(
              workerId,
              { account_id: instanceId },
              { signal },
            )
        : skipToken,
  });

export const d1DatabaseQuery = (
  client: ManagementClient | null,
  instanceId: string | null,
  databaseId: string,
) =>
  queryOptions({
    queryKey: ["cloudflare-v4", "d1", instanceId, databaseId] as const,
    queryFn:
      client && instanceId
        ? ({ signal }) =>
            client.d1.database.get(
              databaseId,
              { account_id: instanceId },
              { signal },
            )
        : skipToken,
  });

export const kvNamespaceQuery = (
  client: ManagementClient | null,
  instanceId: string | null,
  namespaceId: string,
) =>
  queryOptions({
    queryKey: ["cloudflare-v4", "kv", instanceId, namespaceId] as const,
    queryFn:
      client && instanceId
        ? ({ signal }) =>
            client.kv.namespaces.get(
              namespaceId,
              { account_id: instanceId },
              { signal },
            )
        : skipToken,
  });

export const queueQuery = (
  client: ManagementClient | null,
  instanceId: string | null,
  queueId: string,
) =>
  queryOptions({
    queryKey: ["cloudflare-v4", "queues", instanceId, queueId] as const,
    queryFn:
      client && instanceId
        ? ({ signal }) =>
            client.queues.get(queueId, { account_id: instanceId }, { signal })
        : skipToken,
  });

export const workflowQuery = (
  client: ManagementClient | null,
  instanceId: string | null,
  workflowId: string,
) =>
  queryOptions({
    queryKey: ["cloudflare-v4", "workflows", instanceId, workflowId] as const,
    queryFn:
      client && instanceId
        ? ({ signal }) =>
            client.workflows.get(
              workflowId,
              { account_id: instanceId },
              { signal },
            )
        : skipToken,
  });
