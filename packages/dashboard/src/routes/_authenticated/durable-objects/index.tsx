import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { useInfiniteQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Button } from "@cloudflare/kumo/components/button";
import { OperatorApiError, parsePageCursor, parseResourceId } from "@open-compute/operator-sdk";
import { CatalogToolbar } from "../../../components/CatalogToolbar";
import { CatalogFilters, catalogSearchSchema } from "../../../components/CatalogFilters";
import { ConfirmDeleteResourceDialog } from "../../../components/ConfirmDeleteResourceDialog";
import { CreateDoNamespaceDialog } from "../../../components/CreateDoNamespaceDialog";
import { RenameResourceDialog } from "../../../components/RenameResourceDialog";
import { RowActionsMenu } from "../../../components/RowActionsMenu";
import { DataTable, ErrorState, LoadingState, PageHeader, StatusBadge } from "../../../components/PageLayout";
import { docsLinks } from "../../../lib/docs";
import { formatTimestamp } from "../../../lib/format";
import { doNamespaceRow } from "../../../lib/catalog";
import { useAuth } from "../../../features/auth/AuthProvider";
import { queryKeys } from "../../../queries/keys";
import { invalidateDoNamespacesQueries } from "../../../queries/invalidate";

export const Route = createFileRoute("/_authenticated/durable-objects/")({
  validateSearch: search => catalogSearchSchema.parse(search),
  component: DurableObjectsPage,
});

