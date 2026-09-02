import { useEffect, useState } from "react";
import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Input } from "@cloudflare/kumo/components/input";

export type QueueConfigInput = {
  name?: string;
  deliveryDelaySeconds?: number;
  retentionSeconds?: number;
  maxBacklogBytes?: number;
};

interface QueueConfigDialogProps {
  open: boolean;
  mode: "create" | "edit";
  initial?: QueueConfigInput;
  errorMessage: string | null;
  isPending: boolean;
  onClose: () => void;
  onSubmit: (input: QueueConfigInput) => void;
}

function parseOptionalInteger(value: string) {
  if (!value.trim()) return undefined;
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) ? parsed : undefined;
}

export function QueueConfigDialog({
  open,
  mode,
  initial,
  errorMessage,
  isPending,
  onClose,
  onSubmit,
}: QueueConfigDialogProps) {
  const [name, setName] = useState("");
  const [deliveryDelay, setDeliveryDelay] = useState("");
  const [retention, setRetention] = useState("");
  const [maxBacklog, setMaxBacklog] = useState("");

  useEffect(() => {
    if (!open) return;
    setName(initial?.name ?? "");
    setDeliveryDelay(initial?.deliveryDelaySeconds?.toString() ?? "");
    setRetention(initial?.retentionSeconds?.toString() ?? "");
    setMaxBacklog(initial?.maxBacklogBytes?.toString() ?? "");
  }, [initial?.deliveryDelaySeconds, initial?.maxBacklogBytes, initial?.name, initial?.retentionSeconds, open]);

  const deliveryDelaySeconds = parseOptionalInteger(deliveryDelay);
  const retentionSeconds = parseOptionalInteger(retention);
  const maxBacklogBytes = parseOptionalInteger(maxBacklog);
  const numericValuesValid =
    (deliveryDelay === "" || (deliveryDelaySeconds !== undefined && deliveryDelaySeconds >= 0))
    && (retention === "" || (retentionSeconds !== undefined && retentionSeconds > 0))
    && (maxBacklog === "" || (maxBacklogBytes !== undefined && maxBacklogBytes > 0));
  const canSubmit = numericValuesValid && (mode === "edit" || Boolean(name.trim()));

  return (
    <Dialog.Root open={open} onOpenChange={nextOpen => {
      if (!nextOpen) onClose();
    }}>
      <Dialog className="p-6" size="lg">
        <form onSubmit={event => {
          event.preventDefault();
          if (!canSubmit || isPending) return;
          onSubmit({
            ...(mode === "create" ? { name: name.trim() } : {}),
            ...(deliveryDelaySeconds !== undefined ? { deliveryDelaySeconds } : {}),
            ...(retentionSeconds !== undefined ? { retentionSeconds } : {}),
            ...(maxBacklogBytes !== undefined ? { maxBacklogBytes } : {}),
          });
        }}>
          <Dialog.Title>{mode === "create" ? "Create queue" : "Edit queue configuration"}</Dialog.Title>
          <Dialog.Description>
            Configure delivery delay, retention, and the bounded backlog limit. Values are stored as seconds and bytes.
          </Dialog.Description>
          <div className="mt-4 grid gap-4 sm:grid-cols-2">
            {mode === "create" ? (
              <div className="sm:col-span-2">
                <Input label="Queue name" value={name} onChange={event => setName(event.target.value)} placeholder="my-queue" autoFocus />
              </div>
            ) : null}
            <Input label="Delivery delay (seconds)" type="number" min={0} step={1} value={deliveryDelay} onChange={event => setDeliveryDelay(event.target.value)} />
            <Input label="Retention (seconds)" type="number" min={1} step={1} value={retention} onChange={event => setRetention(event.target.value)} />
            <div className="sm:col-span-2">
              <Input label="Maximum backlog (bytes)" type="number" min={1} step={1} value={maxBacklog} onChange={event => setMaxBacklog(event.target.value)} />
            </div>
          </div>
          {!numericValuesValid ? <p className="mt-3 text-sm text-kumo-danger" role="alert">Enter whole-number limits in the allowed range.</p> : null}
          {errorMessage ? <p className="mt-3 text-sm text-kumo-danger" role="alert">{errorMessage}</p> : null}
          <div className="mt-6 flex justify-end gap-2">
            <Button variant="secondary" type="button" onClick={onClose} disabled={isPending}>Cancel</Button>
            <Button variant="primary" type="submit" disabled={!canSubmit || isPending}>
              {isPending ? "Saving…" : mode === "create" ? "Create queue" : "Save configuration"}
            </Button>
          </div>
        </form>
      </Dialog>
    </Dialog.Root>
  );
}
