import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Input } from "@cloudflare/kumo/components/input";
import { useState } from "react";

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

export function RenameResourceDialog(props: RenameResourceDialogProps) {
  return (
    <Dialog.Root
      open={props.open}
      onOpenChange={(open) => !open && props.onClose()}
    >
      {props.open ? <RenameResourceForm {...props} /> : null}
    </Dialog.Root>
  );
}

function RenameResourceForm({
  title,
  description,
  nameLabel,
  currentName,
  errorMessage,
  isPending,
  onClose,
  onSubmit,
}: RenameResourceDialogProps) {
  const [name, setName] = useState(currentName);
  const trimmed = name.trim();
  const unchanged = !trimmed || trimmed === currentName;
  return (
    <Dialog className="p-6" size="lg">
      <form
        onSubmit={(event) => {
          event.preventDefault();
          if (!unchanged && !isPending) onSubmit(trimmed);
        }}
      >
        <Dialog.Title>{title}</Dialog.Title>
        <Dialog.Description>{description}</Dialog.Description>
        <div className="mt-4">
          <Input
            label={nameLabel}
            value={name}
            onChange={(event) => setName(event.target.value)}
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
            disabled={unchanged || isPending}
          >
            {isPending ? "Saving…" : "Save"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
