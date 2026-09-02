import { useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { Button } from "@cloudflare/kumo/components/button";
import { DataTable, ErrorState, LoadingState, PageHeader, SectionHeader, StatusBadge } from "../../../components/PageLayout";
import { QueueConfigDialog, type QueueConfigInput } from "../../../components/QueueConfigDialog";
import { useAuth } from "../../../features/auth/AuthProvider";
import { useMutationFeedback } from "../../../features/toast/useMutationFeedback";

export const Route = createFileRoute("/_authenticated/queues/$queueId")({ component: QueueDetailPage });

function QueueDetailPage() {
  const { queueId } = Route.useParams();
  const { client, accountId } = useAuth();
  const feedback = useMutationFeedback();
  const enabled = client !== null && accountId !== null;
  const [configOpen, setConfigOpen] = useState(false);
  const [mutationError, setMutationError] = useState<string | null>(null);
  const queue = useQuery({
    queryKey: ["cloudflare-v4", "queues", queueId],
    queryFn: ({ signal }) => client!.cloudflare.queues.get(queueId, { account_id: accountId! }, { signal }),
    enabled,
  });
  const metrics = useQuery({
    queryKey: ["cloudflare-v4", "queues", queueId, "metrics"],
    queryFn: ({ signal }) => client!.cloudflare.queues.getMetrics(queueId, { account_id: accountId! }, { signal }),
    enabled,
  });
  const consumers = useQuery({
    queryKey: ["cloudflare-v4", "queues", queueId, "consumers"],
    queryFn: ({ signal }) => client!.cloudflare.queues.consumers.list(queueId, { account_id: accountId! }, { signal }),
    enabled,
  });
  const configMutation = useMutation({
    mutationFn: (input: QueueConfigInput) => client!.cloudflare.queues.edit(queueId, {
      account_id: accountId!,
      settings: {
        ...(input.deliveryDelaySeconds !== undefined ? { delivery_delay: input.deliveryDelaySeconds } : {}),
        ...(input.retentionSeconds !== undefined ? { message_retention_period: input.retentionSeconds } : {}),
      },
    }),
    onSuccess: async () => {
      setConfigOpen(false);
      setMutationError(null);
      await queue.refetch();
      feedback.success("Queue configuration updated.");
    },
    onError: error => {
      setMutationError(error instanceof Error ? error.message : "Unable to update queue configuration.");
      feedback.failure(error, "Unable to update queue configuration.");
    },
  });
  const deliveryMutation = useMutation({
    mutationFn: (paused: boolean) => client!.cloudflare.queues.edit(queueId, {
      account_id: accountId!,
      settings: { delivery_paused: paused },
    }),
    onSuccess: async (_result, paused) => {
      await queue.refetch();
      feedback.success(paused ? "Queue delivery paused." : "Queue delivery resumed.");
    },
    onError: error => feedback.failure(error, "Unable to update queue delivery."),
  });
  const paused = queue.data?.settings?.delivery_paused === true;
  return <div>
    <PageHeader
      title={queue.data?.queue_name ?? "Queue"}
      resourceId={queueId}
      description="Inspect metrics and consumers, and update queue delivery settings through the official Queues API."
      actions={<>
        <Button variant="secondary" disabled={!queue.data || deliveryMutation.isPending} onClick={() => deliveryMutation.mutate(!paused)}>{paused ? "Resume delivery" : "Pause delivery"}</Button>
        <Button variant="primary" disabled={!queue.data} onClick={() => setConfigOpen(true)}>Edit configuration</Button>
      </>}
    />
    <QueueConfigDialog
      mode="edit"
      open={configOpen}
      initial={{
        ...(queue.data?.settings?.delivery_delay === undefined ? {} : { deliveryDelaySeconds: queue.data.settings.delivery_delay }),
        ...(queue.data?.settings?.message_retention_period === undefined ? {} : { retentionSeconds: queue.data.settings.message_retention_period }),
      }}
      errorMessage={configOpen ? mutationError : null}
      isPending={configMutation.isPending}
      onClose={() => { setConfigOpen(false); setMutationError(null); }}
      onSubmit={input => configMutation.mutate(input)}
    />
    {queue.isLoading || consumers.isLoading || metrics.isLoading ? <LoadingState /> : queue.error || consumers.error || metrics.error ? <ErrorState message="Unable to load Queue details." /> : <>
      <DataTable columns={[{ key: "name", label: "Metric" }, { key: "value", label: "Value" }]} rows={[
        { name: "Delivery", value: <StatusBadge value={paused ? "paused" : "active"} /> },
        { name: "Backlog messages", value: metrics.data?.backlog_count ?? 0 },
        { name: "Backlog bytes", value: metrics.data?.backlog_bytes ?? 0 },
        { name: "Oldest message timestamp", value: metrics.data?.oldest_message_timestamp_ms || "unknown" },
      ]} />
      <div className="mt-6">
        <SectionHeader title="Consumers" />
        <DataTable columns={[
          { key: "id", label: "Consumer" },
          { key: "type", label: "Type" },
          { key: "target", label: "Target" },
          { key: "batch", label: "Batch size" },
          { key: "retries", label: "Retries" },
        ]} rows={(consumers.data?.result ?? []).map(item => ({
          id: item.consumer_id ?? "unknown",
          type: item.type ?? "unknown",
          target: "script_name" in item ? item.script_name ?? "unknown" : "HTTP pull",
          batch: item.settings?.batch_size ?? "default",
          retries: item.settings?.max_retries ?? "default",
        }))} emptyLabel="No consumers found." />
      </div>
    </>}
  </div>;
}
