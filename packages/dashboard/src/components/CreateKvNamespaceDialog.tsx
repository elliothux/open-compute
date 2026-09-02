import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Input } from "@cloudflare/kumo/components/input";
import { OperatorApiError } from "@open-compute/operator-sdk";
import type { AccountId, OperatorClient } from "@open-compute/operator-sdk";
import { invalidateKvNamespacesQueries } from "../queries/invalidate";

interface CreateKvNamespaceDialogProps {
  client: OperatorClient;
  accountId: AccountId;
  open: boolean;
  onClose: () => void;
}

export function CreateKvNamespaceDialog({
  client,
  accountId,
  open,
  onClose,
}: CreateKvNamespaceDialogProps) {
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [errorMessage, setErrorMessage] = useState<string | null>(null);

  const createMutation = useMutation({
    mutationFn: () => client.kv.createNamespace({
      accountId,
      name: name.trim(),
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async () => {
      await invalidateKvNamespacesQueries(queryClient, accountId);
      setName("");
      setErrorMessage(null);
      onClose();
    },
    onError: error => {
      if (error instanceof OperatorApiError) {
        setErrorMessage(error.message);
      } else {
        setErrorMessage("Unable to create the KV namespace.");
      }
    },
  });

  return (
    <Dialog.Root open={open} onOpenChange={nextOpen => {
      if (!nextOpen) {
        setName("");
        setErrorMessage(null);
        onClose();
      }
    }}>
      <Dialog className="p-6" size="lg">
      <form
        onSubmit={event => {
          event.preventDefault();
          if (!name.trim() || createMutation.isPending) return;
          createMutation.mutate();
        }}
      >
        <Dialog.Title>Create KV namespace</Dialog.Title>
        <Dialog.Description>
          Namespaces are account-scoped resources managed through the Operator API.
        </Dialog.Description>
        <div className="mt-4">
          <Input
            label="Namespace name"
            value={name}
            onChange={event => setName(event.target.value)}
            placeholder="MY_NAMESPACE"
            autoFocus
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
            onClick={() => {
              setName("");
              setErrorMessage(null);
              onClose();
            }}
            disabled={createMutation.isPending}
          >
            Cancel
          </Button>
          <Button
            variant="primary"
            type="submit"
            disabled={!name.trim() || createMutation.isPending}
          >
            {createMutation.isPending ? "Creating…" : "Create namespace"}
          </Button>
        </div>
      </form>
      </Dialog>
    </Dialog.Root>
  );
}
