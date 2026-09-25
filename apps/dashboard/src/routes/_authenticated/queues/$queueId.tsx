import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import { LayerCard } from "@cloudflare/kumo/components/layer-card";
import { Tabs } from "@cloudflare/kumo/components/tabs";
import {
  IconCopy,
  IconEdit,
  IconGauge,
  IconPlayerPause,
  IconPlayerPlay,
  IconRefresh,
} from "@tabler/icons-react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import {
  ErrorState,
  LoadingRows,
  PageHeader,
  Panel,
  Section,
  StatGrid,
} from "../../../components/dashboard-page";
import { DataTable } from "../../../components/page-layout";
import { QueueConsumerEditor } from "../../../components/queue-consumer-editor";
import {
  openConfirmDeleteDialog,
  openResourceNameDialog,
} from "../../../components/resource-dialog";
import { useAuth } from "../../../features/auth/auth-atoms";
import { useMutationFeedback } from "../../../features/toast/use-mutation-feedback";
import {
  formatBytes,
  formatDate,
  formatDateTime as formatTimestamp,
} from "../../../lib/format";
import { queueQuery } from "../../../lib/query-options";

type Tab = "metrics" | "settings";

export const Route = createFileRoute("/_authenticated/queues/$queueId")({
  validateSearch: (search: Record<string, unknown>): { tab?: Tab } =>
    search.tab === "settings" ? { tab: "settings" } : {},
  loader: ({ context, params }) => {
    const { client, instanceId } = context.auth;
    if (!client || !instanceId) return;
    return context.queryClient.ensureQueryData(
      queueQuery(client, instanceId, params.queueId),
    );
  },
  component: QueueDetailPage,
});

