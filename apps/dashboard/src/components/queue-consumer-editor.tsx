import { Badge } from "@cloudflare/kumo/components/badge";
import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import { LayerCard } from "@cloudflare/kumo/components/layer-card";
import { Select } from "@cloudflare/kumo/components/select";
import { IconPlus } from "@tabler/icons-react";
import {
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { useState } from "react";
import type { Consumer } from "@open-compute/sdk";
import { useAuth } from "../features/auth/auth-atoms";
import { useMutationFeedback } from "../features/toast/use-mutation-feedback";
import { DefinitionList, ErrorState, LoadingRows } from "./dashboard-page";
import { DataTable } from "./page-layout";
import { openConfirmDeleteDialog } from "./resource-dialog";
import { RowActionsMenu } from "./row-actions-menu";

type Draft = {
  id?: string;
  scriptName: string;
  batchSize: string;
  waitSeconds: string;
  maxRetries: string;
  retryDelay: string;
  maxConcurrency: string;
  deadLetterQueue: string;
};

type Runtime = {
  projection_exists: boolean;
  backlog_messages: number;
  ready_messages: number;
  claimed_batches: number;
  claimed_messages: number;
  dlq_pending: number;
};

const integer = (value: string, minimum: number, optional = false) =>
  (optional && value === "") ||
  (/^\d+$/.test(value) &&
    Number.isSafeInteger(Number(value)) &&
    Number(value) >= minimum);

function fromConsumer(consumer?: Consumer): Draft {
  const settings = consumer?.settings;
  return {
    ...(consumer?.consumer_id ? { id: consumer.consumer_id } : {}),
    scriptName:
      consumer && "script_name" in consumer ? (consumer.script_name ?? "") : "",
    batchSize: String(settings?.batch_size ?? 10),
    waitSeconds:
      settings && "max_wait_time_ms" in settings
        ? String((settings.max_wait_time_ms ?? 5000) / 1000)
        : "5",
    maxRetries: String(settings?.max_retries ?? 3),
    retryDelay: String(settings?.retry_delay ?? 0),
    maxConcurrency:
      settings && "max_concurrency" in settings
        ? String(settings.max_concurrency ?? "")
        : "",
    deadLetterQueue: consumer?.dead_letter_queue ?? "",
  };
}

export function QueueConsumerEditor({ queueId }: { queueId: string }) {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const [draft, setDraft] = useState<Draft | null>(null);
  const enabled = client !== null && selectedInstanceId !== null;
  const consumers = useQuery({
    queryKey: [
      "cloudflare-v4",
      "queues",
      selectedInstanceId,
      queueId,
      "consumers",
    ],
    queryFn: ({ signal }) =>
      client!.queues.consumers.list(
        queueId,
        { account_id: selectedInstanceId! },
        { signal },
      ),
    enabled,
  });
  const workers = useQuery({
    queryKey: ["cloudflare-v4", "workers", selectedInstanceId],
    queryFn: ({ signal }) =>
      client!.workers.scripts.list(
        { account_id: selectedInstanceId! },
        { signal },
      ),
    enabled: enabled && draft !== null,
  });
  const rows = consumers.data?.result ?? [];
  const runtimes = useQueries({
    queries: rows.map((consumer) => ({
      queryKey: [
        "open-compute",
        "queues",
        selectedInstanceId,
        queueId,
        "consumers",
        consumer.consumer_id,
        "runtime",
      ],
      queryFn: ({ signal }: { signal: AbortSignal }) =>
        client!.openCompute.queues.consumerRuntime(
          selectedInstanceId!,
          queueId,
          consumer.consumer_id!,
          { signal },
        ),
      enabled: enabled && Boolean(consumer.consumer_id),
    })),
  });
  const refresh = () =>
    queryClient.invalidateQueries({
      queryKey: ["cloudflare-v4", "queues", selectedInstanceId, queueId],
    });
  const save = useMutation({
    mutationFn: (input: Draft) =>
      saveConsumer(client!, selectedInstanceId!, queueId, input),
    onSuccess: async () => {
      setDraft(null);
      await refresh();
      feedback.success("Consumer saved.");
    },
    onError: (error) => feedback.failure(error, "Unable to save the consumer."),
  });
  function confirmDeleteConsumer(id: string, label: string) {
    openConfirmDeleteDialog({
      name: label,
      confirm: async () => {
        try {
          await client!.queues.consumers.delete(id, {
            account_id: selectedInstanceId!,
            queue_id: queueId,
          });
        } catch (error) {
          feedback.failure(error, "Unable to delete the consumer.");
          throw error;
        }
        await refresh();
        feedback.success("Consumer deleted.");
      },
    });
  }

  const valid =
    draft !== null &&
    Boolean(draft.scriptName.trim()) &&
    integer(draft.batchSize, 1) &&
    Number(draft.batchSize) <= 100 &&
    integer(draft.maxRetries, 0) &&
    Number(draft.maxRetries) <= 100 &&
    integer(draft.retryDelay, 0) &&
    Number(draft.retryDelay) <= 86_400 &&
    integer(draft.waitSeconds, 0) &&
    Number(draft.waitSeconds) <= 60 &&
    integer(draft.maxConcurrency, 1, true);
  const change = (field: keyof Draft, value: string) =>
    setDraft((current) => (current ? { ...current, [field]: value } : null));

  return (
    <section id="queue-consumers" className="grid min-w-0 gap-3">
      <div className="flex items-center justify-between gap-3">
        <h2 className="text-base font-semibold">Consumers</h2>
        <Button
          variant="ghost"
          icon={<IconPlus size={16} />}
          onClick={() => {
            save.reset();
            setDraft(fromConsumer());
          }}
        >
          Add
        </Button>
      </div>
      {consumers.isLoading ? (
        <LoadingRows />
      ) : consumers.error ? (
        <ErrorState error={consumers.error} />
      ) : rows.length === 0 && draft === null ? (
        <LayerCard className="text-kumo-subtle px-4 py-3 text-center">
          No consumers configured
        </LayerCard>
      ) : rows.length > 0 ? (
        <DataTable
          columns={[
            { key: "name", label: "Name" },
            { key: "type", label: "Type" },
            { key: "batch", label: "Batch size" },
            { key: "actions", label: "" },
          ]}
          rows={rows.map((consumer) => {
            const id = consumer.consumer_id ?? "";
            const label =
              consumer.type === "worker" ? (consumer.script_name ?? id) : id;
            return {
              name: label,
              type: consumer.type === "worker" ? "Worker" : "HTTP pull",
              batch: consumer.settings?.batch_size ?? "Default",
              actions: (
                <RowActionsMenu
                  label={label}
                  actions={[
                    {
                      id: "edit",
                      label: "Edit consumer",
                      onSelect: () => {
                        save.reset();
                        setDraft(fromConsumer(consumer));
                      },
                    },
                    {
                      id: "delete",
                      label: "Delete consumer",
                      variant: "danger",
                      onSelect: () => confirmDeleteConsumer(id, label),
                    },
                  ]}
                />
              ),
            };
          })}
        />
      ) : null}
      {rows.map((consumer, index) => {
        const runtime = runtimes[index];
        if (!consumer.consumer_id) return null;
        return (
          <details
            key={consumer.consumer_id}
            className="ring-kumo-line min-w-0 rounded-lg px-4 py-3 ring"
          >
            <summary className="cursor-pointer">
              Runtime:{" "}
              {consumer.type === "worker"
                ? consumer.script_name
                : consumer.consumer_id}
            </summary>
            {runtime?.isLoading ? (
              <p className="text-kumo-subtle mt-3">Loading runtime…</p>
            ) : runtime?.error ? (
              <p className="text-kumo-danger mt-3">Runtime unavailable.</p>
            ) : runtime?.data ? (
              <div className="mt-3 grid gap-2">
                <Badge
                  variant={
                    runtime.data.projection_exists ? "success" : "warning"
                  }
                  appearance="dot"
                >
                  {runtime.data.projection_exists ? "Ready" : "Pending"}
                </Badge>
                <DefinitionList items={runtimeItems(runtime.data)} />
              </div>
            ) : null}
          </details>
        );
      })}
      {draft ? (
        <LayerCard className="px-5 py-4">
          <form
            className="grid gap-3"
            onSubmit={(event) => {
              event.preventDefault();
              if (valid && !save.isPending) save.mutate(draft);
            }}
          >
            <h3 className="sr-only">
              {draft.id ? "Edit consumer" : "Add consumer"}
            </h3>
            <Select
              className="w-full"
              label="Type"
              value="worker"
              onValueChange={() => undefined}
            >
              <Select.Option value="worker">Worker</Select.Option>
              <Select.Option value="http_pull" disabled>
                HTTP pull (not supported)
              </Select.Option>
            </Select>
            {workers.error ? (
              <Input
                label="Worker"
                value={draft.scriptName}
                onChange={(event) => change("scriptName", event.target.value)}
              />
            ) : (
              <Select
                className="w-full"
                label="Worker"
                value={draft.scriptName}
                onValueChange={(value) => change("scriptName", value ?? "")}
              >
                <Select.Option value="">Select a Worker</Select.Option>
                {(workers.data?.result ?? []).map((worker) =>
                  worker.id ? (
                    <Select.Option key={worker.id} value={worker.id}>
                      {worker.id}
                    </Select.Option>
                  ) : null,
                )}
                {draft.scriptName &&
                !(workers.data?.result ?? []).some(
                  (worker) => worker.id === draft.scriptName,
                ) ? (
                  <Select.Option value={draft.scriptName}>
                    {draft.scriptName}
                  </Select.Option>
                ) : null}
              </Select>
            )}
            <p className="text-kumo-subtle -mt-2">
              Worker that handles messages from this queue.
            </p>
            <Input
              label="Batch size"
              type="number"
              min={1}
              max={100}
              step={1}
              value={draft.batchSize}
              onChange={(event) => change("batchSize", event.target.value)}
            />
            <p className="text-kumo-subtle -mt-2">
              Maximum messages delivered in each batch.
            </p>
            <Input
              label="Max wait (seconds)"
              type="number"
              min={0}
              max={60}
              step={1}
              value={draft.waitSeconds}
              onChange={(event) => change("waitSeconds", event.target.value)}
            />
            <p className="text-kumo-subtle -mt-2">
              Maximum time to wait for a full batch.
            </p>
            <Input
              label="Max retries"
              type="number"
              min={0}
              max={100}
              step={1}
              value={draft.maxRetries}
              onChange={(event) => change("maxRetries", event.target.value)}
            />
            <p className="text-kumo-subtle -mt-2">
              Maximum redelivery attempts before a message is dropped.
            </p>
            <Input
              label="Retry delay (seconds)"
              type="number"
              min={0}
              max={86_400}
              step={1}
              value={draft.retryDelay}
              onChange={(event) => change("retryDelay", event.target.value)}
            />
            <p className="text-kumo-subtle -mt-2">
              Minimum delay before retrying a message.
            </p>
            <Input
              label="Max consumer concurrency"
              type="number"
              min={1}
              step={1}
              value={draft.maxConcurrency}
              onChange={(event) => change("maxConcurrency", event.target.value)}
            />
            <p className="text-kumo-subtle -mt-2">
              Leave empty for automatic concurrency.
            </p>
            <Input
              label="Dead-letter queue"
              value={draft.deadLetterQueue}
              onChange={(event) =>
                change("deadLetterQueue", event.target.value)
              }
            />
            <p className="text-kumo-subtle -mt-2">
              Queue for messages that cannot be delivered.
            </p>
            {save.error ? (
              <p role="alert" className="text-kumo-danger">
                {save.error.message}
              </p>
            ) : null}
            <div className="flex justify-end gap-2">
              <Button
                type="button"
                variant="secondary"
                disabled={save.isPending}
                onClick={() => setDraft(null)}
              >
                Cancel
              </Button>
              <Button
                type="submit"
                variant="primary"
                disabled={!valid || save.isPending}
              >
                {save.isPending ? "Saving…" : draft.id ? "Save" : "Add"}
              </Button>
            </div>
          </form>
        </LayerCard>
      ) : null}
    </section>
  );
}

function runtimeItems(runtime: Runtime) {
  return [
    { label: "Backlog", value: `${runtime.backlog_messages} messages` },
    { label: "Ready", value: runtime.ready_messages },
    {
      label: "Claimed",
      value: `${runtime.claimed_batches} batches / ${runtime.claimed_messages} messages`,
    },
    { label: "Dead-letter pending", value: runtime.dlq_pending },
  ];
}

async function saveConsumer(
  client: NonNullable<ReturnType<typeof useAuth>["client"]>,
  selectedInstanceId: string,
  queueId: string,
  draft: Draft,
) {
  const common = {
    account_id: selectedInstanceId,
    dead_letter_queue: draft.deadLetterQueue,
  };
  const settings = {
    batch_size: Number(draft.batchSize),
    max_wait_time_ms: Number(draft.waitSeconds) * 1000,
    max_retries: Number(draft.maxRetries),
    retry_delay: Number(draft.retryDelay),
    ...(draft.maxConcurrency
      ? { max_concurrency: Number(draft.maxConcurrency) }
      : {}),
  };
  const body = {
    ...common,
    type: "worker" as const,
    script_name: draft.scriptName.trim(),
    settings,
  };
  return draft.id
    ? client.queues.consumers.update(draft.id, { ...body, queue_id: queueId })
    : client.queues.consumers.create(queueId, body);
}
