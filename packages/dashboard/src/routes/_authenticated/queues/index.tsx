import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { useInfiniteQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Button } from "@cloudflare/kumo/components/button";
import { OperatorApiError, parsePageCursor, parseQueueId } from "@open-compute/operator-sdk";
import { CatalogToolbar } from "../../../components/CatalogToolbar";
import { CatalogFilters, catalogSearchSchema } from "../../../components/CatalogFilters";
import { ConfirmDeleteResourceDialog } from "../../../components/ConfirmDeleteResourceDialog";
import { QueueConfigDialog, type QueueConfigInput } from "../../../components/QueueConfigDialog";
import { RenameResourceDialog } from "../../../components/RenameResourceDialog";
import { RowActionsMenu } from "../../../components/RowActionsMenu";
import { DataTable, ErrorState, LoadingState, PageHeader, StatusBadge } from "../../../components/PageLayout";
import { docsLinks } from "../../../lib/docs";
import { useMutationFeedback } from "../../../features/toast/useMutationFeedback";
import { formatTimestamp } from "../../../lib/format";
import { queueRow } from "../../../lib/catalog";
import { useAuth } from "../../../features/auth/AuthProvider";
import { queryKeys } from "../../../queries/keys";
import { invalidateQueuesQueries } from "../../../queries/invalidate";

export const Route = createFileRoute("/_authenticated/queues/")({
  validateSearch: search => catalogSearchSchema.parse(search),
  component: QueuesPage,
});