function QueueDetailPage() {
  const { queueId } = Route.useParams();
  const { tab: searchTab } = Route.useSearch();
  const tab = searchTab ?? "metrics";
  const navigate = useNavigate();
  const { client, instanceId: selectedInstanceId } = useAuth();
  const feedback = useMutationFeedback();
  const queryClient = useQueryClient();
  const enabled = client !== null && selectedInstanceId !== null;

  const queue = useQuery(queueQuery(client, selectedInstanceId, queueId));
  const metrics = useQuery({
    queryKey: [
      "cloudflare-v4",
      "queues",
      selectedInstanceId,
      queueId,
      "metrics",
    ],
    queryFn: ({ signal }) =>
      client!.queues.getMetrics(
        queueId,
        { account_id: selectedInstanceId! },
        { signal },
      ),
    enabled,
  });

  const refresh = async () => {
    await Promise.all([
      queryClient.invalidateQueries({
        queryKey: ["cloudflare-v4", "queues", selectedInstanceId, queueId],
      }),
      queryClient.invalidateQueries({
        queryKey: ["open-compute", "queues", selectedInstanceId, queueId],
      }),
    ]);
  };
  function openRenameQueueDialog() {
    openResourceNameDialog({
      title: "Rename queue",
      description: "Change the name used to identify this queue.",
      label: "Queue name",
      initialValue: queue.data?.queue_name ?? "",
      submitLabel: "Save name",
      submit: async (name) => {
        try {
          if (!queue.data?.settings)
            throw new Error("Queue settings are unavailable.");
          await client!.queues.update(queueId, {
            account_id: selectedInstanceId!,
            queue_name: name,
            settings: queue.data.settings,
          });
        } catch (error) {
          feedback.failure(error, "Unable to rename the queue.");
          throw error;
        }
        await refresh();
        feedback.success("Queue renamed.");
      },
    });
  }

  const saveSetting = useMutation({
    mutationFn: (input: {
      field: "delivery_delay" | "message_retention_period";
      value: number;
    }) => {
      if (!queue.data?.queue_name || !queue.data.settings)
        throw new Error("Queue settings are unavailable.");
      return client!.queues.update(queueId, {
        account_id: selectedInstanceId!,
        queue_name: queue.data.queue_name,
        settings: {
          ...queue.data.settings,
          [input.field]: input.value,
        },
      });
    },
    onSuccess: async () => {
      await refresh();
      feedback.success("Queue setting saved.");
    },
    onError: (error) =>
      feedback.failure(error, "Unable to save the queue setting."),
  });
  const toggleDelivery = useMutation({
    mutationFn: (paused: boolean) => {
      if (!queue.data?.queue_name || !queue.data.settings)
        throw new Error("Queue settings are unavailable.");
      return client!.queues.update(queueId, {
        account_id: selectedInstanceId!,
        queue_name: queue.data.queue_name,
        settings: { ...queue.data.settings, delivery_paused: paused },
      });
    },
    onSuccess: async (_data, paused) => {
      await refresh();
      feedback.success(
        paused ? "Queue delivery paused." : "Queue delivery resumed.",
      );
    },
    onError: (error) =>
      feedback.failure(error, "Unable to change delivery state."),
  });
  function confirmDeleteQueue() {
    openConfirmDeleteDialog({
      name: queue.data?.queue_name ?? "",
      confirm: async () => {
        try {
          await client!.queues.delete(queueId, {
            account_id: selectedInstanceId!,
          });
        } catch (error) {
          feedback.failure(error, "Unable to delete the queue.");
          throw error;
        }
        feedback.success("Queue deleted.");
        await navigate({ to: "/queues" });
      },
    });
  }

  const paused = queue.data?.settings?.delivery_paused === true;
  const loading = queue.isLoading || metrics.isLoading;
  const error = queue.error ?? metrics.error;
  return (
    <div className="text-sm leading-5">
      <PageHeader
        title={queue.data?.queue_name ?? "Queue"}
        description="Monitor delivery and configure consumers for this queue."
        actions={
          <Button
            variant="secondary"
            icon={<IconRefresh size={16} />}
            disabled={loading}
            onClick={() => void refresh()}
          >
            Refresh
          </Button>
        }
      />
      {queue.data?.queue_id ? (
        <div className="text-kumo-subtle mb-4 flex min-w-0 flex-wrap items-center gap-2">
          <span>Queue ID</span>
          <code className="max-w-full font-mono text-xs break-all">
            {queue.data.queue_id}
          </code>
          <Button
            variant="ghost"
            icon={<IconCopy size={16} />}
            aria-label="Copy queue ID"
            onClick={() =>
              void navigator.clipboard
                .writeText(queue.data.queue_id ?? "")
                .then(
                  () => feedback.success("Queue ID copied."),
                  (error: unknown) =>
                    feedback.failure(error, "Unable to copy the queue ID."),
                )
            }
          />
        </div>
      ) : null}
      <div className="mb-6 overflow-x-auto">
        <Tabs
          variant="underline"
          value={tab}
          onValueChange={(value) =>
            void navigate({
              to: "/queues/$queueId",
              params: { queueId },
              search: value === "metrics" ? {} : { tab: value as Tab },
            })
          }
          tabs={[
            { value: "metrics", label: "Metrics" },
            { value: "settings", label: "Settings" },
          ]}
          listClassName="min-w-max"
        />
      </div>

      {loading ? (
        <LoadingRows />
      ) : error ? (
        <ErrorState error={error} />
      ) : tab === "metrics" ? (
        <MetricsTab paused={paused} metrics={metrics.data} />
      ) : (
        <SettingsTab
          queue={queue.data!}
          queueId={queueId}
          paused={paused}
          toggling={toggleDelivery.isPending}
          saving={saveSetting.isPending}
          saveError={saveSetting.error}
          onEditStart={() => saveSetting.reset()}
          onSave={(field, value) =>
            saveSetting.mutateAsync({ field, value }).then(() => undefined)
          }
          onRename={openRenameQueueDialog}
          onToggle={() => toggleDelivery.mutate(!paused)}
          onDelete={confirmDeleteQueue}
        />
      )}
    </div>
  );
}

