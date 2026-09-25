import { Button } from "@cloudflare/kumo/components/button";
import {
  IconCopy,
  IconDownload,
  IconRefresh,
  IconTrash,
} from "@tabler/icons-react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ItemGetResponse } from "cloudflare/resources/aisearch/namespaces/instances/items";
import { useAuth } from "../features/auth/auth-atoms";
import { useMutationFeedback } from "../features/toast/use-mutation-feedback";
import { ErrorState } from "./dashboard-page";
import { openAlert } from "./dialog-manager";

function fileSize(bytes: number | null) {
  if (bytes === null) return "—";
  if (bytes < 1024) return `${bytes} B`;
  return `${(bytes / 1024).toFixed(1)} KB`;
}

export function AISearchItemDetail({
  itemId,
  namespaceName,
  instanceId,
  onDeleted,
}: {
  itemId: string;
  namespaceName: string;
  instanceId: string;
  onDeleted: () => void;
}) {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const params = {
    account_id: selectedInstanceId!,
    name: namespaceName,
    id: instanceId,
  };
  const prefix = [
    "ai-search",
    selectedInstanceId,
    namespaceName,
    instanceId,
    "items",
  ];
  const item = useQuery({
    queryKey: [...prefix, itemId],
    queryFn: ({ signal }) =>
      client!.aiSearch.namespaces.instances.items.get(itemId, params, {
        signal,
      }),
    enabled: client !== null && selectedInstanceId !== null,
  });
  const chunks = useQuery({
    queryKey: [...prefix, itemId, "chunks"],
    queryFn: ({ signal }) =>
      client!.aiSearch.namespaces.instances.items.chunks(
        itemId,
        { ...params, limit: 6, offset: 0 },
        { signal },
      ),
    enabled: client !== null && selectedInstanceId !== null,
  });
  const logs = useQuery({
    queryKey: [...prefix, itemId, "logs"],
    queryFn: ({ signal }) =>
      client!.aiSearch.namespaces.instances.items.logs(
        itemId,
        { ...params, limit: 20 },
        { signal },
      ),
    enabled: client !== null && selectedInstanceId !== null,
  });
  const reindex = useMutation({
    mutationFn: () =>
      client!.aiSearch.namespaces.instances.items.sync(itemId, {
        ...params,
        next_action: "INDEX",
      }),
    onSuccess: async () => {
      feedback.success("Item queued for reindexing.");
      await queryClient.invalidateQueries({ queryKey: prefix });
    },
    onError: (error) => feedback.failure(error, "Could not reindex item."),
  });
  function confirmDeleteItem() {
    openAlert({
      title: "Delete item",
      description:
        "Are you sure you want to delete this item? This action cannot be undone.",
      size: "lg",
      contentClassName: "px-5 py-4",
      content: (
        <div className="bg-kumo-recessed mt-4 rounded px-3 py-2 font-mono text-sm">
          {value?.key}
        </div>
      ),
      confirmVariant: "destructive",
      confirmText: "Delete",
      onConfirm: async () => {
        try {
          await client!.aiSearch.namespaces.instances.items.delete(
            itemId,
            params,
          );
        } catch (error) {
          feedback.failure(error, "Could not delete item.");
          throw error;
        }
        onDeleted();
        feedback.success("Item deleted.");
        await queryClient.invalidateQueries({ queryKey: prefix });
      },
    });
  }

  const download = useMutation({
    mutationFn: async (value: ItemGetResponse) => {
      const response =
        await client!.aiSearch.namespaces.instances.items.download(
          itemId,
          params,
        );
      const url = URL.createObjectURL(await response.blob());
      const link = document.createElement("a");
      link.href = url;
      link.download = value.key.split("/").at(-1) || value.key;
      link.click();
      setTimeout(() => URL.revokeObjectURL(url), 60_000);
    },
    onError: (error) => feedback.failure(error, "Could not download item."),
  });
  const value = item.data;
  if (item.isLoading) return <div className="p-5 text-sm">Loading item…</div>;
  if (item.error)
    return (
      <div className="p-5">
        <ErrorState error={item.error} />
      </div>
    );
  if (!value) return null;
  const metadata = Object.entries(value.metadata ?? {});
  return (
    <div className="border-kumo-line grid min-h-96 border-t md:grid-cols-3">
      <div className="min-w-0 md:col-span-2">
        <div className="flex flex-wrap items-center justify-between gap-2 px-4 py-3">
          <div className="flex items-center gap-2 text-xs">
            <span className="text-kumo-subtle">ID:</span>
            <span className="font-mono text-[0.9em]">{value.id}</span>
            <Button
              shape="square"
              variant="ghost"
              aria-label="Copy item ID"
              onClick={() => navigator.clipboard.writeText(value.id)}
            >
              <IconCopy size={14} />
            </Button>
          </div>
          <div className="flex gap-2">
            {value.source_id === "builtin" ? (
              <Button
                variant="secondary"
                disabled={download.isPending}
                onClick={() => download.mutate(value)}
              >
                <IconDownload size={14} /> Download
              </Button>
            ) : null}
            <Button
              variant="secondary"
              disabled={reindex.isPending}
              onClick={() => reindex.mutate()}
            >
              <IconRefresh size={14} /> Reindex
            </Button>
            <Button variant="destructive" onClick={confirmDeleteItem}>
              <IconTrash size={14} /> Delete
            </Button>
          </div>
        </div>
        <div className="grid gap-1 px-4 py-2 text-sm">
          <p>
            <span className="text-kumo-subtle">Key:</span> {value.key}
          </p>
          <p>
            <span className="text-kumo-subtle">File size:</span>{" "}
            {fileSize(value.file_size)}
          </p>
          <p>
            <span className="text-kumo-subtle">Source:</span>{" "}
            {value.source_id === "builtin"
              ? "Uploaded"
              : (value.source_id ?? "—")}
          </p>
        </div>
        <div className="px-4 py-4 text-sm">
          <h3 className="font-medium">Metadata</h3>
          {metadata.length ? (
            <div className="mt-1 grid grid-cols-2 gap-x-6 gap-y-1">
              {metadata.map(([key, metadataValue]) => (
                <p className="min-w-0 truncate" key={key}>
                  <span className="text-kumo-subtle">{key}:</span>{" "}
                  {String(metadataValue)}
                </p>
              ))}
            </div>
          ) : (
            <p className="text-kumo-subtle mt-1">No metadata available</p>
          )}
        </div>
        <div className="border-kumo-line min-h-52 border-t px-4 py-3 text-sm">
          <h3 className="font-medium">Chunks</h3>
          {chunks.error ? (
            <ErrorState error={chunks.error} />
          ) : chunks.isLoading ? (
            <p className="text-kumo-subtle mt-10 text-center">
              Loading chunks…
            </p>
          ) : !chunks.data?.length ? (
            <p className="text-kumo-subtle mt-10 text-center">
              No chunks available
            </p>
          ) : (
            <div className="mt-3 grid gap-3">
              {chunks.data.map((chunk) => (
                <div
                  className="border-kumo-line rounded border p-3"
                  key={chunk.id}
                >
                  <p className="text-kumo-subtle mb-1 font-mono text-xs">
                    {chunk.id}
                  </p>
                  <p className="whitespace-pre-wrap">{chunk.text}</p>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
      <div className="border-kumo-line bg-kumo-recessed border-l px-4 py-4 text-sm">
        <h3 className="font-medium">Processing logs</h3>
        {logs.error ? (
          <ErrorState error={logs.error} />
        ) : logs.isLoading ? (
          <p className="text-kumo-subtle mt-6 text-center">Loading logs…</p>
        ) : !logs.data?.length ? (
          <p className="text-kumo-subtle mt-6 text-center">No logs available</p>
        ) : (
          <div className="mt-4 grid gap-3">
            {logs.data.map((log, index) => (
              <div
                className="border-kumo-line bg-kumo-base rounded border p-3"
                key={`${log.timestamp}:${index}`}
              >
                <p className="font-medium">{log.action}</p>
                <p className="text-kumo-subtle text-xs">
                  {new Date(log.timestamp).toLocaleString()}
                </p>
                {log.message ? <p className="mt-1">{log.message}</p> : null}
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
