import { createFileRoute, Link } from "@tanstack/react-router";
import { useQueries, useQuery } from "@tanstack/react-query";
import { Surface } from "@cloudflare/kumo/components/surface";
import { OperatorApiError } from "@open-compute/operator-sdk";
import { ResourceCountCard } from "../../components/StructuredSummary";
import { DataTable, ErrorState, LoadingState, PageHeader, SectionHeader, StatusBadge } from "../../components/PageLayout";
import { docsLinks } from "../../lib/docs";
import { catalogResourceRow, doNamespaceRow, queueRow, workflowRow } from "../../lib/catalog";
import { formatRelative, formatTimestamp } from "../../lib/format";
import { useAuth } from "../../features/auth/AuthProvider";
import { queryKeys } from "../../queries/keys";

export const Route = createFileRoute("/_authenticated/")({
  component: OverviewPage,
});

interface RecentResource {
  kind: string;
  name: string;
  id: string;
  updatedAtMs: number;
  to: string;
}

function OverviewPage() {
  const { client, accountId, clearAuth } = useAuth();
  const enabled = Boolean(client && accountId);

  const metaQuery = useQuery({
    queryKey: queryKeys.meta,
    queryFn: ({ signal }) => client!.system.meta({ signal }),
    enabled: Boolean(client),
  });
  const statusQuery = useQuery({
    queryKey: queryKeys.status,
    queryFn: ({ signal }) => client!.system.status({ signal }),
    enabled: Boolean(client),
    refetchInterval: 30_000,
  });

  const [
    workersQuery,
    kvQuery,
    d1Query,
    r2Query,
    doQuery,
    queuesQuery,
    workflowsQuery,
  ] = useQueries({
    queries: [
      {
        queryKey: queryKeys.overview.workers(accountId ?? ""),
        queryFn: ({ signal }) => client!.workers.list({ accountId: accountId!, signal, limit: 100 }),
        enabled,
      },
      {
        queryKey: queryKeys.overview.kvNamespaces(accountId ?? ""),
        queryFn: ({ signal }) => client!.kv.listNamespaces({ accountId: accountId!, signal, limit: 100 }),
        enabled,
      },
      {
        queryKey: queryKeys.overview.d1Databases(accountId ?? ""),
        queryFn: ({ signal }) => client!.d1.listDatabases({ accountId: accountId!, signal, limit: 100 }),
        enabled,
      },
      {
        queryKey: queryKeys.overview.r2Buckets(accountId ?? ""),
        queryFn: ({ signal }) => client!.r2.listBuckets({ accountId: accountId!, signal, limit: 100 }),
        enabled,
      },
      {
        queryKey: queryKeys.overview.doNamespaces(accountId ?? ""),
        queryFn: ({ signal }) => client!.durableObjects.listNamespaces({ accountId: accountId!, signal, limit: 100 }),
        enabled,
      },
      {
        queryKey: queryKeys.overview.queues(accountId ?? ""),
        queryFn: ({ signal }) => client!.queues.list({ accountId: accountId!, limit: 100, signal }),
        enabled,
      },
      {
        queryKey: queryKeys.overview.workflows(accountId ?? ""),
        queryFn: ({ signal }) => client!.workflows.list({ accountId: accountId!, limit: 100, signal }),
        enabled,
      },
    ],
  });

  if (metaQuery.error instanceof OperatorApiError && metaQuery.error.status === 401) {
    clearAuth();
  }

  const recentResources: RecentResource[] = [
    ...(workersQuery.data?.workers ?? []).map(worker => ({
      kind: "Worker",
      name: worker.name,
      id: worker.id,
      updatedAtMs: worker.updatedAtMs ?? worker.createdAtMs,
      to: `/workers/${worker.id}`,
    })),
    ...(kvQuery.data?.namespaces ?? []).map(record => {
      const row = catalogResourceRow(record);
      return {
        kind: "KV",
        name: row.name,
        id: row.id,
        updatedAtMs: row.createdAtMs,
        to: `/kv/${row.id}`,
      };
    }),
    ...(d1Query.data?.databases ?? []).map(record => {
      const row = catalogResourceRow(record);
      return {
        kind: "D1",
        name: row.name,
        id: row.id,
        updatedAtMs: row.createdAtMs,
        to: `/d1/${row.id}`,
      };
    }),
    ...(r2Query.data?.buckets ?? []).map(bucket => ({
      kind: "R2",
      name: bucket.name,
      id: bucket.resourceId,
      updatedAtMs: bucket.updatedAtMs,
      to: `/r2/${bucket.resourceId}`,
    })),
    ...(doQuery.data?.namespaces ?? []).map(namespace => {
      const row = doNamespaceRow(namespace);
      return {
        kind: "Durable Object",
        name: row.name,
        id: row.id,
        updatedAtMs: row.createdAtMs,
        to: `/durable-objects/${row.id}`,
      };
    }),
    ...(queuesQuery.data?.queues ?? []).map(queue => {
      const row = queueRow(queue);
      return {
        kind: "Queue",
        name: row.name,
        id: row.id,
        updatedAtMs: queue.updatedAtMs ?? row.createdAtMs,
        to: `/queues/${row.id}`,
      };
    }),
    ...(workflowsQuery.data?.workflows ?? []).map(workflow => {
      const row = workflowRow(workflow);
      return {
        kind: "Workflow",
        name: row.name,
        id: row.id,
        updatedAtMs: workflow.updatedAtMs ?? row.createdAtMs,
        to: `/workflows/${row.id}`,
      };
    }),
  ]
    .sort((left, right) => right.updatedAtMs - left.updatedAtMs)
    .slice(0, 8);

  const queueCount = queuesQuery.data?.queues.length ?? null;
  const workerSuffix = workersQuery.data?.listComplete === false || workersQuery.data?.cursor ? "+" : undefined;
  const kvSuffix = kvQuery.data?.listComplete === false || kvQuery.data?.cursor ? "+" : undefined;
  const d1Suffix = d1Query.data?.listComplete === false || d1Query.data?.cursor ? "+" : undefined;
  const r2Suffix = r2Query.data?.listComplete === false || r2Query.data?.cursor ? "+" : undefined;
  const doSuffix = doQuery.data?.listComplete === false || doQuery.data?.cursor ? "+" : undefined;
  const queueSuffix = queuesQuery.data?.nextCursor ? "+" : undefined;
  const workflowCount = workflowsQuery.data?.workflows.length ?? null;
  const workflowSuffix = workflowsQuery.data?.nextCursor ? "+" : undefined;

  return (
    <div>
      <PageHeader
        title="Overview"
        description="Platform readiness, release identity, and bounded resource summaries."
        docsUrl={docsLinks.overview}
      />
      <div className="grid gap-4 lg:grid-cols-3">
        <Surface className="p-5 lg:col-span-1">
          <div className="text-sm text-kumo-subtle">Release</div>
          {metaQuery.isLoading ? (
            <LoadingState label="Loading release metadata…" />
          ) : metaQuery.error ? (
            <ErrorState message="Unable to load release metadata." />
          ) : (
            <div className="mt-2 space-y-2">
              <div className="text-lg font-semibold">{metaQuery.data?.release ?? "Unknown"}</div>
              <div className="text-sm text-kumo-subtle">API {metaQuery.data?.apiVersion ?? "v1"}</div>
              <div className="flex flex-wrap gap-2">
                {metaQuery.data?.capabilities.map(capability => (
                  <StatusBadge key={capability} value={capability} />
                ))}
              </div>
            </div>
          )}
        </Surface>
        <Surface className="p-5 lg:col-span-2">
          <div className="text-sm text-kumo-subtle">System status</div>
          {statusQuery.isLoading ? (
            <LoadingState label="Loading system status…" />
          ) : statusQuery.error ? (
            <ErrorState message="Unable to load system status." />
          ) : (
            <div className="mt-2 space-y-4">
              <div className="flex items-center gap-3">
                <span className="text-lg font-semibold">Readiness</span>
                <StatusBadge value={statusQuery.data?.readiness ?? "unknown"} />
              </div>
              {statusQuery.data?.supervisor ? (
                <div className="rounded-md bg-kumo-control/40 p-4 text-sm">
                  <div>Supervisor: {statusQuery.data.supervisor.state}</div>
                  <div className="text-kumo-subtle">Reason: {statusQuery.data.supervisor.reason}</div>
                </div>
              ) : null}
            </div>
          )}
        </Surface>
      </div>

      <div className="mt-8">
        <SectionHeader title="Resources" description="Bounded catalog counts for the active account." />
        <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
          <ResourceCountCard label="Workers" count={workersQuery.data?.workers.length ?? null} isLoading={workersQuery.isLoading} error={Boolean(workersQuery.error)} to="/workers" {...(workerSuffix ? { suffix: workerSuffix } : {})} />
          <ResourceCountCard label="KV namespaces" count={kvQuery.data?.namespaces.length ?? null} isLoading={kvQuery.isLoading} error={Boolean(kvQuery.error)} to="/kv" {...(kvSuffix ? { suffix: kvSuffix } : {})} />
          <ResourceCountCard label="D1 databases" count={d1Query.data?.databases.length ?? null} isLoading={d1Query.isLoading} error={Boolean(d1Query.error)} to="/d1" {...(d1Suffix ? { suffix: d1Suffix } : {})} />
          <ResourceCountCard label="R2 buckets" count={r2Query.data?.buckets.length ?? null} isLoading={r2Query.isLoading} error={Boolean(r2Query.error)} to="/r2" {...(r2Suffix ? { suffix: r2Suffix } : {})} />
          <ResourceCountCard label="DO namespaces" count={doQuery.data?.namespaces.length ?? null} isLoading={doQuery.isLoading} error={Boolean(doQuery.error)} to="/durable-objects" {...(doSuffix ? { suffix: doSuffix } : {})} />
          <ResourceCountCard label="Queues" count={queueCount} isLoading={queuesQuery.isLoading} error={Boolean(queuesQuery.error)} to="/queues" {...(queueSuffix ? { suffix: queueSuffix } : {})} />
          <ResourceCountCard label="Workflows" count={workflowCount} isLoading={workflowsQuery.isLoading} error={Boolean(workflowsQuery.error)} to="/workflows" {...(workflowSuffix ? { suffix: workflowSuffix } : {})} />
        </div>
      </div>

      <div className="mt-8">
        <SectionHeader title="Recent resources" description="Most recently updated catalog entries across products." />
        <DataTable
          columns={[
            { key: "kind", label: "Product" },
            { key: "name", label: "Name" },
            { key: "id", label: "ID" },
            { key: "updated", label: "Updated" },
            { key: "actions", label: "" },
          ]}
          rows={recentResources.map(resource => ({
            kind: resource.kind,
            name: resource.name,
            id: <code className="[font-size:0.9em]">{resource.id}</code>,
            updated: (
              <div>
                <div>{formatRelative(resource.updatedAtMs)}</div>
                <div className="text-xs text-kumo-subtle">{formatTimestamp(resource.updatedAtMs)}</div>
              </div>
            ),
            actions: (
              <Link to={resource.to} className="text-sm text-kumo-link">
                Open
              </Link>
            ),
          }))}
          emptyLabel="Create a resource to see recent activity here."
        />
      </div>

      <div className="mt-8">
        <SectionHeader title="Components" description="Live component health reported by the Operator API." />
        {statusQuery.isLoading ? (
          <LoadingState />
        ) : (
          <DataTable
            columns={[
              { key: "name", label: "Component" },
              { key: "state", label: "State" },
              { key: "reason", label: "Reason" },
            ]}
            rows={(statusQuery.data?.components ?? []).map(component => ({
              name: component.name,
              state: <StatusBadge value={component.state} />,
              reason: component.reason ?? "—",
            }))}
            emptyLabel="No component diagnostics are available yet."
          />
        )}
      </div>
    </div>
  );
}
