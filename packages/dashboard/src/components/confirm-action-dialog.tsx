import { Button } from "@cloudflare/kumo/components/button";
import { Checkbox } from "@cloudflare/kumo/components/checkbox";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Input } from "@cloudflare/kumo/components/input";
import { useState } from "react";

interface ConfirmActionDialogProps {
  title: string;
  description: string;
  resourceLabel: string;
  confirmValue: string;
  submitLabel?: string;
  submitVariant?: "primary" | "destructive";
  open: boolean;
  errorMessage: string | null;
  isPending: boolean;
  forceOption?: { label: string; description: string };
  onClose: () => void;
  onConfirm: (options: { force: boolean }) => void;
}

export function ConfirmActionDialog(props: ConfirmActionDialogProps) {
  return (
    <Dialog.Root
      open={props.open}
      role="alertdialog"
      onOpenChange={(open) => !open && props.onClose()}
    >
      {props.open ? <ConfirmActionForm {...props} /> : null}
    </Dialog.Root>
  );
}

function ConfirmActionForm({
  title,
  description,
  resourceLabel,
  confirmValue,
  submitLabel = "Delete",
  submitVariant = "primary",
  errorMessage,
  isPending,
  forceOption,
  onClose,
  onConfirm,
}: ConfirmActionDialogProps) {
  const [typedValue, setTypedValue] = useState("");
  const [force, setForce] = useState(false);
  const confirmed = typedValue === confirmValue;
  return (
    <Dialog className="p-6" size="lg">
      <form
        onSubmit={(event) => {
          event.preventDefault();
          if (confirmed && !isPending) onConfirm({ force });
        }}
      >
        <Dialog.Title>{title}</Dialog.Title>
        <Dialog.Description>{description}</Dialog.Description>
        {forceOption ? (
          <Checkbox
            className="mt-4"
            checked={force}
            onCheckedChange={(checked) => setForce(checked === true)}
            label={
              <span>
                <span className="text-kumo-default font-medium">
                  {forceOption.label}
                </span>
                <span className="text-kumo-subtle mt-1 block">
                  {forceOption.description}
                </span>
              </span>
            }
          />
        ) : null}
        <p className="text-kumo-subtle mt-4 text-sm">
          Enter{" "}
          <code className="text-kumo-default [font-size:0.9em] break-all">
            {confirmValue}
          </code>{" "}
          to continue.
        </p>
        <div className="mt-4">
          <Input
            label={`Type ${resourceLabel} to confirm`}
            value={typedValue}
            onChange={(event) => setTypedValue(event.target.value)}
            placeholder={confirmValue}
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
            variant={submitVariant}
            type="submit"
            disabled={!confirmed || isPending}
          >
            {isPending ? "Working…" : submitLabel}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