function QueuesPage() {
  const navigate = useNavigate({ from: Route.fullPath });
  const { q = "", status = "all", sort = "updatedAt", direction = "desc" } = Route.useSearch();
  const { client, accountId } = useAuth();
  const queryClient = useQueryClient();
  const [createOpen, setCreateOpen] = useState(false);
  const [renameTarget, setRenameTarget] = useState<{ id: string; name: string; configGeneration: number } | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<{ id: string; name: string; lifecycleGeneration: number } | null>(null);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const feedback = useMutationFeedback();
  const searchNeedle = q.trim();
  const catalogState = `${searchNeedle}:${status}:${sort}:${direction}`;

  const queuesQuery = useInfiniteQuery({
    queryKey: queryKeys.queues(accountId ?? "", catalogState),
    queryFn: ({ pageParam, signal }) => client!.queues.list({
      accountId: accountId!,
      ...(searchNeedle ? { search: searchNeedle } : {}),
      ...(status === "all" ? {} : { status }),
      sort,
      direction,
      ...(pageParam !== undefined ? { cursor: parsePageCursor(pageParam) } : {}),
      limit: 100,
      signal,
    }),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: lastPage => lastPage.nextCursor ?? undefined,
    enabled: Boolean(client && accountId),
  });

  const createMutation = useMutation({
    mutationFn: (input: QueueConfigInput) => client!.queues.create({
      accountId: accountId!,
      name: input.name!,
      idempotencyKey: crypto.randomUUID(),
      ...(input.deliveryDelaySeconds !== undefined ? { deliveryDelaySeconds: input.deliveryDelaySeconds } : {}),
      ...(input.retentionSeconds !== undefined ? { retentionSeconds: input.retentionSeconds } : {}),
      ...(input.maxBacklogBytes !== undefined ? { maxBacklogBytes: input.maxBacklogBytes } : {}),
    }),
    onSuccess: async () => {
      await invalidateQueuesQueries(queryClient, accountId!);
      setErrorMessage(null);
      setCreateOpen(false);
      feedback.success("Queue created.");
    },
    onError: error => {
      setErrorMessage(
        error instanceof OperatorApiError ? error.message : "Unable to create the Queue.",
      );
      feedback.failure(error, "Unable to create the Queue.");
    },
  });

  const renameMutation = useMutation({
    mutationFn: (input: { queueId: string; name: string; configGeneration: number }) => client!.queues.rename({
      accountId: accountId!,
      queueId: parseQueueId(input.queueId),
      name: input.name,
      expectedConfigGeneration: input.configGeneration,
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async () => {
      await invalidateQueuesQueries(queryClient, accountId!);
      setErrorMessage(null);
      setRenameTarget(null);
      feedback.success("Queue renamed.");
    },
    onError: error => {
      setErrorMessage(
        error instanceof OperatorApiError ? error.message : "Unable to rename the Queue.",
      );
      feedback.failure(error, "Unable to rename the Queue.");
    },
  });

  const deleteMutation = useMutation({
    mutationFn: (input: { queueId: string; lifecycleGeneration: number; force: boolean }) => client!.queues.delete({
      accountId: accountId!,
      queueId: parseQueueId(input.queueId),
      idempotencyKey: crypto.randomUUID(),
      expectedLifecycleGeneration: input.lifecycleGeneration,
      force: input.force,
    }),
    onSuccess: async () => {
      await invalidateQueuesQueries(queryClient, accountId!);
      setErrorMessage(null);
      setDeleteTarget(null);
      feedback.success("Queue deleted.");
    },
    onError: error => {
      setErrorMessage(
        error instanceof OperatorApiError ? error.message : "Unable to delete the Queue.",
      );
      feedback.failure(error, "Unable to delete the Queue.");
    },
  });

  const allQueues = useMemo(
    () => (queuesQuery.data?.pages ?? []).flatMap(page => page.queues),
    [queuesQuery.data?.pages],
  );

  const createButton = (
    <Button variant="primary" onClick={() => setCreateOpen(true)}>
      Create Queue
    </Button>
  );

  return (
    <div>
      <PageHeader
        title="Queues"
        description="Queue configuration and consumer status. Message bodies are not exposed in Day1."
        docsUrl={docsLinks.platform}
      />
      <CatalogToolbar
        search={q}
        onSearchChange={value => void navigate({ search: previous => ({ ...previous, q: value || undefined }) })}
        searchPlaceholder="Search Queues"
        onRefresh={() => void queuesQuery.refetch()}
        isRefreshing={queuesQuery.isFetching}
        filters={(
          <CatalogFilters
            status={status}
            sort={sort}
            direction={direction}
            onStatusChange={value => void navigate({ search: previous => ({ ...previous, status: value === "all" ? undefined : value }) })}
            onSortChange={(nextSort, nextDirection) => void navigate({ search: previous => ({ ...previous, sort: nextSort, direction: nextDirection }) })}
          />
        )}
        primaryAction={createButton}
      />
      <QueueConfigDialog
        mode="create"
        open={createOpen}
        errorMessage={errorMessage}
        isPending={createMutation.isPending}
        onClose={() => {
          setCreateOpen(false);
          setErrorMessage(null);
        }}
        onSubmit={input => createMutation.mutate(input)}
      />
      <RenameResourceDialog
        title="Rename Queue"
        description="Updates the Queue display name. The current config generation is sent with the rename request."
        nameLabel="Queue name"
        currentName={renameTarget?.name ?? ""}
        open={Boolean(renameTarget)}
        errorMessage={renameTarget ? errorMessage : null}
        isPending={renameMutation.isPending}
        onClose={() => {
          setRenameTarget(null);
          setErrorMessage(null);
        }}
        onSubmit={name => {
          if (!renameTarget) return;
          renameMutation.mutate({
            queueId: renameTarget.id,
            name,
            configGeneration: renameTarget.configGeneration,
          });
        }}
      />
      <ConfirmDeleteResourceDialog
        title="Delete Queue"
        description="This fences the Queue and purges retained messages before retirement."
        resourceLabel="the Queue name"
        confirmValue={deleteTarget?.name ?? ""}
        open={Boolean(deleteTarget)}
        errorMessage={deleteTarget ? errorMessage : null}
        isPending={deleteMutation.isPending}
        forceOption={{
          label: "Force delete non-empty Queue",
          description: "Purges retained messages before completing deletion when the Queue is not empty.",
        }}
        onClose={() => {
          setDeleteTarget(null);
          setErrorMessage(null);
        }}
        onConfirm={({ force }) => {
          if (!deleteTarget) return;
          deleteMutation.mutate({
            queueId: deleteTarget.id,
            lifecycleGeneration: deleteTarget.lifecycleGeneration,
            force,
          });
        }}
      />
      {queuesQuery.isLoading ? (
        <LoadingState />
      ) : queuesQuery.error ? (
        <ErrorState message="Unable to load Queues." />
      ) : (
        <>
          <DataTable
            columns={[
              { key: "name", label: "Queue" },
              { key: "id", label: "ID" },
              { key: "state", label: "State" },
              { key: "created", label: "Created" },
              { key: "actions", label: "" },
            ]}
            rows={allQueues.map(queue => {
              const row = queueRow(queue);
              return {
                name: row.name,
                id: <code className="[font-size:0.9em]">{row.id}</code>,
                state: <StatusBadge value={row.state} />,
                created: formatTimestamp(row.createdAtMs),
                actions: (
                  <RowActionsMenu
                    label={row.name}
                    actions={[
                      {
                        id: "open",
                        label: "Open",
                        onSelect: () => {
                          void navigate({ to: "/queues/$queueId", params: { queueId: row.id } });
                        },
                      },
                      {
                        id: "rename",
                        label: "Rename",
                        onSelect: () => {
                          setErrorMessage(null);
                          setRenameTarget({
                            id: row.id,
                            name: row.name,
                            configGeneration: row.configGeneration,
                          });
                        },
                      },
                      {
                        id: "delete",
                        label: "Delete",
                        variant: "danger",
                        onSelect: () => {
                          setErrorMessage(null);
                          setDeleteTarget({
                            id: row.id,
                            name: row.name,
                            lifecycleGeneration: row.lifecycleGeneration,
                          });
                        },
                      },
                    ]}
                  />
                ),
              };
            })}
            emptyLabel="Create a Queue to configure producers and consumers."
            emptyAction={createButton}
          />
          {queuesQuery.hasNextPage ? (
            <div className="mt-4 flex justify-center">
              <Button
                variant="secondary"
                disabled={queuesQuery.isFetchingNextPage}
                onClick={() => void queuesQuery.fetchNextPage()}
              >
                {queuesQuery.isFetchingNextPage ? "Loading…" : "Load more"}
              </Button>
            </div>
          ) : null}
        </>
      )}
    </div>
  );
}
