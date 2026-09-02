import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { useInfiniteQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Button } from "@cloudflare/kumo/components/button";
import { OperatorApiError, parsePageCursor, parseResourceId } from "@open-compute/operator-sdk";
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
import { catalogResourceRow } from "../../../lib/catalog";
import { useAuth } from "../../../features/auth/AuthProvider";
import { queryKeys } from "../../../queries/keys";
import { invalidateD1DatabasesQueries } from "../../../queries/invalidate";

export const Route = createFileRoute("/_authenticated/d1/")({
  validateSearch: search => catalogSearchSchema.parse(search),
  component: D1Page,
});

function D1Page() {
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

  const databasesQuery = useInfiniteQuery({
    queryKey: queryKeys.d1Databases(accountId ?? "", catalogState),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam, signal }) => client!.d1.listDatabases({
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

  const createMutation = useMutation({
    mutationFn: (name: string) => client!.d1.createDatabase({
      accountId: accountId!,
      name,
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async () => {
      await invalidateD1DatabasesQueries(queryClient, accountId!);
      setErrorMessage(null);
      setCreateOpen(false);
      feedback.success("D1 database created.");
    },
    onError: error => {
      setErrorMessage(
        error instanceof OperatorApiError ? error.message : "Unable to create the D1 database.",
      );
      feedback.failure(error, "Unable to create the D1 database.");
    },
  });

  const renameMutation = useMutation({
    mutationFn: (input: { databaseId: string; name: string }) => client!.d1.renameDatabase({
      accountId: accountId!,
      databaseId: parseResourceId(input.databaseId),
      name: input.name,
    }),
    onSuccess: async () => {
      await invalidateD1DatabasesQueries(queryClient, accountId!);
      setErrorMessage(null);
      setRenameTarget(null);
      feedback.success("D1 database renamed.");
    },
    onError: error => {
      setErrorMessage(
        error instanceof OperatorApiError ? error.message : "Unable to rename the D1 database.",
      );
      feedback.failure(error, "Unable to rename the D1 database.");
    },
  });

  const deleteMutation = useMutation({
    mutationFn: (databaseId: string) => client!.d1.deleteDatabase({
      accountId: accountId!,
      databaseId: parseResourceId(databaseId),
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async () => {
      await invalidateD1DatabasesQueries(queryClient, accountId!);
      setErrorMessage(null);
      setDeleteTarget(null);
      feedback.success("D1 database deleted.");
    },
    onError: error => {
      setErrorMessage(
        error instanceof OperatorApiError ? error.message : "Unable to delete the D1 database.",
      );
      feedback.failure(error, "Unable to delete the D1 database.");
    },
  });

  const databases = useMemo(
    () => (databasesQuery.data?.pages ?? []).flatMap(page => page.databases),
    [databasesQuery.data?.pages],
  );

  const createButton = (
    <Button variant="primary" onClick={() => setCreateOpen(true)}>
      Create database
    </Button>
  );

  return (
    <div>
      <PageHeader
        title="D1"
        description="Database studio inspired by Localflare: browse tables and run bounded SQL queries."
        docsUrl={docsLinks.storage}
      />
      <CatalogToolbar
        search={q}
        onSearchChange={value => void navigate({ search: previous => ({ ...previous, q: value || undefined }) })}
        searchPlaceholder="Search databases"
        onRefresh={() => void databasesQuery.refetch()}
        isRefreshing={databasesQuery.isFetching}
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
        title="Create D1 database"
        description="Databases are account-scoped resources managed through the Operator API."
        nameLabel="Database name"
        namePlaceholder="MY_DATABASE"
        submitLabel="Create database"
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
        title="Rename D1 database"
        description="Updates the database display name in platform metadata."
        nameLabel="Database name"
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
          renameMutation.mutate({ databaseId: renameTarget.id, name });
        }}
      />
      <ConfirmDeleteResourceDialog
        title="Delete D1 database"
        description="This removes the database and its binding metadata from the platform registry."
        resourceLabel="the database name"
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
      {databasesQuery.isLoading ? (
        <LoadingState />
      ) : databasesQuery.error ? (
        <ErrorState message="Unable to load D1 databases." />
      ) : (
        <>
        <DataTable
          columns={[
            { key: "name", label: "Database" },
            { key: "id", label: "ID" },
            { key: "state", label: "State" },
            { key: "created", label: "Created" },
            { key: "actions", label: "" },
          ]}
          rows={databases.map(database => {
            const row = catalogResourceRow(database);
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
                      label: "Open studio",
                      onSelect: () => {
                        void navigate({ to: "/d1/$databaseId", params: { databaseId: row.id } });
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
          emptyLabel="Create a D1 database to inspect schema and data."
          emptyAction={createButton}
        />
          {databasesQuery.hasNextPage ? (
            <div className="mt-4 flex justify-center">
              <Button
                variant="secondary"
                disabled={databasesQuery.isFetchingNextPage}
                onClick={() => void databasesQuery.fetchNextPage()}
              >
                {databasesQuery.isFetchingNextPage ? "Loading…" : "Load more"}
              </Button>
            </div>
          ) : null}
        </>
      )}
    </div>
  );
}
