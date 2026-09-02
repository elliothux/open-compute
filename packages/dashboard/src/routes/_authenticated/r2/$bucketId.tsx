import { OperatorApiError, parsePageCursor, readBoundedStreamBytes, parseResourceId } from "@open-compute/operator-sdk";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { z } from "zod";
import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import { Surface } from "@cloudflare/kumo/components/surface";
import { ConfirmActionDialog } from "../../../components/ConfirmActionDialog";
import { DetailTabs } from "../../../components/DetailTabs";
import { BackLink, DataTable, ErrorState, LoadingState, PageHeader } from "../../../components/PageLayout";
import { docsLinks } from "../../../lib/docs";
import { formatBytes, formatTimestamp } from "../../../lib/format";
import { useAuth } from "../../../features/auth/AuthProvider";
import { useMutationFeedback } from "../../../features/toast/useMutationFeedback";
import { queryKeys } from "../../../queries/keys";

const r2BucketSearchSchema = z.object({
  tab: z.enum(["objects", "upload"]).optional(),
  prefix: z.string().optional(),
  key: z.string().optional(),
});

export const Route = createFileRoute("/_authenticated/r2/$bucketId")({
  validateSearch: search => r2BucketSearchSchema.parse(search),
  component: R2BucketPage,
});

const R2_PREVIEW_MAX_BYTES = 256 * 1024;
const R2_TRANSFER_MAX_BYTES = 64 * 1024 * 1024;

