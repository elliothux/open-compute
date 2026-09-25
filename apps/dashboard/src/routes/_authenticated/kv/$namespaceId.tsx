import { Button } from "@cloudflare/kumo/components/button";
import { Input, Textarea } from "@cloudflare/kumo/components/input";
import { LayerCard } from "@cloudflare/kumo/components/layer-card";
import { Tabs } from "@cloudflare/kumo/components/tabs";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useSetAtom } from "jotai";
import { useEffect, useState } from "react";
import { BackupTable } from "../../../components/backup-table";
import { CodeBlock } from "../../../components/code-block";
import {
  EmptyState,
  ErrorState,
  LoadingRows,
  PageHeader,
  Panel,
  Section,
} from "../../../components/dashboard-page";
import {
  openConfirmDeleteDialog,
  openResourceNameDialog,
} from "../../../components/resource-dialog";
import { useAuth } from "../../../features/auth/auth-atoms";
import { detailBreadcrumbAtom } from "../../../features/navigation/detail-breadcrumb-atom";
import { useMutationFeedback } from "../../../features/toast/use-mutation-feedback";
import { kvNamespaceQuery } from "../../../lib/query-options";
import { KvPairs } from "./-pairs";

export const Route = createFileRoute("/_authenticated/kv/$namespaceId")({
  validateSearch: (search: Record<string, unknown>): { tab?: Tab } =>
    search.tab === "settings" ||
    search.tab === "bulk" ||
    search.tab === "backups"
      ? { tab: search.tab }
      : {},
  loader: ({ context, params }) => {
    const { client, instanceId } = context.auth;
    if (!client || !instanceId) return;
    return context.queryClient.ensureQueryData(
      kvNamespaceQuery(client, instanceId, params.namespaceId),
    );
  },
  component: KvDetailPage,
});

type Tab = "pairs" | "settings" | "bulk" | "backups";
type BulkValue = {
  key: string;
  value: string;
  expiration_ttl?: number;
  metadata?: unknown;
};

function parseBulkValues(source: string): BulkValue[] {
  const value: unknown = JSON.parse(source);
  if (!Array.isArray(value))
    throw new Error("Bulk values must be a JSON array.");
  return value.map((item, index) => {
    if (typeof item !== "object" || item === null)
      throw new Error(`Item ${index + 1} must be an object.`);
    const row = item as Record<string, unknown>;
    if (typeof row.key !== "string" || typeof row.value !== "string")
      throw new Error(`Item ${index + 1} needs string key and value fields.`);
    if (
      row.expiration_ttl !== undefined &&
      (typeof row.expiration_ttl !== "number" ||
        !Number.isSafeInteger(row.expiration_ttl) ||
        row.expiration_ttl < 60)
    )
      throw new Error(`Item ${index + 1} has an invalid expiration_ttl.`);
    return {
      key: row.key,
      value: row.value,
      ...(row.expiration_ttl === undefined
        ? {}
        : { expiration_ttl: row.expiration_ttl }),
      ...(row.metadata === undefined ? {} : { metadata: row.metadata }),
    };
  });
}

