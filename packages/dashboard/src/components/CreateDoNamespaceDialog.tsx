import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Input } from "@cloudflare/kumo/components/input";
import { Select } from "@cloudflare/kumo/components/select";
import { OperatorApiError, parseWorkerId } from "@open-compute/operator-sdk";
import type { AccountId, OperatorClient } from "@open-compute/operator-sdk";
import { queryKeys } from "../queries/keys";
import { invalidateDoNamespacesQueries } from "../queries/invalidate";

interface CreateDoNamespaceDialogProps {
  client: OperatorClient;
  accountId: AccountId;
  open: boolean;
  onClose: () => void;
}

export function CreateDoNamespaceDialog({
  client,
  accountId,
  open,
  onClose,
}: CreateDoNamespaceDialogProps) {
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [workerId, setWorkerId] = useState("");
  const [className, setClassName] = useState("");
  const [errorMessage, setErrorMessage] = useState<string | null>(null);

  const workersQuery = useQuery({
    queryKey: queryKeys.workers(accountId),
    queryFn: ({ signal }) => client.workers.list({ accountId, signal }),
    enabled: open,
  });

  useEffect(() => {
    if (!open) {
      setName("");
      setWorkerId("");
      setClassName("");
      setErrorMessage(null);
    }
  }, [open]);

  useEffect(() => {
    if (!workerId && workersQuery.data?.workers.length) {
      setWorkerId(workersQuery.data.workers[0]!.id);
    }
  }, [workerId, workersQuery.data?.workers]);

  const createMutation = useMutation({
    mutationFn: () => client.durableObjects.createNamespace({
      accountId,
      name: name.trim(),
      workerId: parseWorkerId(workerId),
      className: className.trim(),
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async () => {
      await invalidateDoNamespacesQueries(queryClient, accountId);
      onClose();
    },
    onError: error => {
      setErrorMessage(
        error instanceof OperatorApiError ? error.message : "Unable to create the Durable Object namespace.",
      );
    },
  });

  const canSubmit = Boolean(name.trim() && workerId && className.trim());

  return (
    <Dialog.Root open={open} onOpenChange={nextOpen => {
      if (!nextOpen) {
        setErrorMessage(null);
        onClose();
      }
    }}>
      <Dialog className="p-6" size="lg">
      <form
        onSubmit={event => {
          event.preventDefault();
          if (!canSubmit || createMutation.isPending) return;
          createMutation.mutate();
        }}
      >
        <Dialog.Title>Create Durable Object namespace</Dialog.Title>
        <Dialog.Description>
          Namespaces bind to an owner Worker and exported class name from the active deployment.
        </Dialog.Description>
        <div className="mt-4 space-y-4">
          <Input
            label="Namespace name"
            value={name}
            onChange={event => setName(event.target.value)}
            placeholder="MY_NAMESPACE"
            autoFocus
          />
          <div>
            <Select
              label="Owner Worker"
              value={workerId || null}
              placeholder="Select a Worker"
              renderValue={value => {
                const worker = workersQuery.data?.workers.find(candidate => candidate.id === value);
                return worker ? `${worker.name} (${worker.id})` : null;
              }}
              onValueChange={value => setWorkerId(value ?? "")}
              disabled={workersQuery.isLoading || !workersQuery.data?.workers.length}
            >
              {(workersQuery.data?.workers ?? []).map(worker => (
                <Select.Option key={worker.id} value={worker.id}>
                  {worker.name} ({worker.id})
                </Select.Option>
              ))}
            </Select>
            {!workersQuery.isLoading && !workersQuery.data?.workers.length ? (
              <span className="mt-1 block text-kumo-subtle">Create a Worker before registering a namespace.</span>
            ) : null}
          </div>
          <Input
            label="Class name"
            value={className}
            onChange={event => setClassName(event.target.value)}
            placeholder="MyDurableObject"
          />
        </div>
        {errorMessage ? (
          <p className="mt-3 text-sm text-kumo-danger" role="alert">
            {errorMessage}
          </p>
        ) : null}
        <div className="mt-6 flex justify-end gap-2">
          <Button
            variant="secondary"
            type="button"
            onClick={onClose}
            disabled={createMutation.isPending}
          >
            Cancel
          </Button>
          <Button variant="primary" type="submit" disabled={!canSubmit || createMutation.isPending}>
            {createMutation.isPending ? "Creating…" : "Create namespace"}
          </Button>
        </div>
      </form>
      </Dialog>
    </Dialog.Root>
  );
}