function R2BucketPage() {
  const { bucketId: bucketIdParam } = Route.useParams();
  const bucketId = parseResourceId(bucketIdParam);
  const { tab: tabParam, prefix = "", key: selectedKey = "" } = Route.useSearch();
  const activeTab = tabParam ?? "objects";
  const navigate = useNavigate({ from: Route.fullPath });
  const { client, accountId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const [draftKey, setDraftKey] = useState("");
  const [draftFile, setDraftFile] = useState<File | null>(null);
  const [downloading, setDownloading] = useState(false);
  const [deleteKeyTarget, setDeleteKeyTarget] = useState<string | null>(null);
  const [mutationError, setMutationError] = useState<string | null>(null);

  const objectsQuery = useInfiniteQuery({
    queryKey: queryKeys.r2Objects(accountId ?? "", bucketIdParam, prefix),
    queryFn: ({ pageParam, signal }) => client!.r2.listObjects({
      accountId: accountId!,
      bucketId,
      ...(prefix ? { prefix } : {}),
      ...(pageParam !== undefined ? { cursor: parsePageCursor(pageParam) } : {}),
      limit: 100,
      signal,
    }),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: lastPage => lastPage.cursor ?? undefined,
    enabled: Boolean(client && accountId) && activeTab === "objects",
  });

  const allObjects = useMemo(
    () => (objectsQuery.data?.pages ?? []).flatMap(page => page.objects),
    [objectsQuery.data?.pages],
  );

  const objectQuery = useQuery({
    queryKey: [...queryKeys.r2Objects(accountId ?? "", bucketIdParam, prefix), "meta", selectedKey],
    queryFn: ({ signal }) => client!.r2.headObject({
      accountId: accountId!,
      bucketId,
      key: selectedKey,
      signal,
    }),
    enabled: Boolean(client && accountId && selectedKey && activeTab === "objects"),
  });
  const objectBodyQuery = useQuery({
    queryKey: [...queryKeys.r2Objects(accountId ?? "", bucketIdParam, prefix), "body", selectedKey],
    queryFn: async ({ signal }) => {
      const object = await client!.r2.getObject({
        accountId: accountId!,
        bucketId,
        key: selectedKey,
        signal,
      });
      const previewBytes = await readBoundedStreamBytes(object.body, R2_PREVIEW_MAX_BYTES);
      return { previewBytes, truncated: (object.contentLength ?? previewBytes.byteLength) > previewBytes.byteLength };
    },
    enabled: Boolean(client && accountId && selectedKey && activeTab === "objects"),
  });
  const putMutation = useMutation({
    mutationFn: async (params: { key: string; file: File }) => client!.r2.putObject({
      accountId: accountId!,
      bucketId,
      key: params.key,
      body: new Uint8Array(await params.file.arrayBuffer()),
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async (_data, variables) => {
      await queryClient.invalidateQueries({
        queryKey: queryKeys.r2Objects(accountId!, bucketIdParam, prefix),
      });
      void navigate({ search: prev => ({ ...prev, tab: "objects", key: variables.key }) });
      setDraftKey("");
      setDraftFile(null);
      setMutationError(null);
      feedback.success("R2 object uploaded.");
    },
    onError: error => {
      setMutationError(error instanceof Error ? error.message : "Unable to upload the object.");
      feedback.failure(error, "Unable to upload the object.");
    },
  });
  const deleteMutation = useMutation({
    mutationFn: (key: string) => client!.r2.deleteObject({
      accountId: accountId!,
      bucketId,
      key,
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: queryKeys.r2Objects(accountId!, bucketIdParam, prefix),
      });
      setMutationError(null);
      setDeleteKeyTarget(null);
      void navigate({ search: prev => ({ ...prev, key: undefined }) });
    },
    onError: error => {
      setMutationError(
        error instanceof OperatorApiError ? error.message : "Unable to delete the object.",
      );
    },
  });
  const mutationPending = putMutation.isPending || deleteMutation.isPending;
  const previewText = objectBodyQuery.data
    ? `${new TextDecoder().decode(objectBodyQuery.data.previewBytes)}${objectBodyQuery.data.truncated ? "\n\n[Preview truncated]" : ""}`
    : "";

  const downloadSelectedObject = async () => {
    if (!selectedKey || !objectQuery.data) return;
    if (objectQuery.data.size > R2_TRANSFER_MAX_BYTES) {
      setMutationError("Dashboard downloads are limited to 64 MiB. Use the Operator SDK for larger objects.");
      return;
    }
    setDownloading(true);
    setMutationError(null);
    try {
      const object = await client!.r2.getObject({
        accountId: accountId!,
        bucketId,
        key: selectedKey,
        maxBytes: R2_TRANSFER_MAX_BYTES,
      });
      const bytes = await readBoundedStreamBytes(object.body, R2_TRANSFER_MAX_BYTES);
      const blobBytes = new Uint8Array(bytes.byteLength);
      blobBytes.set(bytes);
      const url = URL.createObjectURL(new Blob([blobBytes.buffer], { type: "application/octet-stream" }));
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = selectedKey.split("/").pop() || "r2-object";
      anchor.click();
      URL.revokeObjectURL(url);
      feedback.success("R2 object download started.");
    } catch (error) {
      setMutationError(error instanceof Error ? error.message : "Unable to download the object.");
      feedback.failure(error, "Unable to download the object.");
    } finally {
      setDownloading(false);
    }
  };

  return (
    <div>
      <PageHeader
        title={`R2 bucket ${bucketIdParam}`}
        description="Browse, upload, and delete objects through bounded operator APIs."
        docsUrl={docsLinks.storage}
        resourceId={bucketIdParam}
        resourceLabel="Bucket ID"
        actions={<BackLink to="/r2" label="Back to R2" />}
      />
      <DetailTabs
        tabs={[
          { id: "objects", label: "Objects" },
          { id: "upload", label: "Upload" },
        ]}
        activeTab={activeTab}
        onTabChange={tabId => {
          void navigate({ search: prev => ({ ...prev, tab: tabId as "objects" | "upload" }) });
        }}
      />
      <ConfirmActionDialog
        title="Delete R2 object"
        description="This permanently removes the object from the bucket."
        resourceLabel="the object key"
        confirmValue={deleteKeyTarget ?? ""}
        submitLabel="Delete object"
        submitVariant="destructive"
        open={Boolean(deleteKeyTarget)}
        errorMessage={deleteKeyTarget ? mutationError : null}
        isPending={deleteMutation.isPending}
        onClose={() => {
          setDeleteKeyTarget(null);
          setMutationError(null);
        }}
        onConfirm={() => {
          if (!deleteKeyTarget) return;
          deleteMutation.mutate(deleteKeyTarget);
        }}
      />
      {activeTab === "objects" ? (
        <>
          <div className="mb-4">
            <Input
              value={prefix}
              onChange={event => {
                void navigate({ search: prev => ({ ...prev, prefix: event.target.value || undefined, key: undefined }) });
              }}
              placeholder="Prefix filter"
            />
          </div>
          <div className="grid gap-4 xl:grid-cols-[minmax(0,1.2fr)_minmax(0,0.8fr)]">
            {objectsQuery.isLoading ? (
              <LoadingState />
            ) : objectsQuery.error ? (
              <ErrorState message="Unable to load bucket objects." />
            ) : (
              <>
                <DataTable
                  columns={[
                    { key: "key", label: "Object key" },
                    { key: "size", label: "Size" },
                    { key: "uploaded", label: "Uploaded" },
                    { key: "actions", label: "" },
                  ]}
                  rows={allObjects.map(object => ({
                    key: object.key,
                    size: formatBytes(object.size),
                    uploaded: formatTimestamp(object.uploaded ?? undefined),
                    actions: (
                      <button
                        type="button"
                        className="text-sm text-kumo-link"
                        onClick={() => {
                          void navigate({ search: prev => ({ ...prev, key: object.key }) });
                        }}
                      >
                        Inspect
                      </button>
                    ),
                  }))}
                  emptyLabel="This bucket has no objects yet."
                />
                {objectsQuery.hasNextPage ? (
                  <div className="mt-4 flex justify-center xl:col-span-2">
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
            <div className="space-y-4">
              <Surface className="p-4">
                <div className="mb-2 text-sm font-medium">Object preview</div>
                {!selectedKey ? (
                  <div className="text-sm text-kumo-subtle">Select an object to inspect its metadata and body.</div>
                ) : objectQuery.isLoading || objectBodyQuery.isLoading ? (
                  <LoadingState label="Loading object…" />
                ) : objectQuery.error || objectBodyQuery.error ? (
                  <ErrorState message="Unable to load the selected object." />
                ) : (
                  <>
                    <dl className="mb-3 space-y-1 text-xs text-kumo-subtle">
                      <div>Size: {formatBytes(objectQuery.data?.size ?? 0)}</div>
                      <div>ETag: {objectQuery.data?.etag ?? "—"}</div>
                      <div>Uploaded: {formatTimestamp(objectQuery.data?.uploaded ?? undefined)}</div>
                    </dl>
                    <pre className="max-h-96 overflow-auto rounded-md bg-kumo-control/40 p-3 text-xs whitespace-pre-wrap break-all">
                      {previewText}
                    </pre>
                    {mutationError ? <p className="mt-3 text-sm text-kumo-danger" role="alert">{mutationError}</p> : null}
                    <div className="mt-3 flex flex-wrap gap-2">
                      <Button
                        variant="secondary"
                        disabled={downloading || (objectQuery.data?.size ?? 0) > R2_TRANSFER_MAX_BYTES}
                        onClick={() => void downloadSelectedObject()}
                      >
                        {downloading ? "Downloading…" : "Download"}
                      </Button>
                      <Button
                        variant="destructive"
                        disabled={mutationPending}
                        onClick={() => {
                          setMutationError(null);
                          setDeleteKeyTarget(selectedKey);
                        }}
                      >
                        Delete object
                      </Button>
                    </div>
                  </>
                )}
              </Surface>
            </div>
          </div>
        </>
      ) : null}
      {activeTab === "upload" ? (
        <Surface className="max-w-xl p-4">
          <div className="mb-2 text-sm font-medium">Upload object</div>
          <div className="space-y-2">
            <Input
              label="Object key"
              value={draftKey}
              onChange={event => setDraftKey(event.target.value)}
              placeholder="Object key"
            />
            <label className="block text-sm">
              <span className="mb-1 block font-medium text-kumo-default">File</span>
              <input
                className="block w-full rounded-md ring ring-kumo-line bg-kumo-base px-3 py-2 text-sm"
                type="file"
                onChange={event => {
                  const file = event.target.files?.[0] ?? null;
                  setDraftFile(file);
                  if (file && !draftKey) setDraftKey(file.name);
                  setMutationError(file && file.size > R2_TRANSFER_MAX_BYTES ? "Dashboard uploads are limited to 64 MiB." : null);
                }}
              />
            </label>
            {draftFile ? <p className="text-sm text-kumo-subtle">{draftFile.name} · {formatBytes(draftFile.size)}</p> : null}
            {mutationError ? <p className="text-sm text-kumo-danger" role="alert">{mutationError}</p> : null}
            <Button
              variant="primary"
              disabled={mutationPending || !draftKey.trim() || !draftFile || draftFile.size > R2_TRANSFER_MAX_BYTES}
              onClick={() => {
                if (draftFile) putMutation.mutate({ key: draftKey.trim(), file: draftFile });
              }}
            >
              Upload object
            </Button>
          </div>
        </Surface>
      ) : null}
    </div>
  );
}
