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
import { useAuth } from "../../../features/auth/AuthProvider";
import { queryKeys } from "../../../queries/keys";
import { invalidateR2BucketsQueries } from "../../../queries/invalidate";

export const Route = createFileRoute("/_authenticated/r2/")({
  validateSearch: search => catalogSearchSchema.parse(search),
  component: R2Page,
});

function R2Page() {
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

  const bucketsQuery = useInfiniteQuery({
    queryKey: queryKeys.r2Buckets(accountId ?? "", catalogState),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam, signal }) => client!.r2.listBuckets({
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
    mutationFn: (name: string) => client!.r2.createBucket({
      accountId: accountId!,
      name,
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async () => {
      await invalidateR2BucketsQueries(queryClient, accountId!);
      setErrorMessage(null);
      setCreateOpen(false);
      feedback.success("R2 bucket created.");
    },
    onError: error => {
      setErrorMessage(
        error instanceof OperatorApiError ? error.message : "Unable to create the R2 bucket.",
      );
      feedback.failure(error, "Unable to create the R2 bucket.");
    },
  });

  const renameMutation = useMutation({
    mutationFn: (input: { bucketId: string; name: string }) => client!.r2.renameBucket({
      accountId: accountId!,
      bucketId: parseResourceId(input.bucketId),
      name: input.name,
    }),
    onSuccess: async () => {
      await invalidateR2BucketsQueries(queryClient, accountId!);
      setErrorMessage(null);
      setRenameTarget(null);
      feedback.success("R2 bucket renamed.");
    },
    onError: error => {
      setErrorMessage(
        error instanceof OperatorApiError ? error.message : "Unable to rename the R2 bucket.",
      );
      feedback.failure(error, "Unable to rename the R2 bucket.");
    },
  });

  const deleteMutation = useMutation({
    mutationFn: (input: { bucketId: string; force: boolean }) => client!.r2.deleteBucket({
      accountId: accountId!,
      bucketId: parseResourceId(input.bucketId),
      idempotencyKey: crypto.randomUUID(),
      force: input.force,
    }),
    onSuccess: async () => {
      await invalidateR2BucketsQueries(queryClient, accountId!);
      setErrorMessage(null);
      setDeleteTarget(null);
      feedback.success("R2 bucket deleted.");
    },
    onError: error => {
      setErrorMessage(
        error instanceof OperatorApiError ? error.message : "Unable to delete the R2 bucket.",
      );
      feedback.failure(error, "Unable to delete the R2 bucket.");
    },
  });

  const buckets = useMemo(
    () => (bucketsQuery.data?.pages ?? []).flatMap(page => page.buckets),
    [bucketsQuery.data?.pages],
  );

  const createButton = (
    <Button variant="primary" onClick={() => setCreateOpen(true)}>
      Create bucket
    </Button>
  );

  return (
    <div>
      <PageHeader
        title="R2"
        description="Browse logical buckets and inspect stored objects."
        docsUrl={docsLinks.storage}
      />
      <CatalogToolbar
        search={q}
        onSearchChange={value => void navigate({ search: previous => ({ ...previous, q: value || undefined }) })}
        searchPlaceholder="Search buckets"
        onRefresh={() => void bucketsQuery.refetch()}
        isRefreshing={bucketsQuery.isFetching}
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
        title="Create R2 bucket"
        description="Buckets are account-scoped resources managed through the Operator API."
        nameLabel="Bucket name"
        namePlaceholder="my-bucket"
        submitLabel="Create bucket"
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
        title="Rename R2 bucket"
        description="Updates the bucket display name in platform metadata."
        nameLabel="Bucket name"
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
          renameMutation.mutate({ bucketId: renameTarget.id, name });
        }}
      />
      <ConfirmDeleteResourceDialog
        title="Delete R2 bucket"
        description="This removes the bucket and its binding metadata from the platform registry."
        resourceLabel="the bucket name"
        confirmValue={deleteTarget?.name ?? ""}
      open={Boolean(deleteTarget)}
        errorMessage={deleteTarget ? errorMessage : null}
      isPending={deleteMutation.isPending}
      forceOption={{
        label: "Delete all objects in this bucket",
        description: "Runs the bounded server-side drain before retiring the bucket. Progress is reported by platform metrics.",
      }}
        onClose={() => {
          setDeleteTarget(null);
          setErrorMessage(null);
        }}
        onConfirm={({ force }) => {
          if (!deleteTarget) return;
          deleteMutation.mutate({ bucketId: deleteTarget.id, force });
        }}
      />
      {bucketsQuery.isLoading ? (
        <LoadingState />
      ) : bucketsQuery.error ? (
        <ErrorState message="Unable to load R2 buckets." />
      ) : (
        <>
          <DataTable
            columns={[
              { key: "name", label: "Bucket" },
              { key: "id", label: "ID" },
              { key: "state", label: "State" },
              { key: "created", label: "Created" },
              { key: "actions", label: "" },
            ]}
            rows={buckets.map(bucket => ({
              name: bucket.name,
              id: <code className="[font-size:0.9em]">{bucket.resourceId}</code>,
              state: <StatusBadge value={bucket.state} />,
              created: formatTimestamp(bucket.createdAtMs),
              actions: (
                <RowActionsMenu
                  label={bucket.name}
                  actions={[
                    {
                      id: "browse",
                      label: "Browse objects",
                      onSelect: () => {
                        void navigate({ to: "/r2/$bucketId", params: { bucketId: bucket.resourceId } });
                      },
                    },
                    {
                      id: "rename",
                      label: "Rename",
                      onSelect: () => {
                        setErrorMessage(null);
                        setRenameTarget({ id: bucket.resourceId, name: bucket.name });
                      },
                    },
                    {
                      id: "delete",
                      label: "Delete",
                      variant: "danger",
                      onSelect: () => {
                        setErrorMessage(null);
                        setDeleteTarget({ id: bucket.resourceId, name: bucket.name });
                      },
                    },
                  ]}
                />
              ),
            }))}
            emptyLabel="Create an R2 bucket to manage objects here."
            emptyAction={createButton}
          />
          {bucketsQuery.hasNextPage ? (
            <div className="mt-4 flex justify-center">
              <Button
                variant="secondary"
                disabled={bucketsQuery.isFetchingNextPage}
                onClick={() => void bucketsQuery.fetchNextPage()}
              >
                {bucketsQuery.isFetchingNextPage ? "Loading…" : "Load more"}
              </Button>
            </div>
          ) : null}
        </>
      )}
    </div>
  );
}
