import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Input } from "@cloudflare/kumo/components/input";
import { useState } from "react";

export type R2DeleteTarget = { key: string; folder: boolean };

export function R2ObjectDeleteDialog({
  open,
  bucketName,
  targets,
  pending,
  error,
  onOpenChange,
  onConfirm,
}: {
  open: boolean;
  bucketName: string;
  targets: R2DeleteTarget[];
  pending: boolean;
  error: unknown;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}) {
  return (
    <Dialog.Root open={open} role="alertdialog" onOpenChange={onOpenChange}>
      <Dialog className="px-6 py-5" size="lg">
        <DeleteForm
          key={`${open}:${targets.map(({ key }) => key).join("\n")}`}
          bucketName={bucketName}
          targets={targets}
          pending={pending}
          error={error}
          onCancel={() => onOpenChange(false)}
          onConfirm={onConfirm}
        />
      </Dialog>
    </Dialog.Root>
  );
}

function DeleteForm({
  bucketName,
  targets,
  pending,
  error,
  onCancel,
  onConfirm,
}: {
  bucketName: string;
  targets: R2DeleteTarget[];
  pending: boolean;
  error: unknown;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const [confirmation, setConfirmation] = useState("");
  const files = targets.filter((target) => !target.folder).length;
  const folders = targets.length - files;
  const singleFile = targets.length === 1 && files === 1;
  const counts = [
    files ? `${files} ${files === 1 ? "file" : "files"}` : "",
    folders ? `${folders} ${folders === 1 ? "folder" : "folders"}` : "",
  ]
    .filter(Boolean)
    .join(" and ");
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        if (!pending && (singleFile || confirmation === "delete")) onConfirm();
      }}
    >
      <Dialog.Title>
        {singleFile ? "Delete this file?" : `Delete ${counts}?`}
      </Dialog.Title>
      <Dialog.Description>
        {singleFile
          ? `Deleting ${targets[0]?.key} from ${bucketName} is permanent and cannot be undone.`
          : `Deleting ${counts}${folders ? " and every object inside those folders" : ""} is permanent and cannot be undone.`}
      </Dialog.Description>
      {!singleFile ? (
        <div className="mt-5">
          <Input
            label="Type delete to confirm"
            placeholder="delete"
            value={confirmation}
            onChange={(event) => setConfirmation(event.target.value)}
            autoFocus
          />
        </div>
      ) : null}
      {error ? (
        <p className="text-kumo-danger mt-3 text-sm">
          {error instanceof Error
            ? error.message
            : "Unable to delete the objects."}
        </p>
      ) : null}
      <div className="mt-6 flex justify-end gap-2">
        <Button
          type="button"
          variant="secondary"
          onClick={onCancel}
          disabled={pending}
        >
          Cancel
        </Button>
        <Button
          type="submit"
          variant="destructive"
          disabled={
            pending ||
            !targets.length ||
            (!singleFile && confirmation !== "delete")
          }
        >
          {pending ? "Deleting…" : "Delete"}
        </Button>
      </div>
    </form>
  );
}
