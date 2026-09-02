import { OperatorApiError, parseDurableObjectId, parseResourceId } from "@open-compute/operator-sdk";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useInfiniteQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { z } from "zod";
import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import { Surface } from "@cloudflare/kumo/components/surface";
import { parsePageCursor } from "@open-compute/operator-sdk";
import { BackLink, DataTable, ErrorState, LoadingState, PageHeader, StatusBadge } from "../../../components/PageLayout";
import { docsLinks } from "../../../lib/docs";
import { ConfirmActionDialog } from "../../../components/ConfirmActionDialog";
import { useMutationFeedback } from "../../../features/toast/useMutationFeedback";
import { formatRelative, formatTimestamp } from "../../../lib/format";
import { useAuth } from "../../../features/auth/AuthProvider";
import { queryKeys } from "../../../queries/keys";

const doNamespaceSearchSchema = z.object({
  search: z.string().optional(),
});

export const Route = createFileRoute("/_authenticated/durable-objects/$namespaceId")({
  validateSearch: search => doNamespaceSearchSchema.parse(search),
  component: DurableObjectNamespacePage,
});

function DurableObjectNamespacePage() {
  const { namespaceId: namespaceIdParam } = Route.useParams();
  const namespaceId = parseResourceId(namespaceIdParam);
  const { search = "" } = Route.useSearch();
  const navigate = useNavigate({ from: Route.fullPath });
  const { client, accountId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);
  const [mutationError, setMutationError] = useState<string | null>(null);
  const objectsQuery = useInfiniteQuery({
    queryKey: [...queryKeys.doObjects(accountId ?? "", namespaceIdParam), search],
    queryFn: ({ pageParam, signal }) => client!.durableObjects.listObjects({
      accountId: accountId!,
      namespaceId,
      ...(search ? { search } : {}),
      ...(pageParam !== undefined ? { cursor: parsePageCursor(pageParam) } : {}),
      limit: 100,
      signal,
    }),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: lastPage => lastPage.cursor ?? undefined,
    enabled: Boolean(client && accountId),
  });

  const allObjects = useMemo(
    () => (objectsQuery.data?.pages ?? []).flatMap(page => page.objects),
    [objectsQuery.data?.pages],
  );

  const deleteMutation = useMutation({
    mutationFn: (objectId: string) => client!.durableObjects.deleteObject({
      accountId: accountId!,
      namespaceId,
      objectId: parseDurableObjectId(objectId),
    }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: queryKeys.doObjects(accountId!, namespaceIdParam) });
      setDeleteTarget(null);
      setMutationError(null);
      feedback.success("Durable Object deleted.");
    },
    onError: error => {
      setMutationError(error instanceof OperatorApiError ? error.message : "Unable to delete the Durable Object.");
      feedback.failure(error, "Unable to delete the Durable Object.");
    },
  });

  return (
    <div>
      <PageHeader
        title={`Durable Object namespace ${namespaceIdParam}`}
        description="Only registered Object IDs appear here. Unvisited theoretical objects cannot be enumerated."
        docsUrl={docsLinks.storage}
        resourceId={namespaceIdParam}
        resourceLabel="Namespace ID"
        actions={<BackLink to="/durable-objects" label="Back to namespaces" />}
      />
      <Surface className="mb-4 p-4 text-sm text-kumo-subtle">
        This view mirrors Cloudflare inventory boundaries: Object ID, generation, lifecycle, and timestamps only.
      </Surface>
      <ConfirmActionDialog
        title="Delete Durable Object"
        description="This fences the object generation, removes its native storage, and tombstones the registered identity."
        resourceLabel="the Object ID"
        confirmValue={deleteTarget ?? ""}
        submitLabel="Delete object"
        submitVariant="destructive"
        open={Boolean(deleteTarget)}
        errorMessage={deleteTarget ? mutationError : null}
        isPending={deleteMutation.isPending}
        onClose={() => {
          setDeleteTarget(null);
          setMutationError(null);
        }}
        onConfirm={() => {
          if (deleteTarget) deleteMutation.mutate(deleteTarget);
        }}
      />
      <div className="mb-4">
        <Input
          value={search}
          onChange={event => {
            void navigate({ search: prev => ({ ...prev, search: event.target.value || undefined }) });
          }}
          placeholder="Search by exact Object ID"
        />
      </div>
      {objectsQuery.isLoading ? (
        <LoadingState />
      ) : objectsQuery.error ? (
        <ErrorState message="Unable to load registered objects." />
      ) : (
        <>
          <DataTable
            columns={[
              { key: "id", label: "Object ID" },
              { key: "generation", label: "Generation" },
              { key: "lifecycle", label: "Lifecycle" },
              { key: "updated", label: "Updated" },
              { key: "actions", label: "" },
            ]}
            rows={allObjects.map(object => ({
              id: <code className="[font-size:0.9em]">{object.id}</code>,
              generation: String(object.generation),
              lifecycle: <StatusBadge value={object.lifecycle} />,
              updated: (
                <div>
                  <div>{formatRelative(object.updatedAtMs ?? object.createdAtMs)}</div>
                  <div className="text-xs text-kumo-subtle">{formatTimestamp(object.updatedAtMs ?? object.createdAtMs)}</div>
                </div>
              ),
              actions: object.lifecycle !== "tombstoned" ? (
                <Button
                  variant="destructive"
                  disabled={deleteMutation.isPending}
                  onClick={() => {
                    setMutationError(null);
                    setDeleteTarget(object.id);
                  }}
                >
                  Delete
                </Button>
              ) : "—",
            }))}
            emptyLabel="No registered objects match the current filter."
          />
          {objectsQuery.hasNextPage ? (
            <div className="mt-4 flex justify-center">
              <Button
                variant="secondary"
                disabled={objectsQuery.isFetchingNextPage}
                onClick={() => void objectsQuery.fetchNextPage()}
              >
                {objectsQuery.isFetchingNextPage ? "Loading…" : "Load more objects"}
              </Button>
            </div>
          ) : null}
        </>
      )}
    </div>
  );
}
