import { Button } from "@cloudflare/kumo/components/button";
import { LayerCard } from "@cloudflare/kumo/components/layer-card";
import { IconArrowDown, IconCopy, IconTrash } from "@tabler/icons-react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { ErrorState, LoadingRows } from "../../../components/dashboard-page";
import { openConfirmDeleteDialog } from "../../../components/resource-dialog";
import { useAuth } from "../../../features/auth/auth-atoms";
import { useMutationFeedback } from "../../../features/toast/use-mutation-feedback";
import { formatBytes } from "../../../lib/format";

export const Route = createFileRoute(
  "/_authenticated/r2/$bucketId_/objects/$objectKey/details",
)({ component: R2ObjectDetailPage });

function R2ObjectDetailPage() {
  const { bucketId, objectKey } = Route.useParams();
  const navigate = useNavigate();
  const { client, instanceId: selectedInstanceId } = useAuth();
  const feedback = useMutationFeedback();
  const enabled = client !== null && selectedInstanceId !== null;
  const detail = useQuery({
    queryKey: [
      "cloudflare-v4",
      "r2",
      selectedInstanceId,
      bucketId,
      "object",
      objectKey,
    ],
    queryFn: async ({ signal }) => {
      const page = await client!.r2.buckets.objects.list(
        bucketId,
        { account_id: selectedInstanceId!, prefix: objectKey, per_page: 1000 },
        { signal },
      );
      const object = page.result.find((item) => item.key === objectKey);
      if (!object) throw new Error("Object not found.");
      const type =
        object.http_metadata?.contentType ?? "application/octet-stream";
      let text: string | undefined;
      if (
        object.size !== undefined &&
        object.size <= 1024 * 1024 &&
        (type.startsWith("text/") || type.includes("json"))
      ) {
        const response = await client!.r2.buckets.objects.get(objectKey, {
          account_id: selectedInstanceId!,
          bucket_name: bucketId,
        });
        text = await response.text();
      }
      return { object, type, text };
    },
    enabled,
  });
  const download = useMutation({
    mutationFn: async () => {
      const response = await client!.r2.buckets.objects.get(objectKey, {
        account_id: selectedInstanceId!,
        bucket_name: bucketId,
      });
      const url = URL.createObjectURL(await response.blob());
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = objectKey.split("/").pop() || "download";
      anchor.click();
      URL.revokeObjectURL(url);
    },
    onError: (error) =>
      feedback.failure(error, "Unable to download the object."),
  });
  function confirmDeleteObject() {
    openConfirmDeleteDialog({
      name: objectKey,
      confirm: async () => {
        try {
          await client!.r2.buckets.objects.delete(objectKey, {
            account_id: selectedInstanceId!,
            bucket_name: bucketId,
          });
        } catch (error) {
          feedback.failure(error, "Unable to delete the object.");
          throw error;
        }
        feedback.success("Object deleted.");
        void navigate({
          to: "/r2/$bucketId",
          params: { bucketId },
          search: { prefix: "" },
        });
      },
    });
  }

  const copy = async (value: string) => {
    try {
      await navigator.clipboard.writeText(value);
      feedback.success("Copied to clipboard.");
    } catch (error) {
      feedback.failure(error, "Unable to copy.");
    }
  };
  const object = detail.data?.object;
  const previewText = detail.data?.text;
  const metadata = Object.entries(object?.custom_metadata ?? {});

  return (
    <div className="-mt-3 grid gap-4 sm:-mt-5">
      <div className="border-kumo-line flex flex-wrap items-center justify-between gap-3 border-b pb-4">
        <div className="flex min-w-0 items-center gap-1">
          <h1 className="min-w-0 truncate font-medium">{objectKey}</h1>
          <Button
            variant="ghost"
            shape="square"
            aria-label="Copy object key"
            onClick={() => void copy(objectKey)}
          >
            <IconCopy size={16} />
          </Button>
        </div>
        <div className="flex gap-2">
          <Button
            variant="secondary"
            disabled={download.isPending}
            onClick={() => download.mutate()}
          >
            <IconArrowDown size={16} />
            Download
          </Button>
          <Button variant="destructive" onClick={() => confirmDeleteObject()}>
            <IconTrash size={16} />
            Delete
          </Button>
        </div>
      </div>
      {detail.isLoading ? (
        <LoadingRows />
      ) : detail.error ? (
        <ErrorState error={detail.error} />
      ) : object ? (
        <>
          <LayerCard className="overflow-hidden p-0">
            <h2 className="border-kumo-line border-b px-4 py-3 font-medium">
              Object details
            </h2>
            <dl className="grid grid-cols-3 gap-4 px-4 py-4 md:grid-cols-4">
              {[
                [
                  "Created",
                  object.last_modified
                    ? new Date(object.last_modified).toLocaleString()
                    : "—",
                ],
                ["Type", detail.data?.type ?? "—"],
                ["Storage class", object.storage_class ?? "Standard"],
                ["Size", formatBytes(object.size)],
              ].map(([label, value]) => (
                <div
                  key={label}
                  className={
                    label === "Created" ? "col-span-3 md:col-span-1" : ""
                  }
                >
                  <dt className="text-kumo-subtle">{label}</dt>
                  <dd>{value}</dd>
                </div>
              ))}
            </dl>
          </LayerCard>
          <LayerCard className="overflow-hidden p-0">
            <h2 className="border-kumo-line border-b px-4 py-3 font-medium">
              Custom metadata
            </h2>
            {metadata.length ? (
              <dl className="grid grid-cols-2 gap-4 px-4 py-4">
                {metadata.map(([key, value]) => (
                  <div key={key}>
                    <dt className="text-kumo-subtle">{key}</dt>
                    <dd>{value}</dd>
                  </div>
                ))}
              </dl>
            ) : (
              <p className="px-4 py-4">No custom metadata set</p>
            )}
          </LayerCard>
          {previewText !== undefined ? (
            <LayerCard className="overflow-hidden p-0">
              <h2 className="border-kumo-line border-b px-4 py-3 font-medium">
                Object preview
              </h2>
              <div className="px-4 py-4">
                <div className="relative max-w-md">
                  <pre className="bg-kumo-tint max-h-80 w-full overflow-auto rounded-md px-3 py-3 pr-12 font-mono text-xs whitespace-pre">
                    {previewText}
                  </pre>
                  <Button
                    variant="secondary"
                    shape="square"
                    className="absolute top-1 right-1"
                    aria-label="Copy object preview"
                    onClick={() => void copy(previewText)}
                  >
                    <IconCopy size={16} />
                  </Button>
                </div>
              </div>
            </LayerCard>
          ) : null}
        </>
      ) : null}
    </div>
  );
}
