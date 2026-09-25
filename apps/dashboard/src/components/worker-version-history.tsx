import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { IconArrowDown, IconCopy } from "@tabler/icons-react";
import { formatDistanceToNow } from "date-fns";
import { useState } from "react";
import { RowActionsMenu } from "./row-actions-menu";

export type WorkerVersionRow = {
  id: string;
  createdOn: string;
  message?: string;
};

function shortId(id: string): string {
  return `${id.slice(0, 8)}…${id.slice(-6)}`;
}

export function WorkerVersionHistory({
  versions,
  activeVersionId,
  pending,
  onPromote,
}: {
  versions: readonly WorkerVersionRow[];
  activeVersionId?: string | null;
  pending: boolean;
  onPromote: (id: string, message: string) => Promise<void>;
}) {
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [message, setMessage] = useState("Promote version to 100%");
  const [error, setError] = useState<string | null>(null);
  const [copyStatus, setCopyStatus] = useState<string | null>(null);
  const selected = versions.find((version) => version.id === selectedId);
  const active = versions.find((version) => version.id === activeVersionId);

  function close() {
    if (!pending) {
      setSelectedId(null);
      setError(null);
    }
  }

  return (
    <section className="grid gap-5">
      <div className="grid gap-2">
        <h2 className="text-base font-semibold">Version history</h2>
        <p className="text-kumo-subtle">
          Versions track changes to your Worker's code and configuration.
          Promote a saved version to make it live.
        </p>
      </div>
      <div className="ring-kumo-line bg-kumo-base divide-kumo-line divide-y overflow-hidden rounded-lg ring">
        {versions.length ? (
          versions.map((version) => {
            const isActive = version.id === activeVersionId;
            const created = new Date(version.createdOn);
            return (
              <div
                key={version.id}
                className="relative flex min-h-14 flex-wrap items-center gap-2 px-4 py-2.5"
              >
                {isActive ? (
                  <span className="bg-kumo-brand absolute inset-y-3 left-1.5 w-1 rounded-full" />
                ) : null}
                <code className="text-sm" title={version.id}>
                  {shortId(version.id)}
                </code>
                <Button
                  variant="ghost"
                  shape="square"
                  aria-label={`Copy version ID ${version.id}`}
                  title="Copy version ID"
                  onClick={() => {
                    void navigator.clipboard.writeText(version.id).then(
                      () => setCopyStatus("Version ID copied"),
                      () => setCopyStatus("Unable to copy version ID"),
                    );
                  }}
                >
                  <IconCopy size={14} />
                </Button>
                <span className="min-w-0 flex-1 truncate">
                  {version.message ||
                    (isActive ? "Deployed version" : "Saved version")}
                </span>
                <span className="text-kumo-subtle text-sm">
                  {isActive ? "Active" : "Saved"}
                </span>
                <time
                  className="text-kumo-subtle text-sm"
                  dateTime={version.createdOn}
                  title={
                    Number.isNaN(created.getTime())
                      ? undefined
                      : created.toLocaleString()
                  }
                >
                  {Number.isNaN(created.getTime())
                    ? version.createdOn
                    : formatDistanceToNow(created, { addSuffix: true })}
                </time>
                {!isActive ? (
                  <RowActionsMenu
                    label={`version ${version.id}`}
                    actions={[
                      {
                        id: "promote",
                        label: "Promote version",
                        onSelect: () => {
                          setSelectedId(version.id);
                          setMessage("Promote version to 100%");
                          setError(null);
                        },
                      },
                    ]}
                  />
                ) : null}
              </div>
            );
          })
        ) : (
          <p className="text-kumo-subtle px-4 py-5">No versions found.</p>
        )}
      </div>
      <span className="sr-only" aria-live="polite">
        {copyStatus}
      </span>
      <Dialog.Root
        open={selectedId !== null}
        onOpenChange={(open) => !open && close()}
      >
        <Dialog className="px-6 py-5" size="xl">
          <form
            onSubmit={async (event) => {
              event.preventDefault();
              if (!selected || pending) return;
              setError(null);
              try {
                await onPromote(selected.id, message.trim());
                setSelectedId(null);
              } catch (cause) {
                setError(
                  cause instanceof Error
                    ? cause.message
                    : "Unable to promote the version.",
                );
              }
            }}
          >
            <Dialog.Title className="text-2xl font-semibold">
              Promote version
            </Dialog.Title>
            <Dialog.Description className="sr-only">
              Deploy the selected Worker version to 100% of traffic.
            </Dialog.Description>
            <div className="mt-5 grid gap-3">
              <div className="ring-kumo-line overflow-hidden rounded-lg ring">
                <p className="text-kumo-subtle bg-kumo-tint border-kumo-line border-b px-4 py-2">
                  Current deployed version
                </p>
                <p className="min-w-0 px-4 py-3">
                  <code className="text-sm">
                    {active ? shortId(active.id) : "None"}
                  </code>
                  {active?.message ? (
                    <span className="ml-4">{active.message}</span>
                  ) : null}
                </p>
              </div>
              <IconArrowDown className="text-kumo-subtle mx-auto" size={20} />
              <div className="ring-kumo-line overflow-hidden rounded-lg ring">
                <p className="text-kumo-subtle bg-kumo-tint border-kumo-line border-b px-4 py-2">
                  Promoting to 100%
                </p>
                <p className="min-w-0 px-4 py-3">
                  <code className="text-sm">
                    {selected ? shortId(selected.id) : ""}
                  </code>
                  {selected?.message ? (
                    <span className="ml-4">{selected.message}</span>
                  ) : null}
                </p>
              </div>
              <div className="mt-2 grid gap-1.5">
                <label
                  htmlFor="worker-promotion-message"
                  className="font-medium"
                >
                  Message (optional)
                </label>
                <textarea
                  id="worker-promotion-message"
                  className="ring-kumo-line bg-kumo-base focus:ring-kumo-brand min-h-14 w-full rounded-lg px-3 py-2 ring outline-none focus:ring-2"
                  maxLength={50}
                  value={message}
                  onChange={(event) => setMessage(event.target.value)}
                />
                <span className="text-kumo-subtle text-sm">
                  {50 - message.length} characters remaining
                </span>
              </div>
              {error ? (
                <p role="alert" className="text-kumo-danger">
                  {error}
                </p>
              ) : null}
            </div>
            <div className="mt-6 flex justify-end gap-2">
              <Button
                type="button"
                variant="secondary"
                disabled={pending}
                onClick={close}
              >
                Cancel
              </Button>
              <Button
                type="submit"
                variant="primary"
                disabled={pending || !selected}
              >
                {pending ? "Promoting…" : "Promote version"}
              </Button>
            </div>
          </form>
        </Dialog>
      </Dialog.Root>
    </section>
  );
}
