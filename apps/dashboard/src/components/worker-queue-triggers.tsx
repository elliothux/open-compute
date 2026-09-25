import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Input } from "@cloudflare/kumo/components/input";
import { Select } from "@cloudflare/kumo/components/select";
import {
  IconExternalLink,
  IconPlus,
  IconSettings,
  IconTrash,
} from "@tabler/icons-react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import type { QueueConsumer } from "@open-compute/sdk";
import { useAuth } from "../features/auth/auth-atoms";
import { useMutationFeedback } from "../features/toast/use-mutation-feedback";
import { ErrorState } from "./dashboard-page";

type Draft = {
  key: string;
  id?: string;
  originalQueueId?: string;
  queueId: string;
  batchSize: string;
  waitSeconds: string;
  maxRetries: string;
  retryDelay: string;
  maxConcurrency: string;
  deadLetterQueue: string;
};

const integer = (
  value: string,
  minimum: number,
  maximum: number,
  optional = false,
) =>
  (optional && value === "") ||
  (/^\d+$/.test(value) &&
    Number.isSafeInteger(Number(value)) &&
    Number(value) >= minimum &&
    Number(value) <= maximum);

const valid = (draft: Draft) =>
  Boolean(draft.queueId) &&
  integer(draft.batchSize, 1, 100) &&
  integer(draft.waitSeconds, 0, 60) &&
  integer(draft.maxRetries, 0, 100) &&
  integer(draft.retryDelay, 0, 86_400) &&
  integer(draft.maxConcurrency, 1, Number.MAX_SAFE_INTEGER, true);

function newDraft(): Draft {
  return {
    key: crypto.randomUUID(),
    queueId: "",
    batchSize: "10",
    waitSeconds: "5",
    maxRetries: "3",
    retryDelay: "0",
    maxConcurrency: "",
    deadLetterQueue: "",
  };
}

function fromConsumer(consumer: QueueConsumer, queueId: string): Draft {
  return {
    key: consumer.consumer_id,
    id: consumer.consumer_id,
    originalQueueId: queueId,
    queueId,
    batchSize: String(consumer.settings.batch_size),
    waitSeconds: String(consumer.settings.max_wait_time_ms / 1000),
    maxRetries: String(consumer.settings.max_retries),
    retryDelay: String(consumer.settings.retry_delay),
    maxConcurrency: consumer.settings.max_concurrency
      ? String(consumer.settings.max_concurrency)
      : "",
    deadLetterQueue: consumer.dead_letter_queue ?? "",
  };
}

