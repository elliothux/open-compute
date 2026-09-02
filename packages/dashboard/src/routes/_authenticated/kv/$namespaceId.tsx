import { useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import { ConfirmActionDialog } from "../../../components/ConfirmActionDialog";
import { DataTable, ErrorState, LoadingState, PageHeader, SectionHeader, StatusBadge } from "../../../components/PageLayout";
import { useAuth } from "../../../features/auth/AuthProvider";
import { useMutationFeedback } from "../../../features/toast/useMutationFeedback";

export const Route = createFileRoute("/_authenticated/kv/$namespaceId")({ component: KvDetailPage });

function KvDetailPage() {
  const { namespaceId } = Route.useParams();
  const { client, accountId } = useAuth();
  const feedback = useMutationFeedback();
  const enabled = client !== null && accountId !== null;
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [draftKey, setDraftKey] = useState("");
  const [draftValue, setDraftValue] = useState("");
  const [draftMetadata, setDraftMetadata] = useState("");
  const [draftTtl, setDraftTtl] = useState("");
  const [deleteKeyTarget, setDeleteKeyTarget] = useState<string | null>(null);
  const [restoreTarget, setRestoreTarget] = useState<string | null>(null);
  const [mutationError, setMutationError] = useState<string | null>(null);
  const keys = useQuery({
    queryKey: ["cloudflare-v4", "kv", namespaceId, "keys"],
    queryFn: ({ signal }) => client!.cloudflare.kv.namespaces.keys.list(namespaceId, { account_id: accountId! }, { signal }),
    enabled,
  });
  const value = useQuery({
    queryKey: ["cloudflare-v4", "kv", namespaceId, "values", selectedKey],
    queryFn: async ({ signal }) => {
      const response = await client!.cloudflare.kv.namespaces.values.get(selectedKey!, {
        account_id: accountId!,
        namespace_id: namespaceId,
      }, { signal });
      return response.text();
    },
    enabled: enabled && selectedKey !== null,
  });
  const backups = useQuery({
    queryKey: ["cloudflare-v4", "kv", namespaceId, "backups"],
    queryFn: ({ signal }) => client!.openCompute.backups.kv.list(accountId!, namespaceId, { signal }),
    enabled,
  });
  const put = useMutation({
    mutationFn: () => {
      let metadata: unknown = undefined;
      if (draftMetadata.trim()) metadata = JSON.parse(draftMetadata) as unknown;
      const ttl = draftTtl.trim() ? Number(draftTtl) : undefined;
      if (ttl !== undefined && (!Number.isSafeInteger(ttl) || ttl < 60)) throw new Error("Expiration TTL must be an integer of at least 60 seconds.");
      return client!.cloudflare.kv.namespaces.values.update(draftKey.trim(), {
        account_id: accountId!,
        namespace_id: namespaceId,
        value: draftValue,
        ...(metadata === undefined ? {} : { metadata }),
        ...(ttl === undefined ? {} : { expiration_ttl: ttl }),
      });
    },
    onSuccess: async () => {
      setMutationError(null);
      setSelectedKey(draftKey.trim());
      await keys.refetch();
      feedback.success("KV value saved.");
    },
    onError: error => {
      setMutationError(error instanceof Error ? error.message : "Unable to save the KV value.");
      feedback.failure(error, "Unable to save the KV value.");
    },
  });
  const remove = useMutation({
    mutationFn: (key: string) => client!.cloudflare.kv.namespaces.values.delete(key, {
      account_id: accountId!,
      namespace_id: namespaceId,
    }),
    onSuccess: async () => {
      setSelectedKey(null);
      setDeleteKeyTarget(null);
      setMutationError(null);
      await keys.refetch();
      feedback.success("KV key deleted.");
    },
    onError: error => {
      setMutationError(error instanceof Error ? error.message : "Unable to delete the KV key.");
      feedback.failure(error, "Unable to delete the KV key.");
    },
  });
  const createBackup = useMutation({
    mutationFn: () => client!.openCompute.backups.kv.create(accountId!, namespaceId),
    onSuccess: async () => { await backups.refetch(); feedback.success("KV backup created."); },
    onError: error => feedback.failure(error, "Unable to create the KV backup."),
  });
  const restore = useMutation({
    mutationFn: (backupID: string) => client!.openCompute.backups.kv.restore(accountId!, backupID),
    onSuccess: async () => {
      setRestoreTarget(null);
      await Promise.all([keys.refetch(), backups.refetch()]);
      feedback.success("KV backup restored.");
    },
    onError: error => {
      setMutationError(error instanceof Error ? error.message : "Unable to restore the KV backup.");
      feedback.failure(error, "Unable to restore the KV backup.");
    },
  });
  return <div>
    <PageHeader title="KV namespace" resourceId={namespaceId} actions={<Button variant="secondary" onClick={() => createBackup.mutate()} disabled={createBackup.isPending}>Create backup</Button>} />
    <ConfirmActionDialog title="Delete KV key" description="This permanently removes the selected key." resourceLabel="key" confirmValue={deleteKeyTarget ?? ""} submitLabel="Delete key" submitVariant="destructive" open={deleteKeyTarget !== null} errorMessage={deleteKeyTarget ? mutationError : null} isPending={remove.isPending} onClose={() => { setDeleteKeyTarget(null); setMutationError(null); }} onConfirm={() => { if (deleteKeyTarget) remove.mutate(deleteKeyTarget); }} />
    <ConfirmActionDialog title="Restore KV backup" description="Restore the selected backup to this namespace." resourceLabel="backup ID" confirmValue={restoreTarget ?? ""} submitLabel="Restore backup" open={restoreTarget !== null} errorMessage={restoreTarget ? mutationError : null} isPending={restore.isPending} onClose={() => { setRestoreTarget(null); setMutationError(null); }} onConfirm={() => { if (restoreTarget) restore.mutate(restoreTarget); }} />
    {keys.isLoading || backups.isLoading ? <LoadingState /> : keys.error || backups.error ? <ErrorState message="Unable to load KV namespace details." /> : <>
      <SectionHeader title="Keys" />
      <div className="grid gap-6 lg:grid-cols-2">
        <DataTable columns={[{ key: "name", label: "Key" }, { key: "expiration", label: "Expiration" }, { key: "actions", label: "" }]} rows={(keys.data?.result ?? []).map(key => ({
          name: key.name,
          expiration: key.expiration ?? "Never",
          actions: <div className="flex gap-2"><Button variant="secondary" onClick={() => setSelectedKey(key.name)}>View</Button><Button variant="destructive" onClick={() => setDeleteKeyTarget(key.name)}>Delete</Button></div>,
        }))} emptyLabel="No keys found." />
        <div>
          {selectedKey === null ? <p className="text-sm text-kumo-subtle">Select a key to read its value.</p> : value.isLoading ? <LoadingState /> : value.error ? <ErrorState message="Unable to read the KV value." /> : <pre className="max-h-80 overflow-auto whitespace-pre-wrap rounded bg-kumo-tinted p-4 text-sm">{value.data}</pre>}
        </div>
      </div>
      <div className="mt-6">
        <SectionHeader title="Put value" />
        <div className="grid gap-3">
          <Input label="Key" value={draftKey} onChange={event => setDraftKey(event.target.value)} />
          <textarea aria-label="Value" className="min-h-24 rounded border p-3 font-mono text-sm" value={draftValue} onChange={event => setDraftValue(event.target.value)} />
          <Input label="JSON metadata (optional)" value={draftMetadata} onChange={event => setDraftMetadata(event.target.value)} />
          <Input label="Expiration TTL seconds (optional, minimum 60)" type="number" min={60} value={draftTtl} onChange={event => setDraftTtl(event.target.value)} />
          <div><Button variant="primary" disabled={!draftKey.trim() || put.isPending} onClick={() => put.mutate()}>Save value</Button></div>
          {mutationError ? <p className="text-sm text-kumo-danger" role="alert">{mutationError}</p> : null}
        </div>
      </div>
      <div className="mt-6">
        <SectionHeader title="Backups" />
        <DataTable columns={[{ key: "id", label: "Backup" }, { key: "state", label: "State" }, { key: "size", label: "Size" }, { key: "created", label: "Created" }, { key: "actions", label: "" }]} rows={(backups.data ?? []).map(backup => ({
          id: backup.id,
          state: <StatusBadge value={backup.state} />,
          size: backup.size,
          created: backup.created_on,
          actions: <Button variant="secondary" onClick={() => setRestoreTarget(backup.id)} disabled={restore.isPending}>Restore</Button>,
        }))} emptyLabel="No backups found." />
      </div>
    </>}
  </div>;
}
