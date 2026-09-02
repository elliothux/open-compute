import { useEffect, useState } from "react";
import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Input } from "@cloudflare/kumo/components/input";
import { Checkbox } from "@cloudflare/kumo/components/checkbox";

interface ConfirmDeleteResourceDialogProps {
  title: string;
  description: string;
  resourceLabel: string;
  confirmValue: string;
  open: boolean;
  errorMessage: string | null;
  isPending: boolean;
  forceOption?: {
    label: string;
    description: string;
  };
  onClose: () => void;
  onConfirm: (options: { force: boolean }) => void;
}

export function ConfirmDeleteResourceDialog({
  title,
  description,
  resourceLabel,
  confirmValue,
  open,
  errorMessage,
  isPending,
  forceOption,
  onClose,
  onConfirm,
}: ConfirmDeleteResourceDialogProps) {
  const [typedValue, setTypedValue] = useState("");
  const [force, setForce] = useState(false);

  useEffect(() => {
    if (!open) {
      setTypedValue("");
      setForce(false);
    }
  }, [open]);

  const confirmed = typedValue === confirmValue;

  return (
    <Dialog.Root role="alertdialog" open={open} onOpenChange={nextOpen => {
      if (!nextOpen) onClose();
    }}>
      <Dialog className="p-6" size="lg">
      <form
        onSubmit={event => {
          event.preventDefault();
          if (!confirmed || isPending) return;
          onConfirm({ force });
        }}
      >
        <Dialog.Title>{title}</Dialog.Title>
        <Dialog.Description>{description}</Dialog.Description>
        {forceOption ? (
          <Checkbox
            className="mt-4"
            checked={force}
            onCheckedChange={checked => setForce(checked === true)}
            label={(
              <span>
              <span className="font-medium text-kumo-default">{forceOption.label}</span>
              <span className="mt-1 block text-kumo-subtle">{forceOption.description}</span>
              </span>
            )}
          />
        ) : null}
        <p className="mt-4 text-sm text-kumo-subtle">
          Enter <code className="break-all [font-size:0.9em] text-kumo-default">{confirmValue}</code> to continue.
        </p>
        <div className="mt-4">
          <Input
            label={`Type ${resourceLabel} to confirm`}
            value={typedValue}
            onChange={event => setTypedValue(event.target.value)}
            placeholder={confirmValue}
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
          <Button variant="destructive" type="submit" disabled={!confirmed || isPending}>
            {isPending ? "Deleting…" : "Delete"}
          </Button>
        </div>
      </form>
      </Dialog>
    </Dialog.Root>
  );
}
