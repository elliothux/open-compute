import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Input } from "@cloudflare/kumo/components/input";
import { useState } from "react";

interface CreateResourceDialogProps {
  title: string;
  description: string;
  nameLabel: string;
  namePlaceholder: string;
  submitLabel: string;
  open: boolean;
  errorMessage: string | null;
  isPending: boolean;
  onClose: () => void;
  onSubmit: (name: string) => void;
}

export function CreateResourceDialog(props: CreateResourceDialogProps) {
  return (
    <Dialog.Root
      open={props.open}
      onOpenChange={(open) => !open && props.onClose()}
    >
      {props.open ? <CreateResourceForm {...props} /> : null}
    </Dialog.Root>
  );
}

function CreateResourceForm({
  title,
  description,
  nameLabel,
  namePlaceholder,
  submitLabel,
  errorMessage,
  isPending,
  onClose,
  onSubmit,
}: CreateResourceDialogProps) {
  const [name, setName] = useState("");
  const trimmed = name.trim();
  return (
    <Dialog className="p-6" size="lg">
      <form
        onSubmit={(event) => {
          event.preventDefault();
          if (trimmed && !isPending) onSubmit(trimmed);
        }}
      >
        <Dialog.Title>{title}</Dialog.Title>
        <Dialog.Description>{description}</Dialog.Description>
        <div className="mt-4">
          <Input
            label={nameLabel}
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder={namePlaceholder}
            autoFocus
          />
        </div>
        {errorMessage ? (
          <p className="text-kumo-danger mt-3 text-sm" role="alert">
            {errorMessage}
          </p>
        ) : null}
        <div className="mt-6 flex justify-end gap-2">
          <Button
            variant="secondary"
            type="button"
            onClick={onClose}
            disabled={isPending}
          >
            Cancel
          </Button>
          <Button
            variant="primary"
            type="submit"
            disabled={!trimmed || isPending}
          >
            {isPending ? "Creating…" : submitLabel}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
