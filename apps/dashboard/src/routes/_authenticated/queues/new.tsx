import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { PageHeader } from "../../../components/dashboard-page";
import { useAuth } from "../../../features/auth/auth-atoms";

export const Route = createFileRoute("/_authenticated/queues/new")({
  component: CreateQueuePage,
});

function CreateQueuePage() {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const queueName = name.trim();
  const create = useMutation({
    mutationFn: async () => {
      if (!client || !selectedInstanceId || !queueName) {
        throw new Error("Enter a queue name.");
      }
      return client.queues.create({
        account_id: selectedInstanceId,
        queue_name: queueName,
      });
    },
    onSuccess: async (queue) => {
      await queryClient.invalidateQueries({
        queryKey: ["cloudflare-v4", "queues", selectedInstanceId],
      });
      if (queue.queue_id) {
        await navigate({
          to: "/queues/$queueId",
          params: { queueId: queue.queue_id },
        });
      } else {
        await navigate({ to: "/queues" });
      }
    },
  });

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (queueName && !create.isPending) create.mutate();
  }

  return (
    <form onSubmit={submit} className="mx-auto max-w-2xl text-sm">
      <PageHeader
        title="Create queue"
        description="Name your queue. Add producers and consumers after creation."
      />
      <div className="mt-8">
        <Input
          label="Name"
          placeholder="Name"
          value={name}
          onChange={(event) => {
            setName(event.target.value);
            create.reset();
          }}
          autoComplete="off"
        />
      </div>
      {create.error ? (
        <p className="text-kumo-danger mt-2" role="alert">
          {create.error instanceof Error
            ? create.error.message
            : "Unable to create the queue."}
        </p>
      ) : null}
      <div className="mt-8 flex justify-end gap-2">
        <Button
          type="button"
          variant="ghost"
          onClick={() => void navigate({ to: "/queues" })}
        >
          Cancel
        </Button>
        <Button
          type="submit"
          variant="primary"
          disabled={
            !queueName || create.isPending || !client || !selectedInstanceId
          }
        >
          {create.isPending ? "Creating…" : "Create"}
        </Button>
      </div>
    </form>
  );
}
