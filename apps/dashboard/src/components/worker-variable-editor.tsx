import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { IconEdit, IconPlus, IconTrash } from "@tabler/icons-react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useAuth } from "../features/auth/auth-atoms";
import { useMutationFeedback } from "../features/toast/use-mutation-feedback";

type Binding = { name?: string; type: string; text?: string; json?: unknown };
type Secret = { name: string };
type Draft = {
  originalName: string | null;
  name: string;
  value: string;
  secret: boolean;
};

export function WorkerVariableEditor({
  workerId,
  bindings,
  secrets,
  onAddSecret,
  onDeleteSecret,
}: {
  workerId: string;
  bindings: readonly Binding[];
  secrets: readonly Secret[];
  onAddSecret: () => void;
  onDeleteSecret: (name: string) => void;
}) {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const [draft, setDraft] = useState<Draft | null>(null);
  const [initial, setInitial] = useState<Draft | null>(null);
  const [discardOpen, setDiscardOpen] = useState(false);
  const [deleteName, setDeleteName] = useState<string | null>(null);
  const names = [
    ...new Set(
      [...bindings, ...secrets]
        .map((item) => item.name)
        .filter((name): name is string => Boolean(name)),
    ),
  ];
  const duplicate =
    draft !== null &&
    names.some(
      (name) => name === draft.name.trim() && name !== draft.originalName,
    );
  const valid =
    draft !== null && Boolean(draft.name.trim() && draft.value) && !duplicate;
  const dirty =
    draft !== null && JSON.stringify(draft) !== JSON.stringify(initial);

  const save = useMutation({
    mutationFn: async (change: {
      name: string;
      value?: string;
      secret?: boolean;
      remove?: boolean;
    }) => {
      if (!client || !selectedInstanceId)
        throw new Error("Dashboard session is unavailable.");
      const inherited = names
        .filter((name) => name !== change.name)
        .map((name) => ({ type: "inherit" as const, name }));
      const changed = change.remove
        ? []
        : change.secret
          ? [
              {
                type: "secret_text" as const,
                name: change.name,
                text: change.value ?? "",
              },
            ]
          : [
              {
                type: "plain_text" as const,
                name: change.name,
                text: change.value ?? "",
              },
            ];
      await client.workers.scripts.scriptAndVersionSettings.edit(workerId, {
        account_id: selectedInstanceId,
        settings: {
          bindings: [...inherited, ...changed],
          annotations: {
            "workers/message": `${change.remove ? "Delete" : names.includes(change.name) ? "Edit" : "Add"} variable: ${change.name}`,
          },
        },
      });
    },
    onSuccess: async () => {
      setDraft(null);
      setDeleteName(null);
      await Promise.all([
        queryClient.invalidateQueries({
          queryKey: [
            "cloudflare-v4",
            "workers",
            selectedInstanceId,
            workerId,
            "version-settings",
          ],
        }),
        queryClient.invalidateQueries({
          queryKey: [
            "cloudflare-v4",
            "workers",
            selectedInstanceId,
            workerId,
            "secrets",
          ],
        }),
      ]);
      feedback.success("Worker version saved. It is not live yet.");
    },
    onError: (error) => feedback.failure(error, "Unable to save variable."),
  });

  const openAdd = () => {
    save.reset();
    const next = { originalName: null, name: "", value: "", secret: false };
    setInitial(next);
    setDraft(next);
  };
  const openEdit = (binding: Binding) => {
    save.reset();
    const next = {
      originalName: binding.name ?? null,
      name: binding.name ?? "",
      value: binding.text ?? "",
      secret: false,
    };
    setInitial(next);
    setDraft(next);
  };
  const close = () => {
    if (save.isPending) return;
    if (dirty) setDiscardOpen(true);
    else setDraft(null);
  };
  const variableRows = bindings.filter(
    (binding) => binding.type === "plain_text" || binding.type === "json",
  );

  return (
    <>
      <div className="flex justify-end gap-2">
        <Button variant="secondary" onClick={onAddSecret}>
          <IconPlus size={16} /> Add secret
        </Button>
        <Button variant="secondary" onClick={openAdd}>
          <IconPlus size={16} /> Add variable
        </Button>
      </div>
      <div className="bg-kumo-base ring-kumo-line min-w-0 overflow-hidden rounded-lg ring">
        <div className="border-kumo-line text-kumo-subtle grid grid-cols-4 gap-2 border-b px-4 py-2 text-xs font-medium sm:gap-3">
          <span>Type</span>
          <span>Name</span>
          <span>Value</span>
          <span>Actions</span>
        </div>
        {variableRows.map((binding) => (
          <div
            key={`${binding.type}-${binding.name}`}
            className="border-kumo-line grid grid-cols-4 items-center gap-2 border-b px-4 py-2 last:border-0 sm:gap-3"
          >
            <span>Variable</span>
            <code className="truncate text-xs">{binding.name}</code>
            <span className="text-kumo-subtle truncate">
              {binding.type === "plain_text"
                ? binding.text
                : JSON.stringify(binding.json)}
            </span>
            {binding.type === "plain_text" ? (
              <span className="flex justify-end gap-1">
                <Button
                  variant="ghost"
                  shape="square"
                  aria-label={`Edit ${binding.name}`}
                  onClick={() => openEdit(binding)}
                >
                  <IconEdit size={16} />
                </Button>
                <Button
                  variant="ghost"
                  shape="square"
                  aria-label={`Delete ${binding.name}`}
                  onClick={() => {
                    save.reset();
                    setDeleteName(binding.name ?? null);
                  }}
                >
                  <IconTrash size={16} />
                </Button>
              </span>
            ) : (
              <span />
            )}
          </div>
        ))}
        {secrets.map((secret) => (
          <div
            key={`secret-${secret.name}`}
            className="border-kumo-line grid grid-cols-4 items-center gap-2 border-b px-4 py-2 last:border-0 sm:gap-3"
          >
            <span>Secret</span>
            <code className="truncate text-xs">{secret.name}</code>
            <span className="text-kumo-subtle">Encrypted</span>
            <span className="flex justify-end">
              <Button
                variant="ghost"
                shape="square"
                aria-label={`Delete ${secret.name}`}
                onClick={() => onDeleteSecret(secret.name)}
              >
                <IconTrash size={16} />
              </Button>
            </span>
          </div>
        ))}
        {!variableRows.length && !secrets.length ? (
          <p className="text-kumo-subtle px-4 py-4">
            No variables or secrets configured.
          </p>
        ) : null}
      </div>
      <Dialog.Root
        open={draft !== null}
        onOpenChange={(open) => {
          if (!open) close();
        }}
      >
        <Dialog className="p-0" size="lg">
          <form
            onSubmit={(event) => {
              event.preventDefault();
              if (valid && draft)
                save.mutate({
                  name: draft.name.trim(),
                  value: draft.value,
                  secret: draft.secret,
                });
            }}
          >
            <Dialog.Title className="border-kumo-line border-b px-4 py-3">
              {draft?.originalName
                ? `Edit variable ${draft.originalName} in Production`
                : "Add a variable"}
            </Dialog.Title>
            <Dialog.Description className="sr-only">
              Save a Production variable in a new Worker version.
            </Dialog.Description>
            <div className="grid gap-3 px-4 py-4">
              <div className="ring-kumo-line overflow-hidden rounded-lg ring">
                <div className="grid h-9 grid-cols-4">
                  <label
                    htmlFor="worker-variable-name"
                    className="border-kumo-line flex items-center border-r px-3"
                  >
                    Key
                  </label>
                  <input
                    id="worker-variable-name"
                    aria-label="Variable name"
                    className="col-span-3 min-w-0 px-3 outline-none"
                    value={draft?.name ?? ""}
                    disabled={Boolean(draft?.originalName)}
                    onChange={(event) =>
                      setDraft(
                        (current) =>
                          current && { ...current, name: event.target.value },
                      )
                    }
                    required
                  />
                </div>
                <div className="border-kumo-line grid h-9 grid-cols-4 border-t">
                  <label
                    htmlFor="worker-variable-value"
                    className="border-kumo-line flex items-center border-r px-3"
                  >
                    Value
                  </label>
                  <div className="col-span-3 flex min-w-0 items-center gap-2 pr-3">
                    <input
                      id="worker-variable-value"
                      aria-label={draft?.secret ? "Secret value" : "Value"}
                      type={draft?.secret ? "password" : "text"}
                      className="min-w-0 flex-1 px-3 outline-none"
                      value={draft?.value ?? ""}
                      onChange={(event) =>
                        setDraft(
                          (current) =>
                            current && {
                              ...current,
                              value: event.target.value,
                            },
                        )
                      }
                      required
                    />
                    {draft?.originalName ? null : (
                      <label className="flex shrink-0 items-center gap-1 text-sm">
                        <span>Secret</span>
                        <input
                          type="checkbox"
                          checked={draft?.secret ?? false}
                          onChange={(event) =>
                            setDraft(
                              (current) =>
                                current && {
                                  ...current,
                                  secret: event.target.checked,
                                },
                            )
                          }
                        />
                      </label>
                    )}
                  </div>
                </div>
              </div>
              <p className="text-kumo-subtle text-sm">
                Only Production is available on this installation.
              </p>
              {draft?.secret ? (
                <p className="text-kumo-subtle text-sm">
                  Secret values cannot be read after saving.
                </p>
              ) : null}
              {duplicate ? (
                <p className="text-kumo-danger text-sm">
                  A binding with this name already exists.
                </p>
              ) : null}
              {save.isError ? (
                <p className="text-kumo-danger text-sm">
                  Unable to save variable. Your changes are still here.
                </p>
              ) : null}
            </div>
            <div className="border-kumo-line flex flex-col-reverse gap-2 border-t px-4 py-4 sm:flex-row sm:justify-between">
              <Button
                variant="secondary"
                type="button"
                disabled={save.isPending}
                onClick={close}
              >
                Cancel
              </Button>
              <Button
                variant="primary"
                type="submit"
                disabled={!valid || save.isPending}
              >
                {save.isPending ? "Saving…" : "Save version"}
              </Button>
            </div>
          </form>
        </Dialog>
      </Dialog.Root>
      <Dialog.Root
        open={discardOpen}
        role="alertdialog"
        onOpenChange={setDiscardOpen}
      >
        <Dialog className="px-6 py-5" size="lg">
          <Dialog.Title>Discard changes?</Dialog.Title>
          <Dialog.Description>
            Your unsaved variable changes will be lost.
          </Dialog.Description>
          <div className="mt-6 flex justify-end gap-2">
            <Button variant="secondary" onClick={() => setDiscardOpen(false)}>
              Keep editing
            </Button>
            <Button
              variant="destructive"
              onClick={() => {
                setDiscardOpen(false);
                setDraft(null);
              }}
            >
              Discard
            </Button>
          </div>
        </Dialog>
      </Dialog.Root>
      <Dialog.Root
        open={deleteName !== null}
        role="alertdialog"
        onOpenChange={(open) => {
          if (!open && !save.isPending) setDeleteName(null);
        }}
      >
        <Dialog className="px-6 py-5" size="lg">
          <Dialog.Title>Delete variable?</Dialog.Title>
          <Dialog.Description>
            Delete {deleteName} in a new, undeployed Worker version? Other
            bindings will be preserved.
          </Dialog.Description>
          {save.isError ? (
            <p className="text-kumo-danger mt-3 text-sm">
              Unable to delete variable. Try again.
            </p>
          ) : null}
          <div className="mt-6 flex justify-end gap-2">
            <Button
              variant="secondary"
              disabled={save.isPending}
              onClick={() => setDeleteName(null)}
            >
              Cancel
            </Button>
            <Button
              variant="destructive"
              disabled={save.isPending}
              onClick={() => {
                if (deleteName) save.mutate({ name: deleteName, remove: true });
              }}
            >
              {save.isPending ? "Deleting…" : "Delete in new version"}
            </Button>
          </div>
        </Dialog>
      </Dialog.Root>
    </>
  );
}
