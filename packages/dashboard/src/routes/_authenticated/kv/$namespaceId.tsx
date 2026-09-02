import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { z } from "zod";
import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import { Surface } from "@cloudflare/kumo/components/surface";
import { OperatorApiError, parsePageCursor, parseResourceId } from "@open-compute/operator-sdk";
import { ConfirmActionDialog } from "../../../components/ConfirmActionDialog";
import { CreateResourceDialog } from "../../../components/CreateResourceDialog";
import { DetailTabs } from "../../../components/DetailTabs";
import { BackLink, DataTable, ErrorState, LoadingState, PageHeader, StatusBadge } from "../../../components/PageLayout";
import { docsLinks } from "../../../lib/docs";
import { useMutationFeedback } from "../../../features/toast/useMutationFeedback";
import { formatTimestamp } from "../../../lib/format";
import { useAuth } from "../../../features/auth/AuthProvider";
import { queryKeys } from "../../../queries/keys";
import { invalidateKvNamespacesQueries } from "../../../queries/invalidate";

const kvNamespaceSearchSchema = z.object({
  tab: z.enum(["keys", "write", "backups"]).optional(),
  prefix: z.string().optional(),
  key: z.string().optional(),
});

export const Route = createFileRoute("/_authenticated/kv/$namespaceId")({
  validateSearch: search => kvNamespaceSearchSchema.parse(search),
  component: KvNamespacePage,
});

