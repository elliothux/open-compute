import { useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { Button } from "@cloudflare/kumo/components/button";
import { CreateResourceDialog } from "../../../components/CreateResourceDialog";
import { DataTable, ErrorState, LoadingState, PageHeader, SectionHeader, StatusBadge } from "../../../components/PageLayout";
import { useAuth } from "../../../features/auth/AuthProvider";
import { useMutationFeedback } from "../../../features/toast/useMutationFeedback";

export const Route = createFileRoute("/_authenticated/d1/$databaseId")({ component: D1DetailPage });

function D1DetailPage() {
  const { databaseId } = Route.useParams();
  const { client, accountId } = useAuth();
  const feedback = useMutationFeedback();
  const enabled = client !== null && accountId !== null;
  const [sql, setSql] = useState("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name;");
  const [restoreTarget, setRestoreTarget] = useState<string | null>(null);
  const [mutationError, setMutationError] = useState<string | null>(null);
  const database = useQuery({
    queryKey: ["cloudflare-v4", "d1", databaseId],
    queryFn: ({ signal }) => client!.cloudflare.d1.database.get(databaseId, { account_id: accountId! }, { signal }),
    enabled,
  });
  const backups = useQuery({
    queryKey: ["cloudflare-v4", "d1", databaseId, "backups"],
    queryFn: ({ signal }) => client!.openCompute.backups.d1.list(accountId!, databaseId, { signal }),
    enabled,
  });
  const query = useMutation({
    mutationFn: () => client!.cloudflare.d1.database.query(databaseId, { account_id: accountId!, sql }),
    onError: error => feedback.failure(error, "Unable to execute the D1 query."),
  });
  const createBackup = useMutation({
    mutationFn: () => client!.openCompute.backups.d1.create(accountId!, databaseId),
    onSuccess: async () => { await backups.refetch(); feedback.success("D1 backup created."); },
    onError: error => feedback.failure(error, "Unable to create the D1 backup."),
  });
  const restore = useMutation({
    mutationFn: ({ backupID, name }: { backupID: string; name: string }) =>
      client!.openCompute.backups.d1.restore(accountId!, backupID, { name }),
    onSuccess: async restored => {
      setRestoreTarget(null);
      setMutationError(null);
      await backups.refetch();
      feedback.success(`D1 backup restored as ${restored.name}.`);
    },
    onError: error => {
      setMutationError(error instanceof Error ? error.message : "Unable to restore the D1 backup.");
      feedback.failure(error, "Unable to restore the D1 backup.");
    },
  });
  return <div>
    <PageHeader title={database.data?.name ?? "D1 database"} resourceId={databaseId} actions={<Button variant="secondary" onClick={() => createBackup.mutate()} disabled={createBackup.isPending}>Create backup</Button>} />
    <CreateResourceDialog title="Restore D1 backup" description="Create a new D1 database from the selected backup." nameLabel="New database name" namePlaceholder="restored-database" submitLabel="Restore backup" open={restoreTarget !== null} errorMessage={restoreTarget ? mutationError : null} isPending={restore.isPending} onClose={() => { setRestoreTarget(null); setMutationError(null); }} onSubmit={name => { if (restoreTarget) restore.mutate({ backupID: restoreTarget, name }); }} />
    {database.isLoading || backups.isLoading ? <LoadingState /> : database.error || backups.error ? <ErrorState message="Unable to load D1 database details." /> : <>
      <DataTable columns={[{ key: "property", label: "Property" }, { key: "value", label: "Value" }]} rows={[
        { property: "Created", value: database.data?.created_at ?? "unknown" },
        { property: "Tables", value: database.data?.num_tables ?? "unknown" },
        { property: "File size", value: database.data?.file_size ?? "unknown" },
        { property: "Jurisdiction", value: database.data?.jurisdiction ?? "default" },
        { property: "Read replication", value: database.data?.read_replication?.mode ?? "unknown" },
      ]} />
      <div className="mt-6">
        <SectionHeader title="Query" description="Execute SQL through the official D1 query endpoint." />
        <textarea aria-label="SQL query" className="min-h-36 w-full rounded border p-3 font-mono text-sm" value={sql} onChange={event => setSql(event.target.value)} />
        <div className="mt-3"><Button variant="primary" disabled={!sql.trim() || query.isPending} onClick={() => query.mutate()}>{query.isPending ? "Running…" : "Run query"}</Button></div>
        {query.error ? <div className="mt-3"><ErrorState message="Query failed." /></div> : null}
        {query.data ? <pre className="mt-3 max-h-96 overflow-auto whitespace-pre-wrap rounded bg-kumo-tinted p-4 text-sm">{JSON.stringify(query.data.result, null, 2)}</pre> : null}
      </div>
      <div className="mt-6">
        <SectionHeader title="Backups" />
        <DataTable columns={[{ key: "id", label: "Backup" }, { key: "state", label: "State" }, { key: "size", label: "Size" }, { key: "created", label: "Created" }, { key: "actions", label: "" }]} rows={(backups.data ?? []).map(backup => ({
          id: backup.id,
          state: <StatusBadge value={backup.state} />,
          size: backup.size ?? "unknown",
          created: backup.created_on,
          actions: <Button variant="secondary" onClick={() => setRestoreTarget(backup.id)} disabled={restore.isPending}>Restore</Button>,
        }))} emptyLabel="No backups found." />
      </div>
    </>}
  </div>;
}
