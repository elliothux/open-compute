import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { Button } from "@cloudflare/kumo/components/button";
import { OfficialCatalog } from "../../../components/OfficialCatalog";
import { QueueConfigDialog, type QueueConfigInput } from "../../../components/QueueConfigDialog";
import { useAuth } from "../../../features/auth/AuthProvider";
import { useMutationFeedback } from "../../../features/toast/useMutationFeedback";

export const Route = createFileRoute("/_authenticated/queues/")({ component: QueuesPage });

function QueuesPage() {
  const { client, accountId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const [createOpen, setCreateOpen] = useState(false);
  const [mutationError, setMutationError] = useState<string | null>(null);
  const create = useMutation({
    mutationFn: async (input: QueueConfigInput) => {
      const queue = await client!.cloudflare.queues.create({ account_id: accountId!, queue_name: input.name! });
      if (queue.queue_id && (input.deliveryDelaySeconds !== undefined || input.retentionSeconds !== undefined)) {
        await client!.cloudflare.queues.edit(queue.queue_id, {
          account_id: accountId!,
          settings: {
            ...(input.deliveryDelaySeconds === undefined ? {} : { delivery_delay: input.deliveryDelaySeconds }),
            ...(input.retentionSeconds === undefined ? {} : { message_retention_period: input.retentionSeconds }),
          },
        });
      }
      return queue;
    },
    onSuccess: async () => {
      setCreateOpen(false);
      setMutationError(null);
      await queryClient.invalidateQueries({ queryKey: ["cloudflare-v4", "Queues", accountId] });
      feedback.success("Queue created.");
    },
    onError: error => {
      setMutationError(error instanceof Error ? error.message : "Unable to create the Queue.");
      feedback.failure(error, "Unable to create the Queue.");
    },
  });
  return <OfficialCatalog
    kind="Queues"
    description="Create and configure Queues through the official Queues API."
    load={async (management, accountID, signal) => {
      const page = await management.cloudflare.queues.list({ account_id: accountID }, { signal });
      return page.result.map(queue => ({
        id: queue.queue_id ?? "unknown",
        name: queue.queue_name ?? "Unnamed queue",
        detail: queue.settings?.delivery_paused ? "Delivery paused" : "Delivery active",
        href: `/queues/${encodeURIComponent(queue.queue_id ?? "unknown")}`,
      }));
    }}
    rename={(management, accountID, row, name) => management.cloudflare.queues.edit(row.id, { account_id: accountID, queue_name: name })}
    remove={(management, accountID, row) => management.cloudflare.queues.delete(row.id, { account_id: accountID })}
    primaryAction={<>
      <Button variant="primary" onClick={() => setCreateOpen(true)}>Create Queue</Button>
      <QueueConfigDialog
        mode="create"
        open={createOpen}
        errorMessage={createOpen ? mutationError : null}
        isPending={create.isPending}
        onClose={() => { setCreateOpen(false); setMutationError(null); }}
        onSubmit={input => create.mutate(input)}
      />
    </>}
  />;
}
