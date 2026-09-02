import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { useInfiniteQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Button } from "@cloudflare/kumo/components/button";
import { OperatorApiError, parsePageCursor, parseResourceId } from "@open-compute/operator-sdk";
import { CatalogToolbar } from "../../../components/CatalogToolbar";
import { CatalogFilters, catalogSearchSchema } from "../../../components/CatalogFilters";
import { ConfirmDeleteResourceDialog } from "../../../components/ConfirmDeleteResourceDialog";
import { CreateKvNamespaceDialog } from "../../../components/CreateKvNamespaceDialog";
import { RenameResourceDialog } from "../../../components/RenameResourceDialog";
import { RowActionsMenu } from "../../../components/RowActionsMenu";
import { DataTable, ErrorState, LoadingState, PageHeader, StatusBadge } from "../../../components/PageLayout";
import { docsLinks } from "../../../lib/docs";
import { useMutationFeedback } from "../../../features/toast/useMutationFeedback";
import { formatTimestamp } from "../../../lib/format";
import { catalogResourceRow } from "../../../lib/catalog";
import { useAuth } from "../../../features/auth/AuthProvider";
import { queryKeys } from "../../../queries/keys";
import { invalidateKvNamespacesQueries } from "../../../queries/invalidate";

export const Route = createFileRoute("/_authenticated/kv/")({
  validateSearch: search => catalogSearchSchema.parse(search),
  component: KvPage,
});

function KvPage() {
  const navigate = useNavigate({ from: Route.fullPath });
  const { q = "", status = "all", sort = "updatedAt", direction = "desc" } = Route.useSearch();
  const { client, accountId } = useAuth();
  const queryClient = useQueryClient();
  const [createOpen, setCreateOpen] = useState(false);
  const [renameTarget, setRenameTarget] = useState<{ id: string; name: string } | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<{ id: string; name: string } | null>(null);
  const [mutationError, setMutationError] = useState<string | null>(null);
  const feedback = useMutationFeedback();
  const searchNeedle = q.trim();
  const catalogState = `${searchNeedle}:${status}:${sort}:${direction}`;

  const namespacesQuery = useInfiniteQuery({
    queryKey: queryKeys.kvNamespaces(accountId ?? "", catalogState),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam, signal }) => client!.kv.listNamespaces({
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
    mutationFn: (input: { namespaceId: string; name: string }) => client!.kv.renameNamespace({
      accountId: accountId!,
      namespaceId: parseResourceId(input.namespaceId),
      name: input.name,
    }),
    onSuccess: async () => {
      await invalidateKvNamespacesQueries(queryClient, accountId!);
      setMutationError(null);
      setRenameTarget(null);
      feedback.success("KV namespace renamed.");
    },
    onError: error => {
      setMutationError(
        error instanceof OperatorApiError ? error.message : "Unable to rename the KV namespace.",
      );
      feedback.failure(error, "Unable to rename the KV namespace.");
    },
  });

  const deleteMutation = useMutation({
    mutationFn: (namespaceId: string) => client!.kv.deleteNamespace({
      accountId: accountId!,
      namespaceId: parseResourceId(namespaceId),
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async () => {
      await invalidateKvNamespacesQueries(queryClient, accountId!);
      setMutationError(null);
      setDeleteTarget(null);
      feedback.success("KV namespace deleted.");
    },
    onError: error => {
      setMutationError(
        error instanceof OperatorApiError ? error.message : "Unable to delete the KV namespace.",
      );
      feedback.failure(error, "Unable to delete the KV namespace.");
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
        title="KV"
        description="Browse KV namespaces and inspect keys through the Operator API."
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
        <CreateKvNamespaceDialog
          client={client}
          accountId={accountId}
          open={createOpen}
          onClose={() => setCreateOpen(false)}
        />
      ) : null}
      <RenameResourceDialog
        title="Rename KV namespace"
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
        title="Delete KV namespace"
        description="This removes the namespace and its binding metadata from the platform registry."
        resourceLabel="the namespace name"
        confirmValue={deleteTarget?.name ?? ""}
        open={Boolean(deleteTarget)}
        errorMessage={deleteTarget ? mutationError : null}
        isPending={deleteMutation.isPending}
        onClose={() => {
          setDeleteTarget(null);
          setMutationError(null);
        }}
        onConfirm={() => {
          if (!deleteTarget) return;
          deleteMutation.mutate(deleteTarget.id);
        }}
      />
      {namespacesQuery.isLoading ? (
        <LoadingState />
      ) : namespacesQuery.error ? (
        <ErrorState message="Unable to load KV namespaces." />
      ) : (
        <>
          <DataTable
            columns={[
              { key: "name", label: "Namespace" },
              { key: "id", label: "ID" },
              { key: "state", label: "State" },
              { key: "created", label: "Created" },
              { key: "actions", label: "" },
            ]}
            rows={namespaces.map(namespace => {
              const row = catalogResourceRow(namespace);
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
                        id: "browse",
                        label: "Browse keys",
                        onSelect: () => {
                          void navigate({ to: "/kv/$namespaceId", params: { namespaceId: row.id } });
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
            emptyLabel="Create a KV namespace to start storing data."
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
