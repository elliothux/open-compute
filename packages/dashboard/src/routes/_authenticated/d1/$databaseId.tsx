import { OperatorApiError, parseResourceId, parseSha256Digest } from "@open-compute/operator-sdk";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { z } from "zod";
import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import { Surface } from "@cloudflare/kumo/components/surface";
import { CreateResourceDialog } from "../../../components/CreateResourceDialog";
import { DetailTabs } from "../../../components/DetailTabs";
import { BackLink, DataTable, ErrorState, LoadingState, PageHeader, StatusBadge } from "../../../components/PageLayout";
import { docsLinks } from "../../../lib/docs";
import { useMutationFeedback } from "../../../features/toast/useMutationFeedback";
import { sha256Hex } from "../../../lib/hash";
import { formatTimestamp } from "../../../lib/format";
import { useAuth } from "../../../features/auth/AuthProvider";
import { queryKeys } from "../../../queries/keys";
import { invalidateD1DatabasesQueries } from "../../../queries/invalidate";

const d1DetailSearchSchema = z.object({
  tab: z.enum(["tables", "query", "migrations", "backups"]).optional(),
});

export const Route = createFileRoute("/_authenticated/d1/$databaseId")({
  validateSearch: search => d1DetailSearchSchema.parse(search),
  component: D1StudioPage,
});