function MetricsTab({
  paused,
  metrics,
}: {
  paused: boolean;
  metrics:
    | {
        backlog_count: number;
        backlog_bytes: number;
        oldest_message_timestamp_ms: number;
      }
    | undefined;
}) {
  return (
    <Section
      title="Realtime metrics"
      description="Best-effort queue measurements from the management API."
    >
      <StatGrid
        items={[
          { label: "Delivery", value: paused ? "Paused" : "Active" },
          { label: "Backlog messages", value: metrics?.backlog_count ?? 0 },
          {
            label: "Backlog bytes",
            value: formatBytes(metrics?.backlog_bytes ?? 0),
          },
          {
            label: "Oldest message",
            value: metrics?.oldest_message_timestamp_ms
              ? formatTimestamp(metrics.oldest_message_timestamp_ms)
              : "None",
          },
        ]}
      />
      <Panel className="mt-3 grid min-h-48 place-items-center text-center">
        <div className="grid max-w-lg gap-1.5">
          <IconGauge className="text-kumo-subtle mx-auto" size={28} />
          <h3 className="font-medium">Point-in-time metrics</h3>
          <p className="text-kumo-subtle">
            Historical charts are not exposed by the supported API. Refresh to
            load the latest backlog snapshot.
          </p>
        </div>
      </Panel>
    </Section>
  );
}