function DurableObjectsPage() {
  const navigate = useNavigate({ from: Route.fullPath });
  const { q = "", status = "all", sort = "updatedAt", direction = "desc" } = Route.useSearch();
  const { client, accountId } = useAuth();
  const queryClient = useQueryClient();
  const [createOpen, setCreateOpen] = useState(false);
  const [renameTarget, setRenameTarget] = useState<{ id: string; name: string } | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<{ id: string; name: string } | null>(null);
  const [mutationError, setMutationError] = useState<string | null>(null);
  const searchNeedle = q.trim();
  const catalogState = `${searchNeedle}:${status}:${sort}:${direction}`;

  const namespacesQuery = useInfiniteQuery({
    queryKey: queryKeys.doNamespaces(accountId ?? "", catalogState),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam, signal }) => client!.durableObjects.listNamespaces({
      accountId: accountId!,
      ...(searchNeedle ? { search: searchNeedle } : {}),
      ...(status === "all" ? {} : { status }),
      sort,
      direction,
      ...(pageParam !== undefined ? { cursor: parsePageCursor(pageParam) } : {}),
      limit: 100,
      signal,
    }),
    getNextPageParam: lastPage => (lastPage.listComplete ? undefined : lastPage.cursor ?? undefined),
    enabled: Boolean(client && accountId),
  });

  const renameMutation = useMutation({
    mutationFn: (input: { namespaceId: string; name: string }) => client!.durableObjects.renameNamespace({
      accountId: accountId!,
      namespaceId: parseResourceId(input.namespaceId),
      name: input.name,
    }),
    onSuccess: async () => {
      await invalidateDoNamespacesQueries(queryClient, accountId!);
      setMutationError(null);
      setRenameTarget(null);
    },
    onError: error => {
      setMutationError(
        error instanceof OperatorApiError ? error.message : "Unable to rename the namespace.",
      );
    },
  });

  const deleteMutation = useMutation({
    mutationFn: (input: { namespaceId: string; force: boolean }) => client!.durableObjects.deleteNamespace({
      accountId: accountId!,
      namespaceId: parseResourceId(input.namespaceId),
      idempotencyKey: crypto.randomUUID(),
      force: input.force,
    }),
    onSuccess: async () => {
      await invalidateDoNamespacesQueries(queryClient, accountId!);
      setMutationError(null);
      setDeleteTarget(null);
    },
    onError: error => {
      setMutationError(
        error instanceof OperatorApiError && error.code === "resource_referenced"
          ? "This namespace is still referenced by a deployment or active runtime generation. Retry after the reference is removed."
          : error instanceof OperatorApiError
            ? error.message
            : "Unable to delete the namespace.",
      );
    },
  });

  const namespaces = useMemo(
    () => (namespacesQuery.data?.pages ?? []).flatMap(page => page.namespaces),
    [namespacesQuery.data?.pages],
  );

  const createButton = (
    <Button variant="primary" onClick={() => setCreateOpen(true)}>
      Create namespace
    </Button>
  );

  return (
    <div>
      <PageHeader
        title="Durable Objects"
        description="Registry metadata only. Object memory, SQL, KV, alarms, and WebSocket state are not exposed."
        docsUrl={docsLinks.storage}
      />
      <CatalogToolbar
        search={q}
        onSearchChange={value => void navigate({ search: previous => ({ ...previous, q: value || undefined }) })}
        searchPlaceholder="Search namespaces"
        onRefresh={() => void namespacesQuery.refetch()}
        isRefreshing={namespacesQuery.isFetching}
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
      {client && accountId ? (
        <CreateDoNamespaceDialog
          client={client}
          accountId={accountId}
          open={createOpen}
          onClose={() => setCreateOpen(false)}
        />
      ) : null}
      <RenameResourceDialog
        title="Rename Durable Object namespace"
        description="Updates the namespace display name in platform metadata."
        nameLabel="Namespace name"
        currentName={renameTarget?.name ?? ""}
        open={Boolean(renameTarget)}
        errorMessage={renameTarget ? mutationError : null}
        isPending={renameMutation.isPending}
        onClose={() => {
          setRenameTarget(null);
          setMutationError(null);
        }}
        onSubmit={name => {
          if (!renameTarget) return;
          renameMutation.mutate({ namespaceId: renameTarget.id, name });
        }}
      />
      <ConfirmDeleteResourceDialog
        title="Delete Durable Object namespace"
        description="This removes namespace metadata from the platform registry. Registered objects must be drained first unless force delete is enabled."
        resourceLabel="the namespace name"
        confirmValue={deleteTarget?.name ?? ""}
        open={Boolean(deleteTarget)}
        errorMessage={deleteTarget ? mutationError : null}
        isPending={deleteMutation.isPending}
        forceOption={{
          label: "Force delete non-empty namespace",
          description: "Fences and deletes all registered object instances before removing the namespace.",
        }}
        onClose={() => {
          setDeleteTarget(null);
          setMutationError(null);
        }}
        onConfirm={({ force }) => {
          if (!deleteTarget) return;
          deleteMutation.mutate({ namespaceId: deleteTarget.id, force });
        }}
      />
      {namespacesQuery.isLoading ? (
        <LoadingState />
      ) : namespacesQuery.error ? (
        <ErrorState message="Unable to load Durable Object namespaces." />
      ) : (
        <>
          <DataTable
            columns={[
              { key: "name", label: "Namespace" },
              { key: "id", label: "ID" },
              { key: "class", label: "Class" },
              { key: "worker", label: "Worker" },
              { key: "state", label: "State" },
              { key: "created", label: "Created" },
              { key: "actions", label: "" },
            ]}
            rows={namespaces.map(namespace => {
              const row = doNamespaceRow(namespace);
              return {
                name: row.name,
                id: <code className="[font-size:0.9em]">{row.id}</code>,
                class: row.className,
                worker: <code className="[font-size:0.9em]">{row.ownerWorkerId}</code>,
                state: <StatusBadge value={row.state} />,
                created: formatTimestamp(row.createdAtMs),
                actions: (
                  <RowActionsMenu
                    label={row.name}
                    actions={[
                      {
                        id: "view",
                        label: "View objects",
                        onSelect: () => {
                          void navigate({
                            to: "/durable-objects/$namespaceId",
                            params: { namespaceId: row.id },
                          });
                        },
                      },
                      {
                        id: "rename",
                        label: "Rename",
                        onSelect: () => {
                          setMutationError(null);
                          setRenameTarget({ id: row.id, name: row.name });
                        },
                      },
                      {
                        id: "delete",
                        label: "Delete",
                        variant: "danger",
                        onSelect: () => {
                          setMutationError(null);
                          setDeleteTarget({ id: row.id, name: row.name });
                        },
                      },
                    ]}
                  />
                ),
              };
            })}
            emptyLabel="Create a Durable Object namespace to register class bindings."
            emptyAction={createButton}
          />
          {namespacesQuery.hasNextPage ? (
            <div className="mt-4 flex justify-center">
              <Button
                variant="secondary"
                disabled={namespacesQuery.isFetchingNextPage}
                onClick={() => void namespacesQuery.fetchNextPage()}
              >
                {namespacesQuery.isFetchingNextPage ? "Loading…" : "Load more"}
              </Button>
            </div>
          ) : null}
        </>
      )}
    </div>
  );
}