function D1StudioPage() {
  const { databaseId: databaseIdParam } = Route.useParams();
  const { tab: tabParam } = Route.useSearch();
  const activeTab = tabParam ?? "tables";
  const navigate = useNavigate({ from: Route.fullPath });
  const databaseId = parseResourceId(databaseIdParam);
  const { client, accountId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const [sql, setSql] = useState("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name;");
  const [migrationId, setMigrationId] = useState("1");
  const [migrationName, setMigrationName] = useState("");
  const [migrationSql, setMigrationSql] = useState("");
  const [restoreBackupTarget, setRestoreBackupTarget] = useState<string | null>(null);
  const [mutationError, setMutationError] = useState<string | null>(null);

  const tablesQuery = useQuery({
    queryKey: queryKeys.d1Tables(accountId ?? "", databaseIdParam),
    queryFn: ({ signal }) => client!.d1.listTables({ accountId: accountId!, databaseId, signal }),
    enabled: Boolean(client && accountId) && activeTab === "tables",
  });
  const migrationsQuery = useQuery({
    queryKey: queryKeys.d1Migrations(accountId ?? "", databaseIdParam),
    queryFn: ({ signal }) => client!.d1.listMigrations({ accountId: accountId!, databaseId, signal }),
    enabled: Boolean(client && accountId) && activeTab === "migrations",
  });
  const backupsQuery = useQuery({
    queryKey: queryKeys.d1Backups(accountId ?? "", databaseIdParam),
    queryFn: ({ signal }) => client!.d1.listBackups({ accountId: accountId!, databaseId, signal }),
    enabled: Boolean(client && accountId) && activeTab === "backups",
  });
  const queryMutation = useMutation({
    mutationFn: () => client!.d1.query({ accountId: accountId!, databaseId, sql }),
    onSuccess: () => feedback.success("Query completed."),
    onError: error => feedback.failure(error, "Query failed."),
  });
  const applyMigrationMutation = useMutation({
    mutationFn: async () => {
      const id = Number.parseInt(migrationId, 10);
      if (!Number.isFinite(id) || id <= 0) throw new Error("Migration id must be a positive integer.");
      const trimmedSql = migrationSql.trim();
      const trimmedName = migrationName.trim();
      if (!trimmedName || !trimmedSql) throw new Error("Migration name and SQL are required.");
      const digest = parseSha256Digest(await sha256Hex(trimmedSql));
      return client!.d1.applyMigrations({
        accountId: accountId!,
        databaseId,
        idempotencyKey: crypto.randomUUID(),
        migrations: [{ id, name: trimmedName, sha256: digest, sql: trimmedSql }],
      });
    },
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: queryKeys.d1Migrations(accountId!, databaseIdParam) });
      await queryClient.invalidateQueries({ queryKey: queryKeys.d1Tables(accountId!, databaseIdParam) });
      setMigrationSql("");
      feedback.success("Migration applied.");
    },
    onError: error => feedback.failure(error, "Migration failed."),
  });
  const createBackupMutation = useMutation({
    mutationFn: () => client!.d1.createBackup({
      accountId: accountId!,
      databaseId,
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: queryKeys.d1Backups(accountId!, databaseIdParam) });
      feedback.success("Backup created.");
    },
    onError: error => feedback.failure(error, "Backup failed."),
  });
  const restoreBackupMutation = useMutation({
    mutationFn: (params: { backupId: string; newName: string }) => client!.d1.restoreDatabase({
      accountId: accountId!,
      backupId: params.backupId,
      newName: params.newName,
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async result => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.d1Backups(accountId!, databaseIdParam) }),
        invalidateD1DatabasesQueries(queryClient, accountId!),
      ]);
      setRestoreBackupTarget(null);
      setMutationError(null);
      feedback.success(`Restored database ${result.resourceId}.`);
    },
    onError: error => {
      setMutationError(
        error instanceof OperatorApiError ? error.message : "Unable to restore database.",
      );
    },
  });

  const columns = queryMutation.data?.results[0]
    ? Object.keys(queryMutation.data.results[0]).map(key => ({ key, label: key }))
    : [];

  return (
    <div>
      <PageHeader
        title={`D1 database ${databaseIdParam}`}
        description="Inspect schema, run bounded SQL, apply migrations, and manage backups."
        docsUrl={docsLinks.storage}
        resourceId={databaseIdParam}
        resourceLabel="Database ID"
        actions={<BackLink to="/d1" label="Back to D1" />}
      />
      <DetailTabs
        tabs={[
          { id: "tables", label: "Tables" },
          { id: "query", label: "Query" },
          { id: "migrations", label: "Migrations" },
          { id: "backups", label: "Backups" },
        ]}
        activeTab={activeTab}
        onTabChange={tabId => {
          void navigate({
            search: prev => ({
              ...prev,
              tab: tabId as "tables" | "query" | "migrations" | "backups",
            }),
          });
        }}
      />
      <CreateResourceDialog
        title="Restore D1 database"
        description="Creates a new database from the selected backup."
        nameLabel="New database name"
        namePlaceholder="restored-database"
        submitLabel="Restore database"
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
      {activeTab === "tables" ? (
        <Surface className="p-4">
          {tablesQuery.isLoading ? (
            <LoadingState label="Loading tables…" />
          ) : tablesQuery.error ? (
            <ErrorState message="Unable to load tables." />
          ) : (
            <ul className="space-y-2 text-sm">
              {(tablesQuery.data?.tables ?? []).map(table => (
                <li key={table.name}>
                  <button
                    type="button"
                    className="text-kumo-link"
                    onClick={() => {
                      setSql(`SELECT * FROM ${table.name} LIMIT 50;`);
                      void navigate({ search: prev => ({ ...prev, tab: "query" }) });
                    }}
                  >
                    {table.name}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </Surface>
      ) : null}
      {activeTab === "query" ? (
        <Surface className="p-4">
          <div className="mb-3 text-sm font-medium">SQL editor</div>
          <textarea value={sql} onChange={event => setSql(event.target.value)} rows={8} className="font-mono text-sm" />
          <div className="mt-3 flex gap-2">
            <Button onClick={() => queryMutation.mutate()} disabled={queryMutation.isPending}>
              {queryMutation.isPending ? "Running…" : "Run query"}
            </Button>
          </div>
          {queryMutation.error ? <div className="mt-3"><ErrorState message="Query failed." /></div> : null}
          {queryMutation.data ? (
            <div className="mt-4">
              <DataTable
                columns={columns.length > 0 ? columns : [{ key: "result", label: "Result" }]}
                rows={(queryMutation.data.results ?? []).map(row =>
                  columns.length > 0
                    ? Object.fromEntries(columns.map(column => [column.key, String(row[column.key] ?? "")]))
                    : { result: JSON.stringify(row) },
                )}
                emptyLabel="The query returned no rows."
              />
            </div>
          ) : null}
        </Surface>
      ) : null}
      {activeTab === "migrations" ? (
        <section className="space-y-4">
          <Surface className="p-4">
            <div className="mb-3 text-sm font-medium">Apply migration</div>
            <div className="grid gap-3 md:grid-cols-2">
              <Input label="Migration id" value={migrationId} onChange={event => setMigrationId(event.target.value)} />
              <Input label="Name" value={migrationName} onChange={event => setMigrationName(event.target.value)} placeholder="0001_init.sql" />
            </div>
            <textarea
              className="mt-3 w-full font-mono text-sm"
              rows={6}
              value={migrationSql}
              onChange={event => setMigrationSql(event.target.value)}
              placeholder="CREATE TABLE ..."
            />
            <Button
              className="mt-3"
              variant="primary"
              disabled={applyMigrationMutation.isPending}
              onClick={() => applyMigrationMutation.mutate()}
            >
              {applyMigrationMutation.isPending ? "Applying…" : "Apply migration"}
            </Button>
          </Surface>
          {migrationsQuery.isLoading ? (
            <LoadingState label="Loading migrations…" />
          ) : migrationsQuery.error ? (
            <ErrorState message="Unable to load migrations." />
          ) : (
            <DataTable
              columns={[
                { key: "id", label: "ID" },
                { key: "name", label: "Name" },
                { key: "sha256", label: "SHA-256" },
                { key: "applied", label: "Applied" },
              ]}
              rows={(migrationsQuery.data?.migrations ?? []).map(migration => ({
                id: migration.id,
                name: migration.name,
                sha256: <code className="[font-size:0.9em]">{migration.sha256.slice(0, 12)}…</code>,
                applied: formatTimestamp(migration.appliedAtMs),
              }))}
              emptyLabel="No migrations have been applied yet."
            />
          )}
        </section>
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
              rows={(backupsQuery.data?.backups ?? []).map(backup => ({
                id: <code className="[font-size:0.9em]">{backup.id}</code>,
                state: <StatusBadge value={backup.state} />,
                size: backup.sizeBytes?.toLocaleString() ?? "—",
                created: formatTimestamp(backup.createdAtMs),
                actions: backup.state === "ready" ? (
                  <button
                    type="button"
                    className="text-sm text-kumo-link"
                    disabled={restoreBackupMutation.isPending}
                    onClick={() => {
                      setMutationError(null);
                      setRestoreBackupTarget(backup.id);
                    }}
                  >
                    Restore
                  </button>
                ) : null,
              }))}
              emptyLabel="No backups exist for this database."
            />
          )}
        </section>
      ) : null}
    </div>
  );
}
