import { useEffect, useState } from "react";
import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Input } from "@cloudflare/kumo/components/input";

interface RenameResourceDialogProps {
  title: string;
  description: string;
  nameLabel: string;
  currentName: string;
  open: boolean;
  errorMessage: string | null;
  isPending: boolean;
  onClose: () => void;
  onSubmit: (name: string) => void;
}

export function RenameResourceDialog({
  title,
  description,
  nameLabel,
  currentName,
  open,
  errorMessage,
  isPending,
  onClose,
  onSubmit,
}: RenameResourceDialogProps) {
  const [name, setName] = useState(currentName);

  useEffect(() => {
    if (open) {
      setName(currentName);
    }
  }, [currentName, open]);

  return (
    <Dialog.Root open={open} onOpenChange={nextOpen => {
      if (!nextOpen) onClose();
    }}>
      <Dialog className="p-6" size="lg">
      <form
        onSubmit={event => {
          event.preventDefault();
          const trimmed = name.trim();
          if (!trimmed || trimmed === currentName || isPending) return;
          onSubmit(trimmed);
        }}
      >
        <Dialog.Title>{title}</Dialog.Title>
        <Dialog.Description>{description}</Dialog.Description>
        <div className="mt-4">
          <Input
            label={nameLabel}
            value={name}
            onChange={event => setName(event.target.value)}
            autoFocus
          />
        </div>
        {errorMessage ? (
          <p className="mt-3 text-sm text-kumo-danger" role="alert">
            {errorMessage}
          </p>
        ) : null}
        <div className="mt-6 flex justify-end gap-2">
          <Button variant="secondary" type="button" onClick={onClose} disabled={isPending}>
            Cancel
          </Button>
          <Button
            variant="primary"
            type="submit"
            disabled={!name.trim() || name.trim() === currentName || isPending}
          >
            {isPending ? "Saving…" : "Save"}
          </Button>
        </div>
      </form>
      </Dialog>
    </Dialog.Root>
  );
}
