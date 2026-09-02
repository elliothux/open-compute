import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { useInfiniteQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Button } from "@cloudflare/kumo/components/button";
import { OperatorApiError, parsePageCursor, parseWorkflowId } from "@open-compute/operator-sdk";
import { CatalogToolbar } from "../../../components/CatalogToolbar";
import { CatalogFilters, catalogSearchSchema } from "../../../components/CatalogFilters";
import { ConfirmDeleteResourceDialog } from "../../../components/ConfirmDeleteResourceDialog";
import { CreateResourceDialog } from "../../../components/CreateResourceDialog";
import { RenameResourceDialog } from "../../../components/RenameResourceDialog";
import { RowActionsMenu } from "../../../components/RowActionsMenu";
import { DataTable, ErrorState, LoadingState, PageHeader, StatusBadge } from "../../../components/PageLayout";
import { docsLinks } from "../../../lib/docs";
import { useMutationFeedback } from "../../../features/toast/useMutationFeedback";
import { formatTimestamp } from "../../../lib/format";
import { workflowRow } from "../../../lib/catalog";
import { useAuth } from "../../../features/auth/AuthProvider";
import { queryKeys } from "../../../queries/keys";
import { invalidateWorkflowsQueries } from "../../../queries/invalidate";

export const Route = createFileRoute("/_authenticated/workflows/")({
  validateSearch: search => catalogSearchSchema.parse(search),
  component: WorkflowsPage,
});