function KvDetailPage() {
  const { namespaceId } = Route.useParams();
  const { tab: searchTab } = Route.useSearch();
  const tab = searchTab ?? "pairs";
  const { client, instanceId: selectedInstanceId } = useAuth();
  const feedback = useMutationFeedback();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const setDetailBreadcrumb = useSetAtom(detailBreadcrumbAtom);
  const enabled = client !== null && selectedInstanceId !== null;
  const setTab = (next: Tab) =>
    void navigate({
      to: "/kv/$namespaceId",
      params: { namespaceId },
      search: next === "pairs" ? {} : { tab: next },
    });
  const [renaming, setRenaming] = useState(false);
  const [newName, setNewName] = useState("");
  const [bulkKeys, setBulkKeys] = useState("");
  const [bulkValues, setBulkValues] = useState(
    '[\n  { "key": "example", "value": "value" }\n]',
  );
  const [bulkResult, setBulkResult] = useState<unknown>(null);

  const namespace = useQuery(
    kvNamespaceQuery(client, selectedInstanceId, namespaceId),
  );
  useEffect(() => {
    if (!namespace.data?.title) return;
    setDetailBreadcrumb({
      path: `/kv/${namespaceId}`,
      name: namespace.data.title,
    });
    return () => setDetailBreadcrumb(null);
  }, [namespace.data?.title, namespaceId, setDetailBreadcrumb]);
  const backups = useQuery({
    queryKey: [
      "cloudflare-v4",
      "kv",
      selectedInstanceId,
      namespaceId,
      "backups",
    ],
    queryFn: ({ signal }) =>
      client!.openCompute.backups.kv.list(selectedInstanceId!, namespaceId, {
        signal,
      }),
    enabled,
  });
  const bulkGet = useMutation({
    mutationFn: () =>
      client!.kv.namespaces.bulkGet(namespaceId, {
        account_id: selectedInstanceId!,
        keys: lines(bulkKeys),
        type: "text",
        withMetadata: true,
      }),
    onSuccess: (result) => setBulkResult(result),
    onError: (error) => feedback.failure(error, "Unable to read the KV pairs."),
  });
  const bulkPut = useMutation({
    mutationFn: () =>
      client!.kv.namespaces.bulkUpdate(namespaceId, {
        account_id: selectedInstanceId!,
        body: parseBulkValues(bulkValues),
      }),
    onSuccess: async (result) => {
      setBulkResult(result);
      await queryClient.invalidateQueries({
        queryKey: [
          "cloudflare-v4",
          "kv",
          selectedInstanceId,
          namespaceId,
          "keys",
        ],
      });
      feedback.success("Bulk values saved.");
    },
    onError: (error) =>
      feedback.failure(error, "Unable to save the bulk values."),
  });
  const bulkRemove = useMutation({
    mutationFn: () =>
      client!.kv.namespaces.bulkDelete(namespaceId, {
        account_id: selectedInstanceId!,
        body: lines(bulkKeys),
      }),
    onSuccess: async (result) => {
      setBulkResult(result);
      await queryClient.invalidateQueries({
        queryKey: [
          "cloudflare-v4",
          "kv",
          selectedInstanceId,
          namespaceId,
          "keys",
        ],
      });
      feedback.success("Bulk keys deleted.");
    },
    onError: (error) =>
      feedback.failure(error, "Unable to delete the bulk keys."),
  });
  const createBackup = useMutation({
    mutationFn: () =>
      client!.openCompute.backups.kv.create(selectedInstanceId!, namespaceId),
    onSuccess: async () => {
      await backups.refetch();
      feedback.success("KV backup created.");
    },
    onError: (error) => feedback.failure(error, "Unable to create the backup."),
  });
  function openRestoreBackupDialog(backupId: string) {
    openResourceNameDialog({
      title: "Restore KV backup",
      description: "The backup will be restored into a new namespace.",
      label: "New namespace name",
      placeholder: "restored-namespace",
      submitLabel: "Restore",
      submit: async (name) => {
        try {
          await client!.openCompute.backups.kv.restore(
            selectedInstanceId!,
            backupId,
            { name },
          );
        } catch (error) {
          feedback.failure(error, "Unable to restore the backup.");
          throw error;
        }
        feedback.success("Backup restored into a new namespace.");
      },
    });
  }

  const rename = useMutation({
    mutationFn: () =>
      client!.kv.namespaces.update(namespaceId, {
        account_id: selectedInstanceId!,
        title: newName.trim(),
      }),
    onSuccess: async () => {
      setRenaming(false);
      await queryClient.invalidateQueries({
        queryKey: ["cloudflare-v4", "kv", selectedInstanceId, namespaceId],
      });
      feedback.success("KV namespace renamed.");
    },
    onError: (error) =>
      feedback.failure(error, "Unable to rename the namespace."),
  });
  function confirmDeleteNamespace() {
    openConfirmDeleteDialog({
      name: namespace.data?.title ?? namespaceId,
      confirm: async () => {
        try {
          await client!.kv.namespaces.delete(namespaceId, {
            account_id: selectedInstanceId!,
          });
        } catch (error) {
          feedback.failure(error, "Unable to delete the namespace.");
          throw error;
        }
        await queryClient.invalidateQueries({
          queryKey: ["cloudflare-v4", "kv", selectedInstanceId, "namespaces"],
        });
        feedback.success("KV namespace deleted.");
        await navigate({ to: "/kv" });
      },
    });
  }

  const tabs: readonly [Tab, string][] = [
    ["pairs", "KV pairs"],
    ["settings", "Settings"],
    ["bulk", "Bulk operations"],
    ["backups", "Backups"],
  ];
  return (
    <div>
      <PageHeader
        title={namespace.data?.title ?? "KV namespace"}
        description="Read, write, and manage key-value data."
        extension
      />
      <nav className="mb-6" aria-label="KV namespace tabs">
        <Tabs
          variant="underline"
          value={tab}
          tabs={tabs.map(([value, label]) => ({ value, label }))}
          onValueChange={(value) => setTab(value as Tab)}
        />
      </nav>
      {namespace.error ? (
        <ErrorState error={namespace.error} />
      ) : tab === "pairs" ? (
        <KvPairs namespaceId={namespaceId} />
      ) : tab === "bulk" ? (
        <div className="grid gap-6 md:grid-cols-2">
          <Section
            title="Read or delete keys"
            description="Enter one key per line. Bulk reads accept up to 100 keys."
          >
            <Panel>
              <Textarea
                aria-label="Bulk keys"
                className="min-h-52 w-full font-mono text-xs"
                value={bulkKeys}
                onChange={(event) => setBulkKeys(event.target.value)}
              />
              <div className="mt-3 flex flex-wrap gap-2">
                <Button
                  variant="primary"
                  disabled={lines(bulkKeys).length === 0 || bulkGet.isPending}
                  onClick={() => bulkGet.mutate()}
                >
                  Get values
                </Button>
                <Button
                  variant="destructive"
                  disabled={
                    lines(bulkKeys).length === 0 || bulkRemove.isPending
                  }
                  onClick={() => bulkRemove.mutate()}
                >
                  Delete keys
                </Button>
              </div>
            </Panel>
          </Section>
          <Section
            title="Write values"
            description="Enter a JSON array with key and value fields."
          >
            <Panel>
              <Textarea
                aria-label="Bulk values JSON"
                className="min-h-52 w-full font-mono text-xs"
                value={bulkValues}
                onChange={(event) => setBulkValues(event.target.value)}
              />
              <div className="mt-3">
                <Button
                  variant="primary"
                  disabled={!bulkValues.trim() || bulkPut.isPending}
                  onClick={() => bulkPut.mutate()}
                >
                  Write values
                </Button>
              </div>
            </Panel>
          </Section>
          {bulkGet.error || bulkPut.error || bulkRemove.error ? (
            <div className="md:col-span-2">
              <ErrorState
                error={bulkGet.error ?? bulkPut.error ?? bulkRemove.error}
              />
            </div>
          ) : null}
          {bulkResult !== null ? (
            <div className="md:col-span-2">
              <Section title="Result">
                <CodeBlock
                  className="bg-kumo-tint ring-kumo-line max-h-80 overflow-auto rounded-lg p-4 font-mono text-xs ring"
                  code={JSON.stringify(bulkResult, null, 2)}
                  language="json"
                />
              </Section>
            </div>
          ) : null}
        </div>
      ) : tab === "backups" ? (
        <Section
          title="Backups"
          description="Create a point-in-time copy and restore it into a new namespace."
        >
          <div>
            <Button
              variant="primary"
              disabled={createBackup.isPending || !enabled}
              onClick={() => createBackup.mutate()}
            >
              {createBackup.isPending ? "Creating…" : "Create backup"}
            </Button>
          </div>
          {backups.isLoading ? (
            <LoadingRows />
          ) : backups.error ? (
            <ErrorState error={backups.error} />
          ) : backups.data?.length === 0 ? (
            <EmptyState
              title="No backups"
              description="Create a backup to preserve the current namespace."
            />
          ) : (
            <BackupTable
              backups={backups.data ?? []}
              onRestore={openRestoreBackupDialog}
            />
          )}
        </Section>
      ) : (
        <div className="mx-auto grid max-w-4xl gap-4 text-sm">
          <h2 className="text-base font-semibold">General</h2>
          <LayerCard className="p-0">
            {renaming ? (
              <form
                className="grid gap-3 px-4 py-4"
                onSubmit={(event) => {
                  event.preventDefault();
                  if (
                    newName.trim() &&
                    newName.trim() !== namespace.data?.title &&
                    !rename.isPending
                  )
                    rename.mutate();
                }}
              >
                <div className="max-w-xs">
                  <Input
                    label="Name"
                    value={newName}
                    onChange={(event) => setNewName(event.target.value)}
                  />
                </div>
                <div className="flex justify-end gap-2">
                  <Button
                    type="button"
                    variant="ghost"
                    onClick={() => setRenaming(false)}
                  >
                    Cancel
                  </Button>
                  <Button
                    type="submit"
                    variant="primary"
                    disabled={
                      !newName.trim() ||
                      newName.trim() === namespace.data?.title ||
                      rename.isPending
                    }
                  >
                    {rename.isPending ? "Saving…" : "Save"}
                  </Button>
                </div>
                {rename.error ? <ErrorState error={rename.error} /> : null}
              </form>
            ) : (
              <div className="flex min-h-12 items-center justify-between gap-4 px-4 py-2">
                <span className="font-medium">Name</span>
                <span className="min-w-0 flex-1 truncate">
                  {namespace.data?.title ?? "—"}
                </span>
                <Button
                  variant="ghost"
                  onClick={() => {
                    setNewName(namespace.data?.title ?? "");
                    setRenaming(true);
                  }}
                >
                  Rename
                </Button>
              </div>
            )}
          </LayerCard>
          <LayerCard className="flex min-h-12 items-center justify-between gap-4 px-4 py-2">
            <span>
              Permanently delete this KV namespace and all of its key-value
              pairs.
            </span>
            <Button
              variant="ghost"
              className="text-kumo-danger"
              onClick={confirmDeleteNamespace}
            >
              Delete
            </Button>
          </LayerCard>
        </div>
      )}
    </div>
  );
}

function lines(value: string): string[] {
  return value
    .split("\n")
    .map((item) => item.trim())
    .filter(Boolean);
}
