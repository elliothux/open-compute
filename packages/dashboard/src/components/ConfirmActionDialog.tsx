import { useEffect, useState } from "react";
import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Input } from "@cloudflare/kumo/components/input";

interface ConfirmActionDialogProps {
  title: string;
  description: string;
  resourceLabel: string;
  confirmValue: string;
  submitLabel: string;
  submitVariant?: "primary" | "destructive";
  open: boolean;
  errorMessage: string | null;
  isPending: boolean;
  onClose: () => void;
  onConfirm: () => void;
}

export function ConfirmActionDialog({
  title,
  description,
  resourceLabel,
  confirmValue,
  submitLabel,
  submitVariant = "primary",
  open,
  errorMessage,
  isPending,
  onClose,
  onConfirm,
}: ConfirmActionDialogProps) {
  const [typedValue, setTypedValue] = useState("");

  useEffect(() => {
    if (!open) {
      setTypedValue("");
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
          onConfirm();
        }}
      >
        <Dialog.Title>{title}</Dialog.Title>
        <Dialog.Description>{description}</Dialog.Description>
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
          <Button variant={submitVariant} type="submit" disabled={!confirmed || isPending}>
            {isPending ? "Working…" : submitLabel}
          </Button>
        </div>
      </form>
      </Dialog>
    </Dialog.Root>
  );
}
