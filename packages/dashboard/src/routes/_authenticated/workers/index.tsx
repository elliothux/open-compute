import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { useInfiniteQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Button } from "@cloudflare/kumo/components/button";
import { Select } from "@cloudflare/kumo/components/select";
import { Surface } from "@cloudflare/kumo/components/surface";
import { OperatorApiError, parsePageCursor } from "@open-compute/operator-sdk";
import { z } from "zod";
import { CatalogToolbar } from "../../../components/CatalogToolbar";
import { CreateResourceDialog } from "../../../components/CreateResourceDialog";
import { RowActionsMenu } from "../../../components/RowActionsMenu";
import { DataTable, ErrorState, LoadingState, PageHeader } from "../../../components/PageLayout";
import { docsLinks } from "../../../lib/docs";
import { useMutationFeedback } from "../../../features/toast/useMutationFeedback";
import { formatRelative, formatTimestamp } from "../../../lib/format";
import { useAuth } from "../../../features/auth/AuthProvider";
import { queryKeys } from "../../../queries/keys";
import { invalidateWorkersQueries } from "../../../queries/invalidate";

const workerCatalogSearchSchema = z.object({
  q: z.string().optional(),
  deployed: z.enum(["all", "deployed", "undeployed"]).optional(),
  sort: z.enum(["name", "createdAt", "updatedAt"]).optional(),
  direction: z.enum(["asc", "desc"]).optional(),
});

const deploymentLabels = {
  all: "All deployments",
  deployed: "Deployed",
  undeployed: "Not deployed",
} as const;

const workerSortLabels = {
  "updatedAt:desc": "Recently updated",
  "createdAt:desc": "Recently created",
  "name:asc": "Name A–Z",
  "name:desc": "Name Z–A",
} as const;

export const Route = createFileRoute("/_authenticated/workers/")({
  validateSearch: search => workerCatalogSearchSchema.parse(search),
  component: WorkersPage,
});

