import { useEffect, useState } from "react";
import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Input } from "@cloudflare/kumo/components/input";

interface WorkflowDefinitionDialogProps {
  open: boolean;
  mode: "create" | "edit";
  initial?: { name?: string; scriptName: string; className: string };
  errorMessage: string | null;
  isPending: boolean;
  onClose: () => void;
  onSubmit: (input: { name?: string; scriptName: string; className: string }) => void;
}

export function WorkflowDefinitionDialog({
  open,
  mode,
  initial,
  errorMessage,
  isPending,
  onClose,
  onSubmit,
}: WorkflowDefinitionDialogProps) {
  const [name, setName] = useState("");
  const [scriptName, setScriptName] = useState("");
  const [className, setClassName] = useState("");

  useEffect(() => {
    if (!open) return;
    setName(initial?.name ?? "");
    setScriptName(initial?.scriptName ?? "");
    setClassName(initial?.className ?? "");
  }, [initial?.className, initial?.name, initial?.scriptName, open]);

  const canSubmit = Boolean((mode === "edit" || name.trim()) && scriptName.trim() && className.trim());

  return (
    <Dialog.Root open={open} onOpenChange={nextOpen => {
      if (!nextOpen) onClose();
    }}>
      <Dialog className="p-6" size="lg">
        <form onSubmit={event => {
          event.preventDefault();
          if (canSubmit && !isPending) onSubmit({
            ...(mode === "create" ? { name: name.trim() } : {}),
            scriptName: scriptName.trim(),
            className: className.trim(),
          });
        }}>
          <Dialog.Title>{mode === "create" ? "Create workflow definition" : "Update workflow definition"}</Dialog.Title>
          <Dialog.Description>
            Create a new workflow version from an existing Worker script and exported class through the Cloudflare v4 API.
          </Dialog.Description>
          <div className="mt-4 space-y-4">
            {mode === "create" ? <Input label="Workflow name" value={name} onChange={event => setName(event.target.value)} autoFocus /> : null}
            <Input label="Worker script name" value={scriptName} onChange={event => setScriptName(event.target.value)} autoFocus={mode === "edit"} />
            <Input label="Exported class name" value={className} onChange={event => setClassName(event.target.value)} placeholder="MyWorkflow" />
          </div>
          {errorMessage ? <p className="mt-3 text-sm text-kumo-danger" role="alert">{errorMessage}</p> : null}
          <div className="mt-6 flex justify-end gap-2">
            <Button variant="secondary" type="button" onClick={onClose} disabled={isPending}>Cancel</Button>
            <Button variant="primary" type="submit" disabled={!canSubmit || isPending}>{isPending ? "Saving…" : mode === "create" ? "Create workflow" : "Update workflow"}</Button>
          </div>
        </form>
      </Dialog>
    </Dialog.Root>
  );
}
