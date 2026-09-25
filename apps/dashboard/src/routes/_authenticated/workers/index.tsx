import { Button } from "@cloudflare/kumo/components/button";
import { LayerCard } from "@cloudflare/kumo/components/layer-card";
import { IconCode, IconTrash } from "@tabler/icons-react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { CloudflareProductIcon } from "../../../components/cloudflare-product-icons";
import {
  CatalogToolbar,
  EmptyState,
  ErrorState,
  LoadingRows,
  PageHeader,
  ResourceList,
  ResourceRow,
} from "../../../components/dashboard-page";
import { openConfirmDeleteDialog } from "../../../components/resource-dialog";
import { useAuth } from "../../../features/auth/auth-atoms";
import { useMutationFeedback } from "../../../features/toast/use-mutation-feedback";

export const Route = createFileRoute("/_authenticated/workers/")({
  validateSearch: (search: Record<string, unknown>): { q?: string } =>
    typeof search.q === "string" && search.q ? { q: search.q } : {},
  component: WorkersPage,
});

function relativeDate(value: string | undefined, now: number) {
  if (!value) return "Not deployed";
  const time = Date.parse(value);
  if (!Number.isFinite(time)) return value;
  const days = Math.floor((now - time) / 86_400_000);
  if (days < 1) return "Today";
  if (days === 1) return "Yesterday";
  return `${days} days ago`;
}

function WorkersPage() {
  const navigate = useNavigate();
  const { q: search = "" } = Route.useSearch();
  const { client, instanceId: selectedInstanceId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const enabled = client !== null && selectedInstanceId !== null;
  const [rangeEnd, setRangeEnd] = useState(() => Date.now());
  const from = rangeEnd - 24 * 60 * 60 * 1_000;

  const workers = useQuery({
    queryKey: ["cloudflare-v4", "workers", selectedInstanceId],
    queryFn: ({ signal }) =>
      client!.workers.scripts.list(
        { account_id: selectedInstanceId! },
        { signal },
      ),
    enabled,
  });
  const usage = useQuery({
    queryKey: [
      "cloudflare-v4",
      "workers",
      "usage",
      selectedInstanceId,
      rangeEnd,
    ],
    queryFn: ({ signal }) =>
      client!.openCompute.workers.observability.usage(selectedInstanceId!, {
        signal,
        query: { from, to: rangeEnd },
      }),
    enabled,
  });
  function confirmDeleteWorker(name: string) {
    openConfirmDeleteDialog({
      name,
      confirm: async () => {
        try {
          await client!.workers.scripts.delete(name, {
            account_id: selectedInstanceId!,
          });
        } catch (error) {
          feedback.failure(error, "Unable to delete the Worker.");
          throw error;
        }
        await queryClient.invalidateQueries({
          queryKey: ["cloudflare-v4", "workers", selectedInstanceId],
        });
        feedback.success("Worker deleted.");
      },
    });
  }

  const rows = useMemo(() => {
    const query = search.trim().toLowerCase();
    return (workers.data?.result ?? []).filter((worker) =>
      (worker.id ?? "").toLowerCase().includes(query),
    );
  }, [search, workers.data]);
  const countByWorker = (usage.data?.breakdown ?? []).reduce(
    (counts, item) =>
      counts.set(item.service, (counts.get(item.service) ?? 0) + item.count),
    new Map<string, number>(),
  );

  const stats = (
    <aside className="grid gap-3 md:order-2 md:content-start">
      <LayerCard className="grid gap-4 px-5 py-4">
        <div className="grid gap-1">
          <h2 className="font-semibold">Usage</h2>
          <p className="text-kumo-subtle">Last 24 hours</p>
        </div>
        <dl className="grid grid-cols-2 gap-3 md:grid-cols-1">
          <div className="bg-kumo-recessed rounded-lg px-4 py-3">
            <dt className="text-kumo-subtle">Log events</dt>
            <dd className="mt-1 text-xl font-semibold">
              {(usage.data?.events ?? 0).toLocaleString()}
            </dd>
          </div>
          <div className="bg-kumo-recessed rounded-lg px-4 py-3">
            <dt className="text-kumo-subtle">Workers</dt>
            <dd className="mt-1 text-xl font-semibold">
              {workers.data?.result.length ?? 0}
            </dd>
          </div>
        </dl>
        <Link className="text-kumo-link hover:underline" to="/observability">
          View observability
        </Link>
      </LayerCard>
      <LayerCard className="grid gap-2 px-5 py-4">
        <h2 className="font-semibold">Account details</h2>
        <span className="text-kumo-subtle">Account ID</span>
        <code className="text-xs break-all">{selectedInstanceId}</code>
      </LayerCard>
    </aside>
  );

  return (
    <>
      <PageHeader
        title="Workers"
        description="Build and deploy serverless applications."
        actions={
          <Button
            variant="primary"
            onClick={() => void navigate({ to: "/workers/new" })}
          >
            <IconCode size={16} />
            Create Worker
          </Button>
        }
      />
      <div className="grid gap-5 md:grid-cols-3">
        {stats}
        <div className="min-w-0 md:order-1 md:col-span-2">
          <CatalogToolbar
            value={search}
            onChange={(value) =>
              void navigate({
                to: "/workers",
                search: value ? { q: value } : {},
                replace: true,
              })
            }
            onRefresh={() => {
              setRangeEnd(Date.now());
              void workers.refetch();
            }}
            refreshing={workers.isFetching || usage.isFetching}
            placeholder="Search Workers"
          />
          {workers.isLoading ? (
            <LoadingRows count={5} />
          ) : workers.error ? (
            <ErrorState error={workers.error} />
          ) : rows.length === 0 ? (
            <EmptyState
              title={
                search ? "No matching Workers" : "Create your first Worker"
              }
              description={
                search
                  ? "Try another name."
                  : "Deploy a Worker to start serving requests."
              }
              action={
                search ? undefined : (
                  <Button
                    variant="primary"
                    onClick={() => void navigate({ to: "/workers/new" })}
                  >
                    Create Worker
                  </Button>
                )
              }
            />
          ) : (
            <ResourceList>
              {rows.map((worker) => {
                const name = worker.id ?? "unknown";
                return (
                  <ResourceRow
                    key={name}
                    href={`/workers/${encodeURIComponent(name)}`}
                    icon={<CloudflareProductIcon product="Workers" size={18} />}
                    title={name}
                    description={
                      worker.compatibility_date
                        ? `Compatibility date ${worker.compatibility_date}`
                        : "Worker service"
                    }
                    meta={relativeDate(worker.modified_on, rangeEnd)}
                    footer={
                      <div className="flex items-center justify-between gap-3">
                        <span>{countByWorker.get(name) ?? 0} log events</span>
                        <Button
                          variant="ghost"
                          shape="square"
                          aria-label={`Delete ${name}`}
                          onClick={() => confirmDeleteWorker(name)}
                        >
                          <IconTrash size={16} />
                        </Button>
                      </div>
                    }
                  />
                );
              })}
            </ResourceList>
          )}
        </div>
      </div>
    </>
  );
}
