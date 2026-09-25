import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Drawer } from "@cloudflare/kumo/primitives/drawer";
import { IconX } from "@tabler/icons-react";
import type { FormEvent, ReactNode } from "react";

export function WorkerBindingDialogLayout({
  title,
  description,
  fields,
  preview,
  pending,
  valid,
  cancelLabel,
  submitLabel,
  onCancel,
}: {
  title: string;
  description: string;
  fields: ReactNode;
  preview: ReactNode;
  pending: boolean;
  valid: boolean;
  cancelLabel: string;
  submitLabel: string;
  onCancel: () => void;
}) {
  return (
    <>
      <Dialog.Title className="border-kumo-line border-b px-5 py-4 text-xl font-medium">
        {title}
      </Dialog.Title>
      <Dialog.Description className="sr-only">{description}</Dialog.Description>
      <div className="grid sm:grid-cols-2">
        <div className="border-kumo-line grid min-w-0 content-start gap-5 px-5 py-4 sm:border-r">
          {fields}
        </div>
        {preview}
      </div>
      <div className="border-kumo-line flex justify-between border-t px-5 py-4">
        <Button
          variant="secondary"
          type="button"
          disabled={pending}
          onClick={onCancel}
        >
          {cancelLabel}
        </Button>
        <Button variant="primary" type="submit" disabled={!valid || pending}>
          {pending ? "Deploying…" : submitLabel}
        </Button>
      </div>
    </>
  );
}

export function WorkerBindingDrawerLayout({
  open,
  pending,
  valid,
  title,
  description,
  docsHref,
  submitLabel = "Deploy",
  onClose,
  onSubmit,
  children,
}: {
  open: boolean;
  pending: boolean;
  valid: boolean;
  title: string;
  description: string;
  docsHref?: string;
  submitLabel?: string;
  onClose: () => void;
  onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  children: ReactNode;
}) {
  return (
    <Drawer.Root
      open={open}
      modal={false}
      swipeDirection="right"
      onOpenChange={(nextOpen) => {
        if (!nextOpen && !pending) onClose();
      }}
    >
      <Drawer.Portal>
        <Drawer.Viewport className="pointer-events-none fixed inset-0 z-50 flex justify-end">
          <Drawer.Popup className="bg-kumo-base shadow-kumo-lg ring-kumo-line pointer-events-auto flex h-full w-full max-w-md flex-col ring">
            <form className="flex h-full flex-col" onSubmit={onSubmit}>
              <div className="border-kumo-line border-b px-5 py-4">
                <div className="flex items-start justify-between gap-2">
                  <div className="grid gap-1.5">
                    <Drawer.Title className="text-xl font-medium">
                      {title}
                    </Drawer.Title>
                    <Drawer.Description className="text-kumo-subtle text-sm">
                      {description}
                    </Drawer.Description>
                  </div>
                  <Button
                    variant="ghost"
                    shape="square"
                    aria-label="Close"
                    disabled={pending}
                    onClick={onClose}
                    type="button"
                  >
                    <IconX size={16} />
                  </Button>
                </div>
                {docsHref ? (
                  <a
                    className="text-kumo-brand mt-2 inline-block text-sm hover:underline"
                    href={docsHref}
                    target="_blank"
                    rel="noreferrer"
                  >
                    Documentation ↗
                  </a>
                ) : null}
              </div>
              <div className="grid content-start gap-5 overflow-y-auto px-5 py-4">
                {children}
              </div>
              <div className="border-kumo-line mt-auto flex justify-end gap-2 border-t px-5 py-4">
                <Button
                  variant="secondary"
                  type="button"
                  disabled={pending}
                  onClick={onClose}
                >
                  Cancel
                </Button>
                <Button
                  variant="primary"
                  type="submit"
                  disabled={!valid || pending}
                >
                  {pending ? "Deploying…" : submitLabel}
                </Button>
              </div>
            </form>
          </Drawer.Popup>
        </Drawer.Viewport>
      </Drawer.Portal>
    </Drawer.Root>
  );
}
