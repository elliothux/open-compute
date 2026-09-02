import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "@cloudflare/kumo/components/button";
import { Surface } from "@cloudflare/kumo/components/surface";
import { parseQueueConsumerId } from "@open-compute/operator-sdk";
import { ConfirmActionDialog } from "../../../components/ConfirmActionDialog";
import { DataTable, ErrorState, LoadingState, PageHeader, StatusBadge } from "../../../components/PageLayout";
import { docsLinks } from "../../../lib/docs";
import { SchedulerInspectPanel, StructuredSummaryPanel } from "../../../components/StructuredSummary";
import { useMutationFeedback } from "../../../features/toast/useMutationFeedback";
import { useAuth } from "../../../features/auth/AuthProvider";
import { queryKeys } from "../../../queries/keys";

export const Route = createFileRoute("/_authenticated/platform/")({
  component: PlatformPage,
});

type QueueConsumerRow = {
  id: string;
  queueId?: string;
  workerId?: string;
  generation?: number;
  state?: string;
  backlogMessages?: number;
};

function PlatformPage() {
  const { client } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const [cacheGcOpen, setCacheGcOpen] = useState(false);

  const schedulerQuery = useQuery({
    queryKey: queryKeys.scheduler,
    queryFn: ({ signal }) => client!.platform.scheduler({ signal }),
    enabled: Boolean(client),
  });
  const queueConsumersQuery = useQuery({
    queryKey: queryKeys.queueConsumers,
    queryFn: ({ signal }) => client!.platform.queueConsumers({ signal }),
    enabled: Boolean(client),
  });
  const cronQuery = useQuery({
    queryKey: queryKeys.cronActivations,
    queryFn: ({ signal }) => client!.platform.cronActivations({ signal }),
    enabled: Boolean(client),
  });
  const cacheQuery = useQuery({
    queryKey: queryKeys.cache,
    queryFn: ({ signal }) => client!.platform.cache({ signal }),
    enabled: Boolean(client),
  });
  const imagesQuery = useQuery({
    queryKey: queryKeys.imagesCapacity,
    queryFn: ({ signal }) => client!.platform.imagesCapacity({ signal }),
    enabled: Boolean(client),
  });

  const invalidatePlatform = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: queryKeys.scheduler }),
      queryClient.invalidateQueries({ queryKey: queryKeys.queueConsumers }),
    ]);
  };

  const pauseSchedulerMutation = useMutation({
    mutationFn: () => client!.platform.pauseScheduler(),
    onSuccess: async () => {
      await invalidatePlatform();
      feedback.success("Scheduler paused.");
    },
    onError: error => feedback.failure(error, "Unable to pause the scheduler."),
  });
  const resumeSchedulerMutation = useMutation({
    mutationFn: () => client!.platform.resumeScheduler(),
    onSuccess: async () => {
      await invalidatePlatform();
      feedback.success("Scheduler resumed.");
    },
    onError: error => feedback.failure(error, "Unable to resume the scheduler."),
  });
  const repairSchedulerMutation = useMutation({
    mutationFn: () => client!.platform.repairScheduler(),
    onSuccess: async result => {
      await invalidatePlatform();
      feedback.success(`Scheduler repair completed (${result.repaired} items).`);
    },
    onError: error => feedback.failure(error, "Unable to repair the scheduler."),
  });
  const workflowReconcileMutation = useMutation({
    mutationFn: () => client!.workflows.reconcile(),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: queryKeys.scheduler });
      feedback.success("Workflow reconciliation completed.");
    },
    onError: error => feedback.failure(error, "Unable to reconcile workflows."),
  });
  const cacheGcMutation = useMutation({
    mutationFn: () => client!.platform.cacheGc(),
    onSuccess: async result => {
      await queryClient.invalidateQueries({ queryKey: queryKeys.cache });
      setCacheGcOpen(false);
      feedback.success(`Cache garbage collection removed ${result.deleted} item${result.deleted === 1 ? "" : "s"}.`);
    },
    onError: error => feedback.failure(error, "Unable to run cache garbage collection."),
  });
  const pauseConsumerMutation = useMutation({
    mutationFn: (input: { consumerId: string; consumerGeneration: number }) =>
      client!.platform.pauseQueueConsumer({
        consumerId: parseQueueConsumerId(input.consumerId),
        consumerGeneration: input.consumerGeneration,
      }),
    onSuccess: async () => {
      await invalidatePlatform();
      feedback.success("Queue consumer paused.");
    },
    onError: error => feedback.failure(error, "Unable to pause the queue consumer."),
  });
  const resumeConsumerMutation = useMutation({
    mutationFn: (input: { consumerId: string; consumerGeneration: number }) =>
      client!.platform.resumeQueueConsumer({
        consumerId: parseQueueConsumerId(input.consumerId),
        consumerGeneration: input.consumerGeneration,
      }),
    onSuccess: async () => {
      await invalidatePlatform();
      feedback.success("Queue consumer resumed.");
    },
    onError: error => feedback.failure(error, "Unable to resume the queue consumer."),
  });

  const schedulerPaused = Boolean(schedulerQuery.data?.paused);
  const platformPending =
    pauseSchedulerMutation.isPending ||
    resumeSchedulerMutation.isPending ||
    repairSchedulerMutation.isPending ||
    workflowReconcileMutation.isPending ||
    cacheGcMutation.isPending ||
    pauseConsumerMutation.isPending ||
    resumeConsumerMutation.isPending;

  const consumerRows: QueueConsumerRow[] = Array.isArray(queueConsumersQuery.data?.queueConsumers)
    ? (queueConsumersQuery.data.queueConsumers as QueueConsumerRow[])
    : [];

  return (
    <div>
      <PageHeader
        title="Platform"
        description="Scheduler, queue consumers, cache lifecycle, and Images capacity reported by the operator control plane."
        docsUrl={docsLinks.platform}
      />
      <ConfirmActionDialog
        title="Run cache garbage collection"
        description="This permanently removes unreferenced cached artifacts. Active deployments and pinned snapshots remain protected."
        resourceLabel="cache"
        confirmValue="cache"
        submitLabel="Run garbage collection"
        submitVariant="destructive"
        open={cacheGcOpen}
        errorMessage={cacheGcMutation.error instanceof Error ? cacheGcMutation.error.message : null}
        isPending={cacheGcMutation.isPending}
        onClose={() => setCacheGcOpen(false)}
        onConfirm={() => cacheGcMutation.mutate()}
      />
      <div className="grid gap-4 xl:grid-cols-2">
        <Surface className="p-4">
          <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
            <div>
              <div className="text-sm font-medium">Scheduler</div>
              <div className="mt-1 text-sm text-kumo-subtle">
                Global scheduler state: {schedulerPaused ? "paused" : "running"}
              </div>
            </div>
            <div className="flex flex-wrap gap-2">
              <Button
                variant="secondary"
                disabled={platformPending || schedulerPaused}
                onClick={() => pauseSchedulerMutation.mutate()}
              >
                Pause
              </Button>
              <Button
                variant="secondary"
                disabled={platformPending || !schedulerPaused}
                onClick={() => resumeSchedulerMutation.mutate()}
              >
                Resume
              </Button>
              <Button
                variant="primary"
                disabled={platformPending}
                onClick={() => repairSchedulerMutation.mutate()}
              >
                Repair
              </Button>
            </div>
          </div>
          <SchedulerInspectPanel title="Scheduler detail" query={schedulerQuery} mode="scheduler" />
        </Surface>

        <Surface className="p-4">
          <div className="mb-3 text-sm font-medium">Queue consumers</div>
          {queueConsumersQuery.isLoading ? (
            <LoadingState label="Loading queue consumers…" />
          ) : queueConsumersQuery.error ? (
            <ErrorState message="Unable to load queue consumers." />
          ) : consumerRows.length === 0 ? (
            <p className="text-sm text-kumo-subtle">No queue consumers reported.</p>
          ) : (
            <DataTable
              columns={[
                { key: "id", label: "Consumer" },
                { key: "queue", label: "Queue" },
                { key: "state", label: "State" },
                { key: "backlog", label: "Backlog" },
                { key: "actions", label: "" },
              ]}
              rows={consumerRows.map(row => {
                const generation = row.generation ?? 0;
                const isPaused = row.state === "paused";
                return {
                  id: <code className="[font-size:0.9em]">{row.id}</code>,
                  queue: row.queueId ?? "—",
                  state: row.state ? <StatusBadge value={row.state} /> : "—",
                  backlog: row.backlogMessages?.toLocaleString() ?? "—",
                  actions: generation > 0 ? (
                    <Button
                      variant={isPaused ? "primary" : "secondary"}
                      disabled={platformPending}
                      onClick={() => {
                        if (isPaused) {
                          resumeConsumerMutation.mutate({
                            consumerId: row.id,
                            consumerGeneration: generation,
                          });
                        } else {
                          pauseConsumerMutation.mutate({
                            consumerId: row.id,
                            consumerGeneration: generation,
                          });
                        }
                      }}
                    >
                      {isPaused ? "Resume" : "Pause"}
                    </Button>
                  ) : (
                    "—"
                  ),
                };
              })}
              emptyLabel="No queue consumers reported."
            />
          )}
        </Surface>

        <SchedulerInspectPanel title="Cron activations" query={cronQuery} mode="cronActivations" />
        <StructuredSummaryPanel title="Cache" query={cacheQuery} />
        <StructuredSummaryPanel title="Images capacity" query={imagesQuery} />
        <Surface className="p-4">
          <div className="text-sm font-medium">Maintenance</div>
          <p className="mt-1 text-sm text-kumo-subtle">
            Run bounded recovery and cleanup operations against persisted platform authority.
          </p>
          <div className="mt-4 flex flex-wrap gap-2">
            <Button
              variant="secondary"
              disabled={platformPending}
              onClick={() => workflowReconcileMutation.mutate()}
            >
              Reconcile workflows
            </Button>
            <Button
              variant="destructive"
              disabled={platformPending}
              onClick={() => setCacheGcOpen(true)}
            >
              Run cache GC
            </Button>
          </div>
        </Surface>
      </div>
    </div>
  );
}