function WorkersPage() {
  const navigate = useNavigate({ from: Route.fullPath });
  const { client, accountId, clearAuth } = useAuth();
  const { q = "", deployed = "all", sort = "updatedAt", direction = "desc" } = Route.useSearch();
  const queryClient = useQueryClient();
  const [createOpen, setCreateOpen] = useState(false);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const feedback = useMutationFeedback();
  const searchNeedle = q.trim();
  const catalogState = `${searchNeedle}:${deployed}:${sort}:${direction}`;

  const workersQuery = useInfiniteQuery({
    queryKey: queryKeys.workers(accountId ?? "", catalogState),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam, signal }) => client!.workers.list({
      accountId: accountId!,
      ...(searchNeedle ? { search: searchNeedle } : {}),
      ...(deployed === "all" ? {} : { deployed: deployed === "deployed" }),
      sort,
      direction,
      ...(pageParam !== undefined ? { cursor: parsePageCursor(pageParam) } : {}),
      limit: 100,
      signal,
    }),
    getNextPageParam: lastPage => (lastPage.listComplete ? undefined : lastPage.cursor ?? undefined),
    enabled: Boolean(client && accountId),
    refetchOnMount: "always",
  });

  const createMutation = useMutation({
    mutationFn: (name: string) => client!.workers.create({
      accountId: accountId!,
      name,
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async () => {
      await invalidateWorkersQueries(queryClient, accountId!);
      setErrorMessage(null);
      setCreateOpen(false);
      feedback.success("Worker created.");
    },
    onError: error => {
      setErrorMessage(
        error instanceof OperatorApiError ? error.message : "Unable to create the Worker.",
      );
      feedback.failure(error, "Unable to create the Worker.");
    },
  });

  if (workersQuery.error instanceof OperatorApiError && workersQuery.error.status === 401) {
    clearAuth();
  }

  const workers = useMemo(
    () => (workersQuery.data?.pages ?? []).flatMap(page => page.workers),
    [workersQuery.data?.pages],
  );
  const usage = useMemo(() => workers.reduce(
    (total, worker) => ({
      requests: total.requests + (worker.traffic?.requests ?? 0),
      errors: total.errors + (worker.traffic?.errors ?? 0),
      latencyTotal: total.latencyTotal
        + (worker.traffic?.averageLatencyMs ?? 0) * (worker.traffic?.requests ?? 0),
    }),
    { requests: 0, errors: 0, latencyTotal: 0 },
  ), [workers]);

  const createButton = (
    <Button variant="primary" onClick={() => setCreateOpen(true)}>
      Create Worker
    </Button>
  );

  return (
    <div>
      <PageHeader
        title="Workers"
        description="Browse Workers, active deployments, and routes. Build and deploy still happen through oc."
        docsUrl={docsLinks.workers}
      />
      <CatalogToolbar
        search={q}
        onSearchChange={value => {
          void navigate({ search: previous => ({ ...previous, q: value || undefined }) });
        }}
        searchPlaceholder="Search Workers"
        onRefresh={() => void workersQuery.refetch()}
        isRefreshing={workersQuery.isFetching}
        filters={(
          <>
            <Select
              aria-label="Deployment status"
              value={deployed}
              renderValue={value => deploymentLabels[value] ?? value}
              onValueChange={value => {
                if (!value) return;
                void navigate({ search: previous => ({ ...previous, deployed: value === "all" ? undefined : value }) });
              }}
            >
              <Select.Option value="all">All deployments</Select.Option>
              <Select.Option value="deployed">Deployed</Select.Option>
              <Select.Option value="undeployed">Not deployed</Select.Option>
            </Select>
            <Select
              aria-label="Sort Workers"
              value={`${sort}:${direction}`}
              renderValue={value => workerSortLabels[value as keyof typeof workerSortLabels] ?? value}
              onValueChange={value => {
                if (!value) return;
                const [nextSort, nextDirection] = value.split(":") as [typeof sort, typeof direction];
                void navigate({ search: previous => ({ ...previous, sort: nextSort, direction: nextDirection }) });
              }}
            >
              <Select.Option value="updatedAt:desc">Recently updated</Select.Option>
              <Select.Option value="createdAt:desc">Recently created</Select.Option>
              <Select.Option value="name:asc">Name A–Z</Select.Option>
              <Select.Option value="name:desc">Name Z–A</Select.Option>
            </Select>
          </>
        )}
        primaryAction={createButton}
      />
      <CreateResourceDialog
        title="Create Worker"
        description="Workers are account-scoped compute resources. Deployments and routes are managed from the Worker detail page."
        nameLabel="Worker name"
        namePlaceholder="my-worker"
        submitLabel="Create Worker"
        open={createOpen}
        errorMessage={errorMessage}
        isPending={createMutation.isPending}
        onClose={() => {
          setCreateOpen(false);
          setErrorMessage(null);
        }}
        onSubmit={name => createMutation.mutate(name)}
      />
      {workersQuery.isLoading ? (
        <LoadingState />
      ) : workersQuery.error ? (
        <ErrorState message="Unable to load Workers." />
      ) : (
        <>
          <div className="grid items-start gap-4 xl:grid-cols-[minmax(0,1fr)_16rem]">
            <DataTable
              columns={[
                { key: "name", label: "Name" },
                { key: "route", label: "Route", className: "hidden lg:table-cell" },
                { key: "source", label: "Source", className: "hidden xl:table-cell" },
                { key: "traffic", label: "Requests", className: "hidden md:table-cell" },
                { key: "latency", label: "Avg latency", className: "hidden lg:table-cell" },
                { key: "updated", label: "Updated" },
                { key: "actions", label: "" },
              ]}
              rows={workers.map(worker => ({
              name: (
                <div>
                  <div>{worker.name}</div>
                  <code className="mt-1 block [font-size:0.9em] text-kumo-subtle">{worker.id.slice(0, 8)}…</code>
                </div>
              ),
              route: worker.primaryRoute
                ? `${worker.primaryRoute.hostnameAscii ?? "platform"}${worker.primaryRoute.pathPrefix}`
                : "—",
              source: worker.deploymentSource === "operator_api" ? "Operator API" : "Not deployed",
              traffic: (worker.traffic?.requests ?? 0).toLocaleString(),
              latency: worker.traffic?.requests
                ? `${worker.traffic.averageLatencyMs.toFixed(1)} ms`
                : "—",
              updated: (
                <div>
                  <div>{formatRelative(worker.updatedAtMs ?? worker.createdAtMs)}</div>
                  <div className="text-xs text-kumo-subtle">{formatTimestamp(worker.updatedAtMs ?? worker.createdAtMs)}</div>
                </div>
              ),
              actions: (
                <RowActionsMenu
                  label={worker.name}
                  actions={[
                    {
                      id: "open",
                      label: "Open",
                      onSelect: () => {
                        void navigate({ to: "/workers/$workerId", params: { workerId: worker.id } });
                      },
                    },
                  ]}
                />
              ),
              }))}
              emptyLabel="Create a Worker to start deploying scripts."
              emptyAction={createButton}
            />
            <Surface className="p-4 xl:sticky xl:top-24">
              <h2 className="text-base font-semibold">Usage since startup</h2>
              <p className="mt-1 text-sm text-kumo-subtle">
                Live ingress totals for the Workers loaded in this catalog.
              </p>
              <dl className="mt-4 space-y-3">
                <div>
                  <dt className="text-xs text-kumo-subtle">Requests</dt>
                  <dd className="mt-1 text-lg font-semibold">{usage.requests.toLocaleString()}</dd>
                </div>
                <div>
                  <dt className="text-xs text-kumo-subtle">Errors</dt>
                  <dd className="mt-1 text-lg font-semibold">{usage.errors.toLocaleString()}</dd>
                </div>
                <div>
                  <dt className="text-xs text-kumo-subtle">Average latency</dt>
                  <dd className="mt-1 text-lg font-semibold">
                    {usage.requests ? `${(usage.latencyTotal / usage.requests).toFixed(1)} ms` : "—"}
                  </dd>
                </div>
              </dl>
            </Surface>
          </div>
          {workersQuery.hasNextPage ? (
            <div className="mt-4 flex justify-center">
              <Button
                variant="secondary"
                disabled={workersQuery.isFetchingNextPage}
                onClick={() => void workersQuery.fetchNextPage()}
              >
                {workersQuery.isFetchingNextPage ? "Loading…" : "Load more"}
              </Button>
            </div>
          ) : null}
        </>
      )}
    </div>
  );
}
