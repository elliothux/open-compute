import type { QueryClient } from "@tanstack/react-query";
import { queryKeys } from "./keys";

async function invalidateCatalogPrefix(
  queryClient: QueryClient,
  catalogPrefix: readonly unknown[],
  overviewKey: readonly unknown[],
) {
  await Promise.all([
    queryClient.invalidateQueries({ queryKey: catalogPrefix }),
    queryClient.invalidateQueries({ queryKey: overviewKey }),
  ]);
}

export function invalidateWorkersQueries(queryClient: QueryClient, accountId: string) {
  return invalidateCatalogPrefix(
    queryClient,
    ["operator", "workers", accountId],
    queryKeys.overview.workers(accountId),
  );
}

export function invalidateKvNamespacesQueries(queryClient: QueryClient, accountId: string) {
  return invalidateCatalogPrefix(
    queryClient,
    ["operator", "kv", accountId, "namespaces"],
    queryKeys.overview.kvNamespaces(accountId),
  );
}

export function invalidateD1DatabasesQueries(queryClient: QueryClient, accountId: string) {
  return invalidateCatalogPrefix(
    queryClient,
    ["operator", "d1", accountId, "databases"],
    queryKeys.overview.d1Databases(accountId),
  );
}

export function invalidateR2BucketsQueries(queryClient: QueryClient, accountId: string) {
  return invalidateCatalogPrefix(
    queryClient,
    ["operator", "r2", accountId, "buckets"],
    queryKeys.overview.r2Buckets(accountId),
  );
}

export function invalidateDoNamespacesQueries(queryClient: QueryClient, accountId: string) {
  return invalidateCatalogPrefix(
    queryClient,
    ["operator", "do", accountId, "namespaces"],
    queryKeys.overview.doNamespaces(accountId),
  );
}

export function invalidateQueuesQueries(queryClient: QueryClient, accountId: string) {
  return invalidateCatalogPrefix(
    queryClient,
    ["operator", "queues", accountId],
    queryKeys.overview.queues(accountId),
  );
}

export function invalidateWorkflowsQueries(queryClient: QueryClient, accountId: string) {
  return invalidateCatalogPrefix(
    queryClient,
    ["operator", "workflows", accountId],
    queryKeys.overview.workflows(accountId),
  );
}