function WorkflowsPage() {
  const navigate = useNavigate({ from: Route.fullPath });
  const { q = "", status = "all", sort = "updatedAt", direction = "desc" } = Route.useSearch();
  const { client, accountId } = useAuth();
  const queryClient = useQueryClient();
  const [createOpen, setCreateOpen] = useState(false);
  const [renameTarget, setRenameTarget] = useState<{ id: string; name: string } | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<{ id: string; name: string } | null>(null);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const feedback = useMutationFeedback();
  const searchNeedle = q.trim();
  const catalogState = `${searchNeedle}:${status}:${sort}:${direction}`;

  const workflowsQuery = useInfiniteQuery({
    queryKey: queryKeys.workflows(accountId ?? "", catalogState),
    queryFn: ({ pageParam, signal }) => client!.workflows.list({
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
    mutationFn: (name: string) => client!.workflows.create({
      accountId: accountId!,
      name,
    }),
    onSuccess: async () => {
      await invalidateWorkflowsQueries(queryClient, accountId!);
      setErrorMessage(null);
      setCreateOpen(false);
      feedback.success("Workflow created.");
    },
    onError: error => {
      setErrorMessage(
        error instanceof OperatorApiError ? error.message : "Unable to create the Workflow.",
      );
      feedback.failure(error, "Unable to create the Workflow.");
    },
  });

  const renameMutation = useMutation({
    mutationFn: (input: { workflowId: string; name: string }) => client!.workflows.rename({
      accountId: accountId!,
      workflowId: parseWorkflowId(input.workflowId),
      name: input.name,
    }),
    onSuccess: async () => {
      await invalidateWorkflowsQueries(queryClient, accountId!);
      setErrorMessage(null);
      setRenameTarget(null);
      feedback.success("Workflow renamed.");
    },
    onError: error => {
      setErrorMessage(
        error instanceof OperatorApiError ? error.message : "Unable to rename the Workflow.",
      );
      feedback.failure(error, "Unable to rename the Workflow.");
    },
  });

  const deleteMutation = useMutation({
    mutationFn: (workflowId: string) => client!.workflows.delete({
      accountId: accountId!,
      workflowId: parseWorkflowId(workflowId),
    }),
    onSuccess: async () => {
      await invalidateWorkflowsQueries(queryClient, accountId!);
      setErrorMessage(null);
      setDeleteTarget(null);
      feedback.success("Workflow deleted.");
    },
    onError: error => {
      setErrorMessage(
        error instanceof OperatorApiError ? error.message : "Unable to delete the Workflow.",
      );
      feedback.failure(error, "Unable to delete the Workflow.");
    },
  });

  const allWorkflows = useMemo(
    () => (workflowsQuery.data?.pages ?? []).flatMap(page => page.workflows),
    [workflowsQuery.data?.pages],
  );

  const createButton = (
    <Button variant="primary" onClick={() => setCreateOpen(true)}>
      Create Workflow
    </Button>
  );

  return (
    <div>
      <PageHeader
        title="Workflows"
        description="Workflow definitions and instances with bounded operator history."
        docsUrl={docsLinks.platform}
      />
      <CatalogToolbar
        search={q}
        onSearchChange={value => void navigate({ search: previous => ({ ...previous, q: value || undefined }) })}
        searchPlaceholder="Search Workflows"
        onRefresh={() => void workflowsQuery.refetch()}
        isRefreshing={workflowsQuery.isFetching}
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
      <CreateResourceDialog
        title="Create Workflow"
        description="Workflow definitions are account-scoped catalog resources. Versions bind to ready deployments."
        nameLabel="Workflow name"
        namePlaceholder="ingest"
        submitLabel="Create Workflow"
        open={createOpen}
        errorMessage={errorMessage}
        isPending={createMutation.isPending}
        onClose={() => {
          setCreateOpen(false);
          setErrorMessage(null);
        }}
        onSubmit={name => createMutation.mutate(name)}
      />
      <RenameResourceDialog
        title="Rename Workflow"
        description="Updates the Workflow definition display name."
        nameLabel="Workflow name"
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
          renameMutation.mutate({ workflowId: renameTarget.id, name });
        }}
      />
      <ConfirmDeleteResourceDialog
        title="Delete Workflow"
        description="This removes the Workflow definition when no active referrers remain."
        resourceLabel="the Workflow name"
        confirmValue={deleteTarget?.name ?? ""}
        open={Boolean(deleteTarget)}
        errorMessage={deleteTarget ? errorMessage : null}
        isPending={deleteMutation.isPending}
        onClose={() => {
          setDeleteTarget(null);
          setErrorMessage(null);
        }}
        onConfirm={() => {
          if (!deleteTarget) return;
          deleteMutation.mutate(deleteTarget.id);
        }}
      />
      {workflowsQuery.isLoading ? (
        <LoadingState />
      ) : workflowsQuery.error ? (
        <ErrorState message="Unable to load Workflows." />
      ) : (
        <>
          <DataTable
            columns={[
              { key: "name", label: "Workflow" },
              { key: "id", label: "ID" },
              { key: "state", label: "State" },
              { key: "created", label: "Created" },
              { key: "actions", label: "" },
            ]}
            rows={allWorkflows.map(workflow => {
              const row = workflowRow(workflow);
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
                          void navigate({ to: "/workflows/$workflowId", params: { workflowId: row.id } });
                        },
                      },
                      {
                        id: "rename",
                        label: "Rename",
                        onSelect: () => {
                          setErrorMessage(null);
                          setRenameTarget({ id: row.id, name: row.name });
                        },
                      },
                      {
                        id: "delete",
                        label: "Delete",
                        variant: "danger",
                        onSelect: () => {
                          setErrorMessage(null);
                          setDeleteTarget({ id: row.id, name: row.name });
                        },
                      },
                    ]}
                  />
                ),
              };
            })}
            emptyLabel="Create a Workflow definition to register orchestration classes."
            emptyAction={createButton}
          />
          {workflowsQuery.hasNextPage ? (
            <div className="mt-4 flex justify-center">
              <Button
                variant="secondary"
                disabled={workflowsQuery.isFetchingNextPage}
                onClick={() => void workflowsQuery.fetchNextPage()}
              >
                {workflowsQuery.isFetchingNextPage ? "Loading…" : "Load more"}
              </Button>
            </div>
          ) : null}
        </>
      )}
    </div>
  );
}