function SettingsTab({
  queue,
  queueId,
  paused,
  toggling,
  saving,
  saveError,
  onEditStart,
  onSave,
  onRename,
  onToggle,
  onDelete,
}: {
  queue: {
    queue_id?: string;
    queue_name?: string;
    created_on?: string;
    producers?: Array<{
      type?: "worker" | "r2_bucket";
      script?: string;
      bucket_name?: string;
    }>;
    settings?: { delivery_delay?: number; message_retention_period?: number };
  };
  queueId: string;
  paused: boolean;
  toggling: boolean;
  saving: boolean;
  saveError: unknown;
  onEditStart: () => void;
  onSave: (
    field: "delivery_delay" | "message_retention_period",
    value: number,
  ) => Promise<void>;
  onRename: () => void;
  onToggle: () => void;
  onDelete: () => void;
}) {
  const [editing, setEditing] = useState<
    "delivery_delay" | "message_retention_period" | null
  >(null);
  const [draft, setDraft] = useState("");
  const current =
    editing === "delivery_delay"
      ? (queue.settings?.delivery_delay ?? 0)
      : (queue.settings?.message_retention_period ?? 345600);
  const value = Number(draft);
  const valid =
    draft !== "" &&
    /^\d+$/.test(draft) &&
    Number.isSafeInteger(value) &&
    (editing === "delivery_delay"
      ? value <= 86_400
      : value >= 60 && value <= 1_209_600) &&
    value !== current;
  const beginEdit = (field: "delivery_delay" | "message_retention_period") => {
    onEditStart();
    setEditing(field);
    setDraft(
      String(
        field === "delivery_delay"
          ? (queue.settings?.delivery_delay ?? 0)
          : (queue.settings?.message_retention_period ?? 345600),
      ),
    );
  };
  const editRow = (
    field: "delivery_delay" | "message_retention_period",
    label: string,
    description: string,
  ) =>
    editing === field ? (
      <form
        className="grid gap-2 px-4 py-4 pr-3"
        onSubmit={(event) => {
          event.preventDefault();
          if (valid && !saving)
            void onSave(field, value).then(
              () => setEditing(null),
              () => undefined,
            );
        }}
      >
        <Input
          label={`${label} (seconds)`}
          type="number"
          min={field === "delivery_delay" ? 0 : 60}
          max={field === "delivery_delay" ? 86_400 : 1_209_600}
          step={1}
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          autoFocus
        />
        <p className="text-kumo-subtle">{description}</p>
        {saveError ? (
          <p role="alert" className="text-kumo-danger">
            {saveError instanceof Error
              ? saveError.message
              : "The request failed."}
          </p>
        ) : null}
        <div className="flex justify-end gap-2">
          <Button
            type="button"
            variant="secondary"
            disabled={saving}
            onClick={() => setEditing(null)}
          >
            Cancel
          </Button>
          <Button type="submit" variant="primary" disabled={!valid || saving}>
            {saving ? "Saving…" : "Save"}
          </Button>
        </div>
      </form>
    ) : (
      <div className="flex min-h-12 min-w-0 items-center gap-3 px-4 py-3 pr-3">
        <span className="w-36 shrink-0 max-sm:w-1/2">{label}</span>
        <span className="min-w-0 flex-1 break-words">
          {field === "delivery_delay"
            ? (queue.settings?.delivery_delay ?? 0)
            : (queue.settings?.message_retention_period ?? 345600)}{" "}
          seconds
        </span>
        <Button
          variant="ghost"
          icon={<IconEdit size={16} />}
          aria-label={`Edit ${label.toLowerCase()}`}
          disabled={saving}
          onClick={() => beginEdit(field)}
        />
      </div>
    );
  return (
    <div className="mx-auto grid max-w-5xl min-w-0 gap-6 lg:grid-cols-4 lg:gap-8">
      <nav
        aria-label="Queue settings sections"
        className="text-kumo-subtle flex gap-4 overflow-x-auto lg:flex-col lg:gap-2 lg:overflow-visible"
      >
        <a href="#queue-producers" className="hover:text-kumo-default shrink-0">
          Producers
        </a>
        <a href="#queue-consumers" className="hover:text-kumo-default shrink-0">
          Consumers
        </a>
        <a href="#queue-general" className="hover:text-kumo-default shrink-0">
          General
        </a>
      </nav>
      <div className="grid min-w-0 gap-8 lg:col-span-3">
        <section id="queue-producers" className="grid gap-3">
          <h2 className="text-base font-semibold">Producers</h2>
          <DataTable
            columns={[
              { key: "type", label: "Type" },
              { key: "name", label: "Name" },
            ]}
            rows={[
              { type: "HTTP", name: "HTTP push" },
              ...(queue.producers ?? []).map((producer) => ({
                type: producer.type === "worker" ? "Worker" : "R2 bucket",
                name:
                  producer.type === "worker"
                    ? (producer.script ?? "Unknown Worker")
                    : (producer.bucket_name ?? "Unknown bucket"),
              })),
            ]}
          />
        </section>
        <QueueConsumerEditor queueId={queueId} />
        <section id="queue-general" className="grid gap-3">
          <h2 className="text-base font-semibold">General</h2>
          <LayerCard className="overflow-hidden p-0">
            <div className="divide-kumo-line divide-y">
              <div className="flex min-h-12 min-w-0 items-center gap-3 px-4 py-3 pr-3">
                <span className="w-36 shrink-0 max-sm:w-1/2">Name</span>
                <span className="min-w-0 break-all">
                  {queue.queue_name ?? "Unknown"}
                </span>
              </div>
              <div className="flex min-h-12 min-w-0 items-center gap-3 px-4 py-3 pr-3">
                <span className="w-36 shrink-0 max-sm:w-1/2">Created</span>
                <span>{formatDate(queue.created_on)}</span>
              </div>
              {editRow(
                "delivery_delay",
                "Delivery delay",
                "Wait this many seconds before making new messages available to consumers.",
              )}
              {editRow(
                "message_retention_period",
                "Message retention",
                "Keep messages for this many seconds before removing them from the queue.",
              )}
              <div className="flex min-h-12 min-w-0 flex-wrap items-center justify-between gap-3 px-4 py-3 pr-3">
                <span>
                  Deleting this queue permanently removes all its messages and
                  configuration.
                </span>
                <Button
                  variant="ghost"
                  className="text-kumo-danger"
                  onClick={onDelete}
                >
                  Delete
                </Button>
              </div>
            </div>
          </LayerCard>
          <div className="flex flex-wrap gap-2 pt-1">
            <Button variant="secondary" onClick={onRename}>
              Rename queue
            </Button>
            <Button
              variant="secondary"
              icon={
                paused ? (
                  <IconPlayerPlay size={16} />
                ) : (
                  <IconPlayerPause size={16} />
                )
              }
              disabled={toggling}
              onClick={onToggle}
            >
              {paused ? "Resume delivery" : "Pause delivery"}
            </Button>
          </div>
        </section>
      </div>
    </div>
  );
}