export function WorkerQueueTriggers({
  workerId,
  consumers,
}: {
  workerId: string;
  consumers: readonly QueueConsumer[];
}) {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const [drafts, setDrafts] = useState<Draft[] | null>(null);
  const [editing, setEditing] = useState<Draft | null>(null);
  const [sendToDlq, setSendToDlq] = useState(false);
  const queues = useQuery({
    queryKey: ["cloudflare-v4", "queues", selectedInstanceId],
    queryFn: ({ signal }) =>
      client!.queues.list({ account_id: selectedInstanceId! }, { signal }),
    enabled: client !== null && selectedInstanceId !== null,
  });
  const catalog = queues.data?.result ?? [];
  const committed = consumers.map((consumer) =>
    fromConsumer(
      consumer,
      catalog.find((queue) => queue.queue_name === consumer.queue_name)
        ?.queue_id ?? "",
    ),
  );
  const rows = drafts ?? committed;
  const used = rows.map((row) => row.queueId).filter(Boolean);
  const canSave =
    drafts !== null &&
    drafts.every(valid) &&
    new Set(used).size === used.length &&
    !queues.isLoading &&
    !queues.error &&
    committed.every((row) => Boolean(row.originalQueueId));

  const save = useMutation({
    mutationFn: async (next: Draft[]) => {
      for (const consumer of consumers) {
        const prior = committed.find((row) => row.id === consumer.consumer_id);
        const current = next.find((row) => row.id === consumer.consumer_id);
        if (
          prior?.originalQueueId &&
          (!current || current.queueId !== prior.queueId)
        ) {
          await client!.queues.consumers.delete(consumer.consumer_id, {
            account_id: selectedInstanceId!,
            queue_id: prior.originalQueueId,
          });
        }
      }
      for (const row of next) {
        const prior = committed.find((item) => item.id === row.id);
        if (prior && JSON.stringify(prior) === JSON.stringify(row)) continue;
        const body = {
          account_id: selectedInstanceId!,
          type: "worker" as const,
          script_name: workerId,
          dead_letter_queue: row.deadLetterQueue,
          settings: {
            batch_size: Number(row.batchSize),
            max_wait_time_ms: Number(row.waitSeconds) * 1000,
            max_retries: Number(row.maxRetries),
            retry_delay: Number(row.retryDelay),
            ...(row.maxConcurrency
              ? { max_concurrency: Number(row.maxConcurrency) }
              : {}),
          },
        };
        if (row.id && prior?.queueId === row.queueId) {
          await client!.queues.consumers.update(row.id, {
            ...body,
            queue_id: row.queueId,
          });
        } else {
          await client!.queues.consumers.create(row.queueId, body);
        }
      }
    },
    onSuccess: async () => {
      setDrafts(null);
      await Promise.all([
        queryClient.invalidateQueries({
          queryKey: [
            "cloudflare-v4",
            "workers",
            selectedInstanceId,
            workerId,
            "queue-consumers",
          ],
        }),
        queryClient.invalidateQueries({
          queryKey: ["cloudflare-v4", "queues", selectedInstanceId],
        }),
      ]);
      feedback.success("Queue triggers saved.");
    },
    onError: async (error) => {
      // A multi-row save can partially succeed: reload authority before another attempt.
      setDrafts(null);
      await queryClient.invalidateQueries({
        queryKey: [
          "cloudflare-v4",
          "workers",
          selectedInstanceId,
          workerId,
          "queue-consumers",
        ],
      });
      feedback.failure(error, "Unable to save Queue triggers.");
    },
  });

  const add = () => {
    save.reset();
    setDrafts([...(drafts ?? committed), newDraft()]);
  };
  const change = (key: string, field: keyof Draft, value: string) =>
    setDrafts((current) =>
      (current ?? committed).map((row) =>
        row.key === key ? { ...row, [field]: value } : row,
      ),
    );
  const remove = (key: string) => {
    save.reset();
    setDrafts((current) =>
      (current ?? committed).filter((row) => row.key !== key),
    );
  };

  return (
    <>
      <div className="bg-kumo-recessed min-w-0 overflow-hidden rounded-xl p-1">
        <div className="px-4 py-3 font-medium">Queues</div>
        {queues.error ? (
          <div className="px-4 pb-3">
            <ErrorState error={queues.error} />
          </div>
        ) : rows.length ? (
          <div className="grid gap-2 px-4 pb-3">
            {rows.map((row) => (
              <div
                key={row.key}
                className="bg-kumo-base ring-kumo-line grid min-w-0 gap-2 rounded-lg p-2 ring sm:grid-cols-4"
              >
                <Select
                  className="w-full min-w-0"
                  aria-label="Queue"
                  value={row.queueId}
                  placeholder="Select a Queue"
                  renderValue={(value) =>
                    catalog.find((queue) => queue.queue_id === value)
                      ?.queue_name ?? value
                  }
                  onValueChange={(value) =>
                    change(row.key, "queueId", value ?? "")
                  }
                >
                  <Select.Option value="">Select a Queue</Select.Option>
                  {catalog.map((queue) =>
                    queue.queue_id ? (
                      <Select.Option
                        key={queue.queue_id}
                        value={queue.queue_id}
                      >
                        {queue.queue_name ?? queue.queue_id}
                      </Select.Option>
                    ) : null,
                  )}
                </Select>
                <Button
                  variant="secondary"
                  disabled={!row.queueId}
                  onClick={() => {
                    setSendToDlq(Boolean(row.deadLetterQueue));
                    setEditing({ ...row });
                  }}
                >
                  <IconSettings size={16} /> Message processing
                </Button>
                {row.queueId ? (
                  <Link
                    to="/queues/$queueId"
                    params={{ queueId: row.queueId }}
                    className="ring-kumo-line hover:bg-kumo-tint flex min-h-9 items-center justify-center gap-2 rounded-md px-3 ring"
                  >
                    <IconExternalLink size={16} /> Queue details
                  </Link>
                ) : (
                  <Button variant="secondary" disabled>
                    Queue details
                  </Button>
                )}
                <Button
                  variant="secondary"
                  shape="square"
                  aria-label={`Remove Queue trigger ${row.queueId || row.key}`}
                  onClick={() => remove(row.key)}
                >
                  <IconTrash size={16} />
                </Button>
              </div>
            ))}
          </div>
        ) : (
          <p className="text-kumo-subtle border-kumo-line mx-4 mb-3 rounded-md border border-dashed px-3 py-3">
            No Queue consumers configured.
          </p>
        )}
        <div className="flex justify-end px-4 py-2">
          <Button
            variant="ghost"
            disabled={queues.isLoading || save.isPending}
            onClick={add}
          >
            <IconPlus size={16} /> Add
          </Button>
        </div>
      </div>
      {drafts !== null ? (
        <div className="bg-kumo-contrast text-kumo-inverse fixed inset-x-4 bottom-4 z-40 mx-auto flex max-w-md items-center justify-between gap-3 rounded-lg px-4 py-3 shadow-lg">
          <span className="min-w-0">Unsaved changes</span>
          <div className="flex gap-2">
            <Button
              variant="secondary"
              disabled={save.isPending}
              onClick={() => setDrafts(null)}
            >
              Discard
            </Button>
            <Button
              variant="primary"
              disabled={!canSave || save.isPending}
              onClick={() => save.mutate(drafts)}
            >
              {save.isPending ? "Saving…" : "Save"}
            </Button>
          </div>
        </div>
      ) : null}
      <Dialog.Root
        open={editing !== null}
        onOpenChange={(open) => {
          if (!open) setEditing(null);
        }}
      >
        <Dialog className="max-h-dvh overflow-y-auto px-6 py-5" size="lg">
          <Dialog.Title>Message processing</Dialog.Title>
          <Dialog.Description>
            Configure optional message processing for{" "}
            {catalog.find((queue) => queue.queue_id === editing?.queueId)
              ?.queue_name ?? "this Queue"}
            .
          </Dialog.Description>
          {editing ? (
            <form
              className="mt-5 grid gap-3"
              onSubmit={(event) => {
                event.preventDefault();
                if (
                  !valid(editing) ||
                  (sendToDlq && !editing.deadLetterQueue.trim())
                )
                  return;
                setDrafts((current) =>
                  (current ?? committed).map((row) =>
                    row.key === editing.key ? editing : row,
                  ),
                );
                setEditing(null);
              }}
            >
              <Input
                label="Batch size"
                type="number"
                min={1}
                max={100}
                value={editing.batchSize}
                onChange={(event) =>
                  setEditing({ ...editing, batchSize: event.target.value })
                }
              />
              <Input
                label="Message wait time (seconds)"
                type="number"
                min={0}
                max={60}
                value={editing.waitSeconds}
                onChange={(event) =>
                  setEditing({ ...editing, waitSeconds: event.target.value })
                }
              />
              <Input
                label="Message retries"
                type="number"
                min={0}
                max={100}
                value={editing.maxRetries}
                onChange={(event) =>
                  setEditing({ ...editing, maxRetries: event.target.value })
                }
              />
              <Input
                label="Retry delay (seconds)"
                type="number"
                min={0}
                max={86400}
                value={editing.retryDelay}
                onChange={(event) =>
                  setEditing({ ...editing, retryDelay: event.target.value })
                }
              />
              <Input
                label="Maximum consumer concurrency"
                type="number"
                min={1}
                placeholder="Automatic (recommended)"
                value={editing.maxConcurrency}
                onChange={(event) =>
                  setEditing({ ...editing, maxConcurrency: event.target.value })
                }
              />
              <fieldset className="grid gap-2">
                <legend className="mb-1 font-medium">On message failure</legend>
                <label className="flex items-center gap-2">
                  <input
                    type="radio"
                    name="failure"
                    checked={!sendToDlq}
                    onChange={() => {
                      setSendToDlq(false);
                      setEditing({ ...editing, deadLetterQueue: "" });
                    }}
                  />{" "}
                  Drop permanently
                </label>
                <label className="flex items-center gap-2">
                  <input
                    type="radio"
                    name="failure"
                    checked={sendToDlq}
                    onChange={() => setSendToDlq(true)}
                  />{" "}
                  Send to dead-letter queue
                </label>
              </fieldset>
              {sendToDlq ? (
                <Select
                  className="w-full"
                  label="Dead-letter queue"
                  placeholder="Select a Queue"
                  value={editing.deadLetterQueue}
                  onValueChange={(value) =>
                    setEditing({ ...editing, deadLetterQueue: value ?? "" })
                  }
                >
                  <Select.Option value="">Select a Queue</Select.Option>
                  {catalog
                    .filter((queue) => queue.queue_id !== editing.queueId)
                    .map((queue) =>
                      queue.queue_name ? (
                        <Select.Option
                          key={queue.queue_id}
                          value={queue.queue_name}
                        >
                          {queue.queue_name}
                        </Select.Option>
                      ) : null,
                    )}
                </Select>
              ) : null}
              <div className="flex justify-end gap-2 pt-2">
                <Button
                  type="button"
                  variant="secondary"
                  onClick={() => setEditing(null)}
                >
                  Cancel
                </Button>
                <Button
                  type="submit"
                  variant="primary"
                  disabled={
                    !valid(editing) ||
                    (sendToDlq && !editing.deadLetterQueue.trim())
                  }
                >
                  Update
                </Button>
              </div>
            </form>
          ) : null}
        </Dialog>
      </Dialog.Root>
    </>
  );
}
