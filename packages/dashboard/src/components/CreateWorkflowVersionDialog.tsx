import { useEffect, useState } from "react";
import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Input } from "@cloudflare/kumo/components/input";

interface CreateWorkflowVersionDialogProps {
  open: boolean;
  errorMessage: string | null;
  isPending: boolean;
  onClose: () => void;
  onSubmit: (input: { deploymentId: string; className: string }) => void;
}

export function CreateWorkflowVersionDialog({
  open,
  errorMessage,
  isPending,
  onClose,
  onSubmit,
}: CreateWorkflowVersionDialogProps) {
  const [deploymentId, setDeploymentId] = useState("");
  const [className, setClassName] = useState("");

  useEffect(() => {
    if (!open) {
      setDeploymentId("");
      setClassName("");
    }
  }, [open]);

  const canSubmit = Boolean(deploymentId.trim() && className.trim());

  return (
    <Dialog.Root open={open} onOpenChange={nextOpen => {
      if (!nextOpen) onClose();
    }}>
      <Dialog className="p-6" size="lg">
        <form onSubmit={event => {
          event.preventDefault();
          if (canSubmit && !isPending) onSubmit({ deploymentId: deploymentId.trim(), className: className.trim() });
        }}>
          <Dialog.Title>Create workflow version</Dialog.Title>
          <Dialog.Description>
            Pin this definition to a validated Worker deployment and its exported workflow class.
          </Dialog.Description>
          <div className="mt-4 space-y-4">
            <Input label="Deployment ID" value={deploymentId} onChange={event => setDeploymentId(event.target.value)} autoFocus />
            <Input label="Exported class name" value={className} onChange={event => setClassName(event.target.value)} placeholder="MyWorkflow" />
          </div>
          {errorMessage ? <p className="mt-3 text-sm text-kumo-danger" role="alert">{errorMessage}</p> : null}
          <div className="mt-6 flex justify-end gap-2">
            <Button variant="secondary" type="button" onClick={onClose} disabled={isPending}>Cancel</Button>
            <Button variant="primary" type="submit" disabled={!canSubmit || isPending}>{isPending ? "Creating…" : "Create version"}</Button>
          </div>
        </form>
      </Dialog>
    </Dialog.Root>
  );
}