function KvNamespacePage() {
  const { namespaceId: namespaceIdParam } = Route.useParams();
  const { tab: tabParam, prefix = "", key: selectedKey = "" } = Route.useSearch();
  const activeTab = tabParam ?? "keys";
  const navigate = useNavigate({ from: Route.fullPath });
  const namespaceId = parseResourceId(namespaceIdParam);
  const { client, accountId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const [draftKey, setDraftKey] = useState("");
  const [draftValue, setDraftValue] = useState("");
  const [draftMetadata, setDraftMetadata] = useState("");
  const [draftTtl, setDraftTtl] = useState("");
  const [deleteKeyTarget, setDeleteKeyTarget] = useState<string | null>(null);
  const [deleteBackupTarget, setDeleteBackupTarget] = useState<string | null>(null);
  const [restoreBackupTarget, setRestoreBackupTarget] = useState<string | null>(null);
  const [mutationError, setMutationError] = useState<string | null>(null);

  const keysQuery = useInfiniteQuery({
    queryKey: queryKeys.kvKeys(accountId ?? "", namespaceIdParam, prefix),
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam, signal }) => client!.kv.listKeys({
      accountId: accountId!,
      namespaceId,
      ...(prefix ? { prefix } : {}),
      ...(pageParam !== undefined ? { cursor: parsePageCursor(pageParam) } : {}),
      limit: 100,
      signal,
    }),
    getNextPageParam: lastPage => (lastPage.listComplete ? undefined : lastPage.cursor ?? undefined),
    enabled: Boolean(client && accountId) && activeTab === "keys",
  });

  const allKeys = useMemo(
    () => (keysQuery.data?.pages ?? []).flatMap(page => page.keys),
    [keysQuery.data?.pages],
  );

  const valueQuery = useQuery({
    queryKey: queryKeys.kvValue(accountId ?? "", namespaceIdParam, selectedKey),
    queryFn: ({ signal }) => client!.kv.getValue({
      accountId: accountId!,
      namespaceId,
      key: selectedKey,
      signal,
    }),
    enabled: Boolean(client && accountId && selectedKey && activeTab === "keys"),
  });
  const backupsQuery = useQuery({
    queryKey: queryKeys.kvBackups(accountId ?? ""),
    queryFn: ({ signal }) => client!.kv.listBackups({ accountId: accountId!, signal }),
    enabled: Boolean(client && accountId) && activeTab === "backups",
  });
  const namespaceBackups = useMemo(
    () => (backupsQuery.data?.backups ?? []).filter(backup => backup.sourceResourceId === namespaceIdParam),
    [backupsQuery.data?.backups, namespaceIdParam],
  );
  const putMutation = useMutation({
    mutationFn: (params: { key: string; value: string; metadata?: unknown; expirationTtl?: number }) => client!.kv.putValue({
      accountId: accountId!,
      namespaceId,
      key: params.key,
      value: params.value,
      ...(params.metadata !== undefined ? { metadata: params.metadata } : {}),
      ...(params.expirationTtl !== undefined ? { expirationTtl: params.expirationTtl } : {}),
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async (_data, variables) => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.kvKeys(accountId!, namespaceIdParam, prefix) }),
        queryClient.invalidateQueries({
          queryKey: queryKeys.kvValue(accountId!, namespaceIdParam, variables.key),
        }),
      ]);
      void navigate({ search: prev => ({ ...prev, tab: "keys", key: variables.key }) });
      setDraftKey("");
      setDraftValue("");
      setDraftMetadata("");
      setDraftTtl("");
      setMutationError(null);
      feedback.success("KV value saved.");
    },
    onError: error => {
      setMutationError(error instanceof Error ? error.message : "Unable to save the KV value.");
      feedback.failure(error, "Unable to save the KV value.");
    },
  });
  const deleteMutation = useMutation({
    mutationFn: (key: string) => client!.kv.deleteValue({
      accountId: accountId!,
      namespaceId,
      key,
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: queryKeys.kvKeys(accountId!, namespaceIdParam, prefix),
      });
      setMutationError(null);
      setDeleteKeyTarget(null);
      void navigate({ search: prev => ({ ...prev, key: undefined }) });
    },
    onError: error => {
      setMutationError(
        error instanceof OperatorApiError ? error.message : "Unable to delete the key.",
      );
    },
  });
  const createBackupMutation = useMutation({
    mutationFn: () => client!.kv.createBackup({
      accountId: accountId!,
      namespaceId,
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: queryKeys.kvBackups(accountId!) });
      feedback.success("KV backup created.");
    },
    onError: error => {
      feedback.failure(error, "Unable to create backup.");
    },
  });
  const deleteBackupMutation = useMutation({
    mutationFn: (backupId: string) => client!.kv.deleteBackup({
      accountId: accountId!,
      backupId,
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: queryKeys.kvBackups(accountId!) });
      setDeleteBackupTarget(null);
      setMutationError(null);
      feedback.success("KV backup deleted.");
    },
    onError: error => {
      setMutationError(
        error instanceof OperatorApiError ? error.message : "Unable to delete backup.",
      );
    },
  });
  const restoreBackupMutation = useMutation({
    mutationFn: (params: { backupId: string; newName: string }) => client!.kv.restoreNamespace({
      accountId: accountId!,
      backupId: params.backupId,
      newName: params.newName,
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async result => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.kvBackups(accountId!) }),
        invalidateKvNamespacesQueries(queryClient, accountId!),
      ]);
      setRestoreBackupTarget(null);
      setMutationError(null);
      feedback.success(`Restored namespace ${result.resourceId}.`);
    },
    onError: error => {
      setMutationError(
        error instanceof OperatorApiError ? error.message : "Unable to restore namespace.",
      );
    },
  });
  const mutationPending =
    putMutation.isPending
    || deleteMutation.isPending
    || createBackupMutation.isPending
    || deleteBackupMutation.isPending
    || restoreBackupMutation.isPending;

  return (
    <div>
      <PageHeader
        title={`KV namespace ${namespaceIdParam}`}
        description="Browse and modify keys through bounded operator APIs."
        docsUrl={docsLinks.storage}
        resourceId={namespaceIdParam}
        resourceLabel="Namespace ID"
        actions={<BackLink to="/kv" label="Back to KV" />}
      />
      <DetailTabs
        tabs={[
          { id: "keys", label: "KV pairs" },
          { id: "write", label: "Write" },
          { id: "backups", label: "Backups" },
        ]}
        activeTab={activeTab}
        onTabChange={tabId => {
          void navigate({ search: prev => ({ ...prev, tab: tabId as "keys" | "write" | "backups" }) });
        }}
      />
      <ConfirmActionDialog
        title="Delete KV key"
        description="This permanently removes the key from the namespace."
        resourceLabel="the key name"
        confirmValue={deleteKeyTarget ?? ""}
        submitLabel="Delete key"
        submitVariant="destructive"
        open={Boolean(deleteKeyTarget)}
        errorMessage={deleteKeyTarget ? mutationError : null}
        isPending={deleteMutation.isPending}
        onClose={() => {
          setDeleteKeyTarget(null);
          setMutationError(null);
        }}
        onConfirm={() => {
          if (!deleteKeyTarget) return;
          deleteMutation.mutate(deleteKeyTarget);
        }}
      />
      <ConfirmActionDialog
        title="Delete KV backup"
        description="This permanently retires the backup identity and removes its stored object."
        resourceLabel="the backup ID"
        confirmValue={deleteBackupTarget ?? ""}
        submitLabel="Delete backup"
        submitVariant="destructive"
        open={Boolean(deleteBackupTarget)}
        errorMessage={deleteBackupTarget ? mutationError : null}
        isPending={deleteBackupMutation.isPending}
        onClose={() => {
          setDeleteBackupTarget(null);
          setMutationError(null);
        }}
        onConfirm={() => {
          if (!deleteBackupTarget) return;
          deleteBackupMutation.mutate(deleteBackupTarget);
        }}
      />
      <CreateResourceDialog
        title="Restore KV namespace"
        description="Creates a new namespace from the selected backup."
        nameLabel="New namespace name"
        namePlaceholder="restored-namespace"
        submitLabel="Restore namespace"
        open={Boolean(restoreBackupTarget)}
        errorMessage={restoreBackupTarget ? mutationError : null}
        isPending={restoreBackupMutation.isPending}
        onClose={() => {
          setRestoreBackupTarget(null);
          setMutationError(null);
        }}
        onSubmit={newName => {
          if (!restoreBackupTarget) return;
          restoreBackupMutation.mutate({ backupId: restoreBackupTarget, newName });
        }}
      />
      {activeTab === "keys" ? (
        <>
          <div className="mb-4 flex gap-2">
            <Input
              value={prefix}
              onChange={event => {
                void navigate({ search: prev => ({ ...prev, prefix: event.target.value || undefined, key: undefined }) });
              }}
              placeholder="Filter by prefix"
            />
          </div>
          <div className="grid gap-4 xl:grid-cols-[minmax(0,1.2fr)_minmax(0,0.8fr)]">
            {keysQuery.isLoading ? (
              <LoadingState />
            ) : keysQuery.error ? (
              <ErrorState message="Unable to load KV keys." />
            ) : (
              <>
                <DataTable
                  columns={[
                    { key: "name", label: "Key" },
                    { key: "expiration", label: "Expiration" },
                    { key: "actions", label: "" },
                  ]}
                  rows={allKeys.map(key => ({
                    name: key.name,
                    expiration: key.expiration ? formatTimestamp(key.expiration * 1000) : "Never",
                    actions: (
                      <button
                        type="button"
                        className="text-sm text-kumo-link"
                        onClick={() => {
                          void navigate({ search: prev => ({ ...prev, key: key.name }) });
                        }}
                      >
                        View value
                      </button>
                    ),
                  }))}
                  emptyLabel="This namespace has no keys yet."
                />
                {keysQuery.hasNextPage ? (
                  <div className="mt-4 flex justify-center xl:col-span-2">
                    <Button
                      variant="secondary"
                      disabled={keysQuery.isFetchingNextPage}
                      onClick={() => void keysQuery.fetchNextPage()}
                    >
                      {keysQuery.isFetchingNextPage ? "Loading…" : "Load more keys"}
                    </Button>
                  </div>
                ) : null}
              </>
            )}
            <div className="space-y-4">
              <Surface className="p-4">
                <div className="mb-2 text-sm font-medium">Value preview</div>
                {!selectedKey ? (
                  <div className="text-sm text-kumo-subtle">Select a key to inspect its value.</div>
                ) : valueQuery.isLoading ? (
                  <LoadingState label="Loading value…" />
                ) : valueQuery.error ? (
                  <ErrorState message="Unable to load the selected value." />
                ) : (
                  <>
                    <pre className="max-h-96 overflow-auto rounded-md bg-kumo-control/40 p-3 text-xs whitespace-pre-wrap break-all">
                      {valueQuery.data?.value ?? "null"}
                    </pre>
                    {valueQuery.data?.metadata !== undefined ? (
                      <div className="mt-3">
                        <div className="mb-1 text-xs font-medium text-kumo-subtle">Metadata</div>
                        <pre className="max-h-40 overflow-auto rounded-md bg-kumo-control/40 p-3 text-xs whitespace-pre-wrap break-all">
                          {JSON.stringify(valueQuery.data.metadata, null, 2)}
                        </pre>
                      </div>
                    ) : null}
                    <div className="mt-3 flex gap-2">
                      <Button
                        variant="destructive"
                        disabled={mutationPending}
                        onClick={() => {
                          setMutationError(null);
                          setDeleteKeyTarget(selectedKey);
                        }}
                      >
                        Delete key
                      </Button>
                    </div>
                  </>
                )}
              </Surface>
            </div>
          </div>
        </>
      ) : null}
      {activeTab === "write" ? (
        <Surface className="max-w-xl p-4">
          <div className="mb-2 text-sm font-medium">Put value</div>
          <div className="space-y-2">
            <Input
              label="Key"
              value={draftKey}
              onChange={event => setDraftKey(event.target.value)}
              placeholder="Key"
            />
            <textarea
              className="min-h-24 w-full rounded-md border border-kumo-line bg-kumo-control px-3 py-2 text-sm"
              value={draftValue}
              onChange={event => setDraftValue(event.target.value)}
              placeholder="Value"
              aria-label="Value"
            />
            <textarea
              className="min-h-20 w-full rounded-md border border-kumo-line bg-kumo-control px-3 py-2 text-sm"
              value={draftMetadata}
              onChange={event => setDraftMetadata(event.target.value)}
              placeholder='Optional JSON metadata, for example {"region":"eu"}'
              aria-label="JSON metadata"
            />
            <Input
              label="Expiration TTL (seconds, optional)"
              type="number"
              min={60}
              step={1}
              value={draftTtl}
              onChange={event => setDraftTtl(event.target.value)}
            />
            {mutationError ? <p className="text-sm text-kumo-danger" role="alert">{mutationError}</p> : null}
            <Button
              variant="primary"
              disabled={mutationPending || !draftKey.trim() || (draftTtl !== "" && (!Number.isSafeInteger(Number(draftTtl)) || Number(draftTtl) < 60))}
              onClick={() => {
                let metadata: unknown;
                if (draftMetadata.trim()) {
                  try {
                    metadata = JSON.parse(draftMetadata);
                  } catch {
                    setMutationError("Metadata must be valid JSON.");
                    return;
                  }
                }
                setMutationError(null);
                putMutation.mutate({
                  key: draftKey.trim(),
                  value: draftValue,
                  ...(metadata !== undefined ? { metadata } : {}),
                  ...(draftTtl ? { expirationTtl: Number(draftTtl) } : {}),
                });
              }}
            >
              Save value
            </Button>
          </div>
        </Surface>
      ) : null}
      {activeTab === "backups" ? (
        <section className="space-y-4">
          <div>
            <Button
              variant="primary"
              disabled={createBackupMutation.isPending}
              onClick={() => createBackupMutation.mutate()}
            >
              {createBackupMutation.isPending ? "Creating…" : "Create backup"}
            </Button>
          </div>
          {backupsQuery.isLoading ? (
            <LoadingState label="Loading backups…" />
          ) : backupsQuery.error ? (
            <ErrorState message="Unable to load backups." />
          ) : (
            <DataTable
              columns={[
                { key: "id", label: "Backup ID" },
                { key: "state", label: "State" },
                { key: "size", label: "Size" },
                { key: "created", label: "Created" },
                { key: "actions", label: "" },
              ]}
              rows={namespaceBackups.map(backup => ({
                id: <code className="[font-size:0.9em]">{backup.id}</code>,
                state: <StatusBadge value={backup.state} />,
                size: backup.sizeBytes?.toLocaleString() ?? "—",
                created: formatTimestamp(backup.createdAtMs),
                actions: (
                  <div className="flex gap-2">
                    {backup.state === "ready" ? (
                      <button
                        type="button"
                        className="text-sm text-kumo-link"
                        disabled={mutationPending}
                        onClick={() => {
                          setMutationError(null);
                          setRestoreBackupTarget(backup.id);
                        }}
                      >
                        Restore
                      </button>
                    ) : null}
                    {backup.state !== "tombstoned" ? (
                      <button
                        type="button"
                        className="text-sm text-kumo-danger"
                        disabled={mutationPending}
                        onClick={() => {
                          setMutationError(null);
                          setDeleteBackupTarget(backup.id);
                        }}
                      >
                        Delete
                      </button>
                    ) : null}
                  </div>
                ),
              }))}
              emptyLabel="No backups exist for this namespace."
            />
          )}
        </section>
      ) : null}
    </div>
  );
}
