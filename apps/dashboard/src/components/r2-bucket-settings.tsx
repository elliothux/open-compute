import { Button } from "@cloudflare/kumo/components/button";
import { LayerCard } from "@cloudflare/kumo/components/layer-card";
import { IconTrash } from "@tabler/icons-react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useAuth } from "../features/auth/auth-atoms";
import { useMutationFeedback } from "../features/toast/use-mutation-feedback";
import { DefinitionList } from "./dashboard-page";
import { openConfirmDeleteDialog } from "./resource-dialog";

export function R2BucketSettings({
  name,
  creationDate,
}: {
  name: string;
  creationDate?: string | undefined;
}) {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const objectCheck = useQuery({
    queryKey: [
      "cloudflare-v4",
      "r2",
      selectedInstanceId,
      name,
      "objects",
      "root-check",
    ],
    queryFn: ({ signal }) =>
      client!.r2.buckets.objects.list(
        name,
        { account_id: selectedInstanceId!, per_page: 1 },
        { signal },
      ),
    enabled: client !== null && selectedInstanceId !== null,
  });
  function confirmDeleteBucket() {
    openConfirmDeleteDialog({
      name,
      kind: "r2-bucket",
      confirm: async () => {
        try {
          await client!.r2.buckets.delete(name, {
            account_id: selectedInstanceId!,
          });
        } catch (error) {
          feedback.failure(error, "Unable to delete the bucket.");
          throw error;
        }
        await queryClient.invalidateQueries({
          queryKey: ["cloudflare-v4", "r2", selectedInstanceId, "buckets"],
        });
        feedback.success("R2 bucket deleted.");
        await navigate({ to: "/r2" });
      },
    });
  }

  const empty = objectCheck.isSuccess && objectCheck.data.result.length === 0;

  return (
    <div className="mx-auto grid max-w-4xl min-w-0 gap-6">
      <section>
        <h2 className="mb-3 text-lg font-semibold">General</h2>
        <LayerCard className="px-5 py-4">
          <DefinitionList
            items={[
              { label: "Name", value: name },
              {
                label: "Created",
                value: creationDate
                  ? new Date(creationDate).toLocaleDateString()
                  : "—",
              },
            ]}
          />
        </LayerCard>
      </section>
      <section>
        <h2 className="mb-3 text-base font-semibold">Delete bucket</h2>
        <LayerCard className="flex flex-wrap items-center justify-between gap-3 px-5 py-4">
          <p className="text-kumo-subtle">
            {objectCheck.isError
              ? "Unable to check whether this bucket is empty."
              : empty
                ? "Permanently delete this empty bucket."
                : "Empty this bucket before deleting it."}
          </p>
          <Button
            variant="destructive"
            disabled={!empty}
            onClick={confirmDeleteBucket}
          >
            <IconTrash size={16} /> Delete
          </Button>
        </LayerCard>
      </section>
    </div>
  );
}
