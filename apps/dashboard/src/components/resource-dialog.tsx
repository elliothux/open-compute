import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import { useState } from "react";
import {
  closeAlert,
  closeDialog,
  openAlert,
  openDialog,
} from "./dialog-manager";

export function openResourceNameDialog(input: {
  title: string;
  description: string;
  label?: string;
  placeholder?: string;
  submitLabel?: string;
  initialValue?: string;
  /** Awaited before closing; a rejection keeps the dialog open. */
  submit: (value: string) => Promise<void>;
}): void {
  openDialog({
    title: input.title,
    description: input.description,
    size: "lg",
    contentClassName: "px-6 py-5",
    content: (
      <NameForm
        initialValue={input.initialValue ?? ""}
        label={input.label ?? "Name"}
        placeholder={input.placeholder ?? "resource-name"}
        submit={input.submit}
        submitLabel={input.submitLabel ?? "Create"}
      />
    ),
  });
}

function NameForm({
  label,
  placeholder,
  submitLabel,
  initialValue,
  submit,
}: {
  label: string;
  placeholder: string;
  submitLabel: string;
  initialValue: string;
  submit: (value: string) => Promise<void>;
}) {
  const [value, setValue] = useState(initialValue);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const trimmed = value.trim();

  async function handleSubmit() {
    if (!trimmed || pending) return;
    setPending(true);
    try {
      await submit(trimmed);
      closeDialog();
    } catch (caught) {
      setError(caught);
    } finally {
      setPending(false);
    }
  }

  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        void handleSubmit();
      }}
    >
      <div className="mt-5">
        <Input
          label={label}
          placeholder={placeholder}
          value={value}
          onChange={(event) => setValue(event.target.value)}
          autoFocus
        />
      </div>
      {error ? (
        <p className="text-kumo-danger mt-3 text-sm">
          {error instanceof Error ? error.message : "The request failed."}
        </p>
      ) : null}
      <div className="mt-6 flex justify-end gap-2">
        <Button
          type="button"
          variant="secondary"
          onClick={() => closeDialog()}
          disabled={pending}
        >
          Cancel
        </Button>
        <Button type="submit" variant="primary" disabled={!trimmed || pending}>
          {pending ? "Working…" : submitLabel}
        </Button>
      </div>
    </form>
  );
}

export function openConfirmDeleteDialog(input: {
  name: string;
  kind?: "resource" | "r2-bucket";
  /** Awaited before closing; a rejection keeps the dialog open. */
  confirm: () => Promise<void>;
}): void {
  const bucket = (input.kind ?? "resource") === "r2-bucket";
  openAlert({
    title: bucket ? "Delete bucket?" : `Delete ${input.name}`,
    description: bucket ? (
      <>
        <span className="mt-6 block">
          Deleting <strong>{input.name}</strong> is permanent and cannot be
          undone. You will no longer be able to add or modify data in this
          bucket.
        </span>
        <span className="mt-6 block">
          Workers in your account that use this bucket may be affected.
        </span>
      </>
    ) : (
      "This action cannot be undone. Enter the resource name to confirm."
    ),
    size: bucket ? "xl" : "lg",
    contentClassName: bucket ? "r2-delete-dialog p-0" : "px-6 py-5",
    content: (
      <DeleteForm bucket={bucket} confirm={input.confirm} name={input.name} />
    ),
  });
}

function DeleteForm({
  name,
  bucket,
  confirm,
}: {
  name: string;
  bucket: boolean;
  confirm: () => Promise<void>;
}) {
  const [value, setValue] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);

  async function handleSubmit() {
    if (value !== name || pending) return;
    setPending(true);
    try {
      await confirm();
      closeAlert();
    } catch (caught) {
      setError(caught);
    } finally {
      setPending(false);
    }
  }

  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        void handleSubmit();
      }}
    >
      <div className={bucket ? "px-8 pt-5 pb-8" : ""}>
        <div className="mt-5">
          <Input
            label={bucket ? `Type "${name}" to confirm` : "Resource name"}
            placeholder={name}
            value={value}
            onChange={(event) => setValue(event.target.value)}
            autoFocus
          />
        </div>
        {error ? (
          <p className="text-kumo-danger mt-3 text-sm">
            {error instanceof Error ? error.message : "The request failed."}
          </p>
        ) : null}
      </div>
      <div
        className={
          bucket
            ? "bg-kumo-recessed flex justify-end gap-2 px-8 py-4"
            : "mt-6 flex justify-end gap-2"
        }
      >
        <Button
          type="button"
          variant="secondary"
          onClick={() => closeAlert()}
          disabled={pending}
        >
          Cancel
        </Button>
        <Button
          type="submit"
          variant="destructive"
          disabled={value !== name || pending}
        >
          {pending ? "Deleting…" : "Delete"}
        </Button>
      </div>
    </form>
  );
}
