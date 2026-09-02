import { createFileRoute, Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "@cloudflare/kumo/components/button";
import { parseQueueId } from "@open-compute/operator-sdk";
import { QueueConfigDialog, type QueueConfigInput } from "../../../components/QueueConfigDialog";
import { BackLink, ErrorState, LoadingState, PageHeader, StatusBadge } from "../../../components/PageLayout";
import { docsLinks } from "../../../lib/docs";
import { useMutationFeedback } from "../../../features/toast/useMutationFeedback";
import { formatTimestamp } from "../../../lib/format";
import { useAuth } from "../../../features/auth/AuthProvider";
import { queryKeys } from "../../../queries/keys";

export const Route = createFileRoute("/_authenticated/queues/$queueId")({
  component: QueueDetailPage,
});

function QueueDetailPage() {
  const { queueId: queueIdParam } = Route.useParams();
  const queueId = parseQueueId(queueIdParam);
  const { client, accountId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const [configOpen, setConfigOpen] = useState(false);
  const [mutationError, setMutationError] = useState<string | null>(null);

  const queueQuery = useQuery({
    queryKey: queryKeys.queue(accountId ?? "", queueIdParam),
    queryFn: ({ signal }) => client!.queues.get({ accountId: accountId!, queueId, signal }),
    enabled: Boolean(client && accountId),
  });

  const queue = queueQuery.data?.queue;
  const updateConfigMutation = useMutation({
    mutationFn: (input: QueueConfigInput) => client!.queues.updateConfig({
      accountId: accountId!,
      queueId,
      expectedConfigGeneration: queue!.configGeneration,
      idempotencyKey: crypto.randomUUID(),
      ...(input.deliveryDelaySeconds !== undefined ? { deliveryDelaySeconds: input.deliveryDelaySeconds } : {}),
      ...(input.retentionSeconds !== undefined ? { retentionSeconds: input.retentionSeconds } : {}),
      ...(input.maxBacklogBytes !== undefined ? { maxBacklogBytes: input.maxBacklogBytes } : {}),
    }),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.queue(accountId!, queueIdParam) }),
        queryClient.invalidateQueries({ queryKey: ["operator", "queues", accountId!] }),
      ]);
      setConfigOpen(false);
      setMutationError(null);
      feedback.success("Queue configuration saved.");
    },
    onError: error => {
      setMutationError(error instanceof Error ? error.message : "Unable to update queue configuration.");
      feedback.failure(error, "Unable to update queue configuration.");
    },
  });

  return (
    <div>
      <PageHeader
        title={queue?.name ?? queueIdParam}
        description="Queue configuration, lifecycle generation, and bounded backlog settings."
        docsUrl={docsLinks.platform}
        resourceId={queueIdParam}
        resourceLabel="Queue ID"
        actions={(
          <>
            <Button variant="primary" disabled={!queue} onClick={() => setConfigOpen(true)}>Edit configuration</Button>
            <BackLink to="/queues" label="Back to queues" />
          </>
        )}
      />
      <QueueConfigDialog
        open={configOpen}
        mode="edit"
        {...(queue ? { initial: {
          deliveryDelaySeconds: queue.deliveryDelaySeconds,
          retentionSeconds: queue.retentionSeconds,
          maxBacklogBytes: queue.maxBacklogBytes,
        } } : {})}
        errorMessage={mutationError}
        isPending={updateConfigMutation.isPending}
        onClose={() => {
          setConfigOpen(false);
          setMutationError(null);
        }}
        onSubmit={input => updateConfigMutation.mutate(input)}
      />
      {queueQuery.isLoading ? <LoadingState /> : null}
      {queueQuery.error ? <ErrorState message="Unable to load the Queue." /> : null}
      {queue ? (
        <dl className="grid gap-4 rounded-lg border border-kumo-line bg-kumo-elevated p-6 sm:grid-cols-2">
          <div>
            <dt className="text-xs font-medium text-kumo-subtle">Queue ID</dt>
            <dd className="mt-1 font-mono text-sm">{queue.id}</dd>
          </div>
          <div>
            <dt className="text-xs font-medium text-kumo-subtle">State</dt>
            <dd className="mt-1"><StatusBadge value={queue.state} /></dd>
          </div>
          <div>
            <dt className="text-xs font-medium text-kumo-subtle">Availability</dt>
            <dd className="mt-1"><StatusBadge value={queue.availability} /></dd>
          </div>
          <div>
            <dt className="text-xs font-medium text-kumo-subtle">Config generation</dt>
            <dd className="mt-1 font-mono text-sm">{queue.configGeneration}</dd>
          </div>
          <div>
            <dt className="text-xs font-medium text-kumo-subtle">Lifecycle generation</dt>
            <dd className="mt-1 font-mono text-sm">{queue.lifecycleGeneration}</dd>
          </div>
          <div>
            <dt className="text-xs font-medium text-kumo-subtle">Retention</dt>
            <dd className="mt-1 text-sm">{queue.retentionSeconds}s</dd>
          </div>
          <div>
            <dt className="text-xs font-medium text-kumo-subtle">Delivery delay</dt>
            <dd className="mt-1 text-sm">{queue.deliveryDelaySeconds}s</dd>
          </div>
          <div>
            <dt className="text-xs font-medium text-kumo-subtle">Max backlog</dt>
            <dd className="mt-1 text-sm">{queue.maxBacklogBytes.toLocaleString()} bytes</dd>
          </div>
          <div>
            <dt className="text-xs font-medium text-kumo-subtle">Updated</dt>
            <dd className="mt-1 text-sm">{formatTimestamp(queue.updatedAtMs)}</dd>
          </div>
        </dl>
      ) : null}
      <div className="mt-6 text-sm text-kumo-subtle">
        Pause and resume queue consumers from the{" "}
        <Link to="/platform" className="text-kumo-link">
          Platform
        </Link>{" "}
        page using the reported consumer generation fence.
      </div>
    </div>
  );
}
