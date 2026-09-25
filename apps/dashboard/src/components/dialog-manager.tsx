import { Button, type ButtonProps } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { atom, createStore, useAtomValue } from "jotai";
import { Fragment, useState, type ReactNode } from "react";
import { useMutationFeedback } from "../features/toast/use-mutation-feedback";

type ButtonVariant = NonNullable<ButtonProps["variant"]>;

type DialogSize = "sm" | "base" | "lg" | "xl";

interface ManagedDialogState {
  id: number;
  open: boolean;
  /** Rendered as the dialog heading. Omit when `content` provides its own. */
  title?: ReactNode;
  description?: ReactNode;
  /** Optional body rendered between the header and the footer buttons. */
  content?: ReactNode;
  confirmText?: string;
  confirmVariant?: ButtonVariant;
  cancelText?: string;
  /** Sync results close immediately; rejections keep the dialog open. */
  onConfirm?: () => unknown | Promise<unknown>;
  onCancel?: () => unknown;
  size?: DialogSize;
  /** Classes for the dialog panel, for callers that restyle the default padding. */
  contentClassName?: string;
}

const dialogStore = createStore();
let nextDialogId = 0;
const dialogAtom = atom<ManagedDialogState>({ id: 0, open: false });

const alertDialogStore = createStore();
let nextAlertId = 0;
const alertDialogAtom = atom<ManagedDialogState>({ id: 0, open: false });

/**
 * Opens a general-purpose dialog. Pass `content` for custom bodies (forms with
 * their own footer) or `onConfirm`/`onCancel` for a standard footer. Only one
 * dialog is visible at a time; opening replaces the current one.
 */
export function openDialog(
  input: Omit<ManagedDialogState, "id" | "open">,
): void {
  nextDialogId += 1;
  dialogStore.set(dialogAtom, { id: nextDialogId, open: true, ...input });
}

/** Closes the dialog currently shown by `openDialog`, if any. */
export function closeDialog(): void {
  dialogStore.set(dialogAtom, (current) => ({ ...current, open: false }));
}

/**
 * Opens a confirmation alert rendered with `role="alertdialog"` (not
 * dismissible via outside click). Only one alert is visible at a time.
 */
export function openAlert(
  input: Omit<ManagedDialogState, "id" | "open">,
): void {
  nextAlertId += 1;
  alertDialogStore.set(alertDialogAtom, {
    id: nextAlertId,
    open: true,
    ...input,
  });
}

/** Closes the alert currently shown by `openAlert`, if any. */
export function closeAlert(): void {
  alertDialogStore.set(alertDialogAtom, (current) => ({
    ...current,
    open: false,
  }));
}

function closeDialogById(expectedId: number): void {
  dialogStore.set(dialogAtom, (current) =>
    current.id === expectedId ? { ...current, open: false } : current,
  );
}

function closeAlertById(expectedId: number): void {
  alertDialogStore.set(alertDialogAtom, (current) =>
    current.id === expectedId ? { ...current, open: false } : current,
  );
}

function DialogFooter({
  close,
  stateId,
  confirmText,
  confirmVariant,
  cancelText,
  onConfirm,
  onCancel,
}: {
  close: (expectedId: number) => void;
  stateId: number;
  confirmText?: string | undefined;
  confirmVariant?: ButtonVariant | undefined;
  cancelText?: string | undefined;
  onConfirm?: (() => unknown | Promise<unknown>) | undefined;
  onCancel?: (() => unknown) | undefined;
}) {
  const feedback = useMutationFeedback();
  const [pendingId, setPendingId] = useState<number>();
  const pending = pendingId === stateId;

  if (!onConfirm && !onCancel && !confirmText) return null;
  return (
    <div className="mt-6 flex justify-end gap-2">
      {onCancel ? (
        <Button
          variant="secondary"
          disabled={pending}
          onClick={() => {
            onCancel();
            close(stateId);
          }}
        >
          {cancelText ?? "Cancel"}
        </Button>
      ) : null}
      {onConfirm || confirmText ? (
        <Button
          variant={confirmVariant ?? "primary"}
          loading={pending}
          onClick={() => {
            const result = onConfirm?.();
            if (!(result instanceof Promise)) {
              close(stateId);
              return;
            }
            setPendingId(stateId);
            result
              .then(() => close(stateId))
              .catch((error: unknown) =>
                feedback.failure(error, "The request failed."),
              )
              .finally(() =>
                setPendingId((current) =>
                  current === stateId ? undefined : current,
                ),
              );
          }}
        >
          {confirmText ?? "Confirm"}
        </Button>
      ) : null}
    </div>
  );
}

function ManagedDialog({
  state,
  role,
  close,
}: {
  state: ManagedDialogState;
  role: "dialog" | "alertdialog";
  close: (expectedId: number) => void;
}) {
  return (
    <Dialog.Root
      open={state.open}
      role={role}
      onOpenChange={(nextOpen) => {
        if (!nextOpen) close(state.id);
      }}
    >
      <Dialog
        className={state.contentClassName ?? "px-6 py-5"}
        {...(state.size ? { size: state.size } : {})}
      >
        {state.title !== undefined ? (
          <Dialog.Title className="text-xl font-semibold">
            {state.title}
          </Dialog.Title>
        ) : null}
        {state.description ? (
          <Dialog.Description className="text-kumo-subtle mt-1.5 text-sm">
            {state.description}
          </Dialog.Description>
        ) : null}
        <Fragment key={state.id}>{state.content}</Fragment>
        <DialogFooter
          cancelText={state.cancelText}
          close={close}
          confirmText={state.confirmText}
          confirmVariant={state.confirmVariant}
          onCancel={state.onCancel}
          onConfirm={state.onConfirm}
          stateId={state.id}
        />
      </Dialog>
    </Dialog.Root>
  );
}

/** Mounted once at the application root; renders dialogs opened by `openDialog`. */
export function DialogHost() {
  const state = useAtomValue(dialogAtom, { store: dialogStore });
  return <ManagedDialog close={closeDialogById} role="dialog" state={state} />;
}

/** Mounted once at the application root; renders alerts opened by `openAlert`. */
export function AlertDialogHost() {
  const state = useAtomValue(alertDialogAtom, { store: alertDialogStore });
  return (
    <ManagedDialog close={closeAlertById} role="alertdialog" state={state} />
  );
}
