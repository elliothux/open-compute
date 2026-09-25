import { Button } from "@cloudflare/kumo/components/button";
import { Input, Textarea } from "@cloudflare/kumo/components/input";
import { LayerCard } from "@cloudflare/kumo/components/layer-card";
import { Select } from "@cloudflare/kumo/components/select";
import { Table } from "@cloudflare/kumo/components/table";
import { Tabs } from "@cloudflare/kumo/components/tabs";
import {
  IconCopy,
  IconInfoCircle,
  IconPlayerPlay,
  IconRefresh,
  IconUpload,
} from "@tabler/icons-react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useSetAtom } from "jotai";
import { useEffect, useState } from "react";
import type { D1MigrationRequest } from "@open-compute/sdk";
import { BackupTable } from "../../../components/backup-table";
import { CloudflareProductIcon } from "../../../components/cloudflare-product-icons";
import { CodeBlock } from "../../../components/code-block";
import {
  DefinitionList,
  EmptyState,
  ErrorState,
  LoadingRows,
  PageHeader,
  Panel,
  Section,
  StatGrid,
} from "../../../components/dashboard-page";
import { closeAlert, openAlert } from "../../../components/dialog-manager";
import {
  openConfirmDeleteDialog,
  openResourceNameDialog,
} from "../../../components/resource-dialog";
import { useAuth } from "../../../features/auth/auth-atoms";
import { detailBreadcrumbAtom } from "../../../features/navigation/detail-breadcrumb-atom";
import { useMutationFeedback } from "../../../features/toast/use-mutation-feedback";
import { formatBytes } from "../../../lib/format";
import { d1DatabaseQuery } from "../../../lib/query-options";

export const Route = createFileRoute("/_authenticated/d1/$databaseId")({
  validateSearch: (search: Record<string, unknown>): { tab?: Tab } =>
    search.tab === "console" ||
    search.tab === "transfer" ||
    search.tab === "recovery" ||
    search.tab === "migrations" ||
    search.tab === "settings"
      ? { tab: search.tab }
      : {},
  loader: ({ context, params }) => {
    const { client, instanceId } = context.auth;
    if (!client || !instanceId) return;
    return context.queryClient.ensureQueryData(
      d1DatabaseQuery(client, instanceId, params.databaseId),
    );
  },
  component: D1DetailPage,
});

type Tab =
  "overview" | "console" | "transfer" | "recovery" | "migrations" | "settings";
type ImportAction = "init" | "ingest" | "poll";

function parseMigrations(source: string): D1MigrationRequest {
  const value: unknown = JSON.parse(source);
  if (!Array.isArray(value))
    throw new Error("Migrations must be a JSON array.");
  return value.map((item, index) => {
    if (typeof item !== "object" || item === null)
      throw new Error(`Migration ${index + 1} must be an object.`);
    const row = item as Record<string, unknown>;
    if (
      !Number.isSafeInteger(row.id) ||
      typeof row.name !== "string" ||
      typeof row.sha256 !== "string" ||
      typeof row.sql !== "string"
    )
      throw new Error(
        `Migration ${index + 1} needs integer id plus name, sha256, and sql strings.`,
      );
    return {
      id: row.id as number,
      name: row.name,
      sha256: row.sha256,
      sql: row.sql,
    };
  });
}

function D1DetailPage() {
  const { databaseId } = Route.useParams();
  const { tab: searchTab } = Route.useSearch();
  const tab = searchTab ?? "overview";
  const { client, instanceId: selectedInstanceId } = useAuth();
  const feedback = useMutationFeedback();
  const navigate = useNavigate();
  const setDetailBreadcrumb = useSetAtom(detailBreadcrumbAtom);
  const enabled = client !== null && selectedInstanceId !== null;
  const setTab = (next: Tab) =>
    void navigate({
      to: "/d1/$databaseId",
      params: { databaseId },
      search: next === "overview" ? {} : { tab: next },
    });
  const [mode, setMode] = useState<"query" | "raw">("query");
  const [sql, setSql] = useState(
    "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name;",
  );
  const [params, setParams] = useState("[]");
  const [consoleResult, setConsoleResult] = useState<unknown>(null);
  const [exportBookmark, setExportBookmark] = useState("");
  const [exportResult, setExportResult] = useState<unknown>(null);
  const [importAction, setImportAction] = useState<ImportAction>("init");
  const [importEtag, setImportEtag] = useState("");
  const [importFilename, setImportFilename] = useState("");
  const [importBookmark, setImportBookmark] = useState("");
  const [importResult, setImportResult] = useState<unknown>(null);
  const [restoreMode, setRestoreMode] = useState<"date" | "bookmark">("date");
  const [time, setTime] = useState("");
  const [selectedCheckpointMs, setSelectedCheckpointMs] = useState<
    number | null
  >(null);
  const [bookmark, setBookmark] = useState("");
  const [currentBookmark, setCurrentBookmark] = useState<string | null>(null);
  const [migrationSource, setMigrationSource] = useState(
    '[\n  { "id": 1, "name": "initial", "sha256": "", "sql": "CREATE TABLE example (id INTEGER PRIMARY KEY);" }\n]',
  );

  const database = useQuery(
    d1DatabaseQuery(client, selectedInstanceId, databaseId),
  );
  useEffect(() => {
    if (!database.data?.name) return;
    setDetailBreadcrumb({
      path: `/d1/${databaseId}`,
      name: database.data.name,
    });
    return () => setDetailBreadcrumb(null);
  }, [database.data?.name, databaseId, setDetailBreadcrumb]);
  const backups = useQuery({
    queryKey: [
      "cloudflare-v4",
      "d1",
      selectedInstanceId,
      databaseId,
      "backups",
    ],
    queryFn: ({ signal }) =>
      client!.openCompute.backups.d1.list(selectedInstanceId!, databaseId, {
        signal,
      }),
    enabled,
  });
  const checkpoints = useQuery({
    queryKey: [
      "open-compute",
      "d1",
      selectedInstanceId,
      databaseId,
      "checkpoints",
    ],
    queryFn: ({ signal }) =>
      client!.openCompute.d1.timeTravel.checkpoints(
        selectedInstanceId!,
        databaseId,
        {
          signal,
        },
      ),
    enabled: enabled && tab === "recovery",
  });
  const migrations = useQuery({
    queryKey: [
      "cloudflare-v4",
      "d1",
      selectedInstanceId,
      databaseId,
      "migrations",
    ],
    queryFn: ({ signal }) =>
      client!.openCompute.d1.migrations.list(selectedInstanceId!, databaseId, {
        signal,
      }),
    enabled,
  });
  const execute = useMutation({
    mutationFn: async () => {
      const parsed: unknown = JSON.parse(params);
      if (
        !Array.isArray(parsed) ||
        !parsed.every((item) => typeof item === "string")
      )
        throw new Error("Parameters must be a JSON array of strings.");
      const request = { account_id: selectedInstanceId!, sql, params: parsed };
      return mode === "raw"
        ? (await client!.d1.database.raw(databaseId, request)).result
        : (await client!.d1.database.query(databaseId, request)).result;
    },
    onSuccess: setConsoleResult,
    onError: (error) =>
      feedback.failure(error, "Unable to execute the SQL statement."),
  });
  const exportDatabase = useMutation({
    mutationFn: () =>
      client!.d1.database.export(databaseId, {
        account_id: selectedInstanceId!,
        output_format: "polling",
        ...(exportBookmark.trim()
          ? { current_bookmark: exportBookmark.trim() }
          : {}),
      }),
    onSuccess: (result) => {
      setExportResult(result);
      if (result.at_bookmark) setExportBookmark(result.at_bookmark);
    },
    onError: (error) =>
      feedback.failure(error, "Unable to export the database."),
  });
  const importDatabase = useMutation({
    mutationFn: () => {
      if (importAction === "init")
        return client!.d1.database.import(databaseId, {
          account_id: selectedInstanceId!,
          action: "init",
          etag: importEtag.trim(),
        });
      if (importAction === "ingest")
        return client!.d1.database.import(databaseId, {
          account_id: selectedInstanceId!,
          action: "ingest",
          etag: importEtag.trim(),
          filename: importFilename.trim(),
        });
      return client!.d1.database.import(databaseId, {
        account_id: selectedInstanceId!,
        action: "poll",
        current_bookmark: importBookmark.trim(),
      });
    },
    onSuccess: (result) => {
      setImportResult(result);
      if (result.filename) setImportFilename(result.filename);
      if (result.at_bookmark) setImportBookmark(result.at_bookmark);
    },
    onError: (error) =>
      feedback.failure(error, "Unable to continue the database import."),
  });
  const getBookmark = useMutation({
    mutationFn: () =>
      client!.d1.database.timeTravel.getBookmark(databaseId, {
        account_id: selectedInstanceId!,
      }),
    onSuccess: (result) => {
      setCurrentBookmark(result.bookmark ?? null);
      void checkpoints.refetch();
    },
    onError: (error) =>
      feedback.failure(error, "Unable to retrieve a bookmark."),
  });
  function openRestorePointDialog() {
    openAlert({
      title: `Restore ${database.data?.name ?? "database"}?`,
      description:
        "Restoring this database overwrites its current contents. You cannot cancel once the restore starts.",
      size: "xl",
      contentClassName: "flex max-h-dvh flex-col overflow-y-auto px-8 py-6",
      content: (
        <RestorePointConfirmForm
          canRestore={canRestore}
          databaseName={database.data?.name ?? ""}
          restoreTarget={
            restoreMode === "bookmark"
              ? bookmark.trim()
              : new Date(time).toLocaleString()
          }
          restoreTargetLabel={
            restoreMode === "bookmark" ? "Bookmark ID" : "Date and time"
          }
          submit={async () => {
            try {
              await client!.d1.database.timeTravel.restore(databaseId, {
                account_id: selectedInstanceId!,
                ...(restoreMode === "bookmark"
                  ? { bookmark: bookmark.trim() }
                  : {
                      timestamp: new Date(
                        selectedCheckpointMs ?? time,
                      ).toISOString(),
                    }),
              });
            } catch (error) {
              feedback.failure(error, "Unable to restore the database.");
              throw error;
            }
            setCurrentBookmark(null);
            void database.refetch();
            void checkpoints.refetch();
            feedback.success("Database restored.");
          }}
        />
      ),
    });
  }

  const createBackup = useMutation({
    mutationFn: () =>
      client!.openCompute.backups.d1.create(selectedInstanceId!, databaseId),
    onSuccess: async () => {
      await backups.refetch();
      feedback.success("D1 backup created.");
    },
    onError: (error) => feedback.failure(error, "Unable to create the backup."),
  });
  function openRestoreBackupDialog(backupId: string) {
    openResourceNameDialog({
      title: "Restore D1 backup",
      description: "The backup will be restored into a new database.",
      label: "New database name",
      placeholder: "restored-database",
      submitLabel: "Restore",
      submit: async (name) => {
        try {
          await client!.openCompute.backups.d1.restore(
            selectedInstanceId!,
            backupId,
            { name },
          );
        } catch (error) {
          feedback.failure(error, "Unable to restore the backup.");
          throw error;
        }
        feedback.success("Backup restored into a new database.");
      },
    });
  }

  const applyMigrations = useMutation({
    mutationFn: () =>
      client!.openCompute.d1.migrations.apply(
        selectedInstanceId!,
        databaseId,
        parseMigrations(migrationSource),
      ),
    onSuccess: async () => {
      await migrations.refetch();
      feedback.success("Migrations applied.");
    },
    onError: (error) =>
      feedback.failure(error, "Unable to apply the migrations."),
  });
  function openRenameDatabaseDialog() {
    openResourceNameDialog({
      title: "Rename database",
      description: "The database UUID and bindings do not change.",
      label: "Database name",
      initialValue: database.data?.name ?? "",
      submitLabel: "Rename",
      submit: async (name) => {
        try {
          await client!.openCompute.d1.rename(selectedInstanceId!, databaseId, {
            name,
          });
        } catch (error) {
          feedback.failure(error, "Unable to rename the database.");
          throw error;
        }
        await database.refetch();
        feedback.success("D1 database renamed.");
      },
    });
  }

  function confirmDeleteDatabase() {
    openConfirmDeleteDialog({
      name: database.data?.name ?? "",
      confirm: async () => {
        try {
          await client!.d1.database.delete(databaseId, {
            account_id: selectedInstanceId!,
          });
        } catch (error) {
          feedback.failure(error, "Unable to delete the database.");
          throw error;
        }
        feedback.success("D1 database deleted.");
        await navigate({ to: "/d1" });
      },
    });
  }

  const tabs: readonly [Tab, string][] = [
    ["overview", "Overview"],
    ["console", "Console"],
    ["recovery", "Time travel"],
    ["settings", "Settings"],
    ["transfer", "Import and export"],
    ["migrations", "Migrations"],
  ];
  const canRestore =
    restoreMode === "bookmark"
      ? bookmark.trim().length > 0
      : time.length > 0 && Number.isFinite(new Date(time).getTime());
  return (
    <div>
      <PageHeader
        title={database.data?.name ?? "D1 database"}
        description="Query data, manage backups, and configure this database."
      />
      <div className="mb-8 flex min-w-0 items-start justify-between gap-3">
        <Tabs
          variant="underline"
          value={tab}
          tabs={tabs.map(([value, label]) => ({ value, label }))}
          onValueChange={(value) => setTab(value as Tab)}
        />
        <Button
          variant="primary"
          className="shrink-0"
          aria-label="Explore data"
          onClick={() => setTab("console")}
        >
          <CloudflareProductIcon product="D1" size={16} />
          <span className="hidden sm:inline">Explore data</span>
        </Button>
      </div>
      {database.isLoading ? (
        <LoadingRows />
      ) : database.error ? (
        <ErrorState error={database.error} />
      ) : tab === "overview" ? (
        <div className="grid gap-5">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="ring-kumo-line bg-kumo-base flex min-w-0 items-center gap-3 rounded-lg px-3 py-2 ring">
              <span className="text-kumo-subtle">Database ID</span>
              <code className="truncate text-xs">{databaseId}</code>
              <Button
                variant="ghost"
                shape="square"
                aria-label="Copy database ID"
                onClick={() =>
                  void navigator.clipboard.writeText(databaseId).then(
                    () => feedback.success("Database ID copied."),
                    (error: unknown) =>
                      feedback.failure(
                        error,
                        "Unable to copy the database ID.",
                      ),
                  )
                }
              >
                <IconCopy size={16} />
              </Button>
            </div>
            <Button
              variant="secondary"
              onClick={() => void database.refetch()}
              disabled={database.isFetching}
            >
              <IconRefresh
                size={16}
                className={database.isFetching ? "animate-spin" : ""}
              />
              Refresh
            </Button>
          </div>
          <StatGrid
            items={[
              { label: "Tables", value: database.data?.num_tables ?? "—" },
              {
                label: "Database size",
                value: formatBytes(database.data?.file_size),
              },
              { label: "Version", value: database.data?.version ?? "—" },
            ]}
          />
          <div className="grid gap-6 md:grid-cols-2">
            <Section title="Database details">
              <Panel>
                <DefinitionList
                  items={[
                    { label: "Name", value: database.data?.name ?? "—" },
                    {
                      label: "UUID",
                      value: (
                        <span className="font-mono text-xs">{databaseId}</span>
                      ),
                    },
                    {
                      label: "Created",
                      value: database.data?.created_at
                        ? new Date(database.data.created_at).toLocaleString()
                        : "—",
                    },
                    {
                      label: "Version",
                      value: database.data?.version ?? "—",
                    },
                  ]}
                />
              </Panel>
            </Section>
            <Section
              title="Explore data"
              description="Use the console to inspect tables or run SQL statements."
            >
              <Panel>
                <CloudflareProductIcon
                  product="D1"
                  size={24}
                  className="text-kumo-brand mb-3"
                />
                <Button variant="primary" onClick={() => setTab("console")}>
                  Open console
                </Button>
              </Panel>
            </Section>
          </div>
        </div>
      ) : tab === "console" ? (
        <Section
          title="SQL console"
          description="Run SQL against this database. Raw mode returns column arrays instead of objects."
        >
          <Panel>
            <div className="mb-3 flex flex-wrap gap-2">
              <Button
                variant={mode === "query" ? "primary" : "secondary"}
                onClick={() => setMode("query")}
              >
                Query
              </Button>
              <Button
                variant={mode === "raw" ? "primary" : "secondary"}
                onClick={() => setMode("raw")}
              >
                Raw
              </Button>
            </div>
            <label className="grid gap-1">
              <span className="font-medium">SQL statement</span>
              <Textarea
                className="min-h-40 font-mono text-xs"
                value={sql}
                onChange={(event) => setSql(event.target.value)}
              />
            </label>
            <div className="mt-3">
              <Input
                label="Parameters (JSON string array)"
                value={params}
                onChange={(event) => setParams(event.target.value)}
              />
            </div>
            <div className="mt-4">
              <Button
                variant="primary"
                disabled={!sql.trim() || execute.isPending}
                onClick={() => execute.mutate()}
              >
                <IconPlayerPlay size={16} />
                {execute.isPending ? "Running…" : "Execute"}
              </Button>
            </div>
            {execute.error ? (
              <div className="mt-4">
                <ErrorState error={execute.error} />
              </div>
            ) : null}
          </Panel>
          {consoleResult !== null ? (
            <CodeBlock
              className="bg-kumo-tint ring-kumo-line max-h-128 overflow-auto rounded-lg p-4 font-mono text-xs ring"
              code={JSON.stringify(consoleResult, null, 2)}
              language="json"
            />
          ) : (
            <EmptyState
              title="No query results"
              description="Run a statement to see its result and execution metadata."
            />
          )}
        </Section>
      ) : tab === "transfer" ? (
        <div className="grid gap-6 md:grid-cols-2">
          <Section
            title="Export database"
            description="Start an export, then poll with the returned bookmark until it completes."
          >
            <Panel>
              <Input
                label="Current bookmark (for polling)"
                value={exportBookmark}
                onChange={(event) => setExportBookmark(event.target.value)}
              />
              <div className="mt-4">
                <Button
                  variant="primary"
                  disabled={exportDatabase.isPending}
                  onClick={() => exportDatabase.mutate()}
                >
                  {exportDatabase.isPending
                    ? "Working…"
                    : exportBookmark
                      ? "Poll export"
                      : "Start export"}
                </Button>
              </div>
              {exportDatabase.error ? (
                <div className="mt-4">
                  <ErrorState error={exportDatabase.error} />
                </div>
              ) : null}
              {exportResult !== null ? (
                <CodeBlock
                  className="bg-kumo-tint mt-4 max-h-64 overflow-auto rounded-md p-3 font-mono text-xs"
                  code={JSON.stringify(exportResult, null, 2)}
                  language="json"
                />
              ) : null}
              {typeof exportResult === "object" &&
              exportResult !== null &&
              "result" in exportResult &&
              typeof exportResult.result === "object" &&
              exportResult.result !== null &&
              "signed_url" in exportResult.result &&
              typeof exportResult.result.signed_url === "string" ? (
                <a
                  className="text-kumo-link mt-3 inline-block hover:underline"
                  href={exportResult.result.signed_url}
                >
                  Download SQL export
                </a>
              ) : null}
            </Panel>
          </Section>
          <Section
            title="Import database"
            description="Use init to obtain an upload URL, upload the SQL file, then ingest and poll."
          >
            <Panel>
              <Select
                label="Action"
                value={importAction}
                items={[
                  { label: "Initialize", value: "init" },
                  { label: "Ingest uploaded file", value: "ingest" },
                  { label: "Poll status", value: "poll" },
                ]}
                onValueChange={(value) =>
                  setImportAction(value as ImportAction)
                }
              />
              {importAction !== "poll" ? (
                <div className="mt-3">
                  <Input
                    label="MD5 etag"
                    value={importEtag}
                    onChange={(event) => setImportEtag(event.target.value)}
                  />
                </div>
              ) : null}
              {importAction === "ingest" ? (
                <div className="mt-3">
                  <Input
                    label="Filename"
                    value={importFilename}
                    onChange={(event) => setImportFilename(event.target.value)}
                  />
                </div>
              ) : null}
              {importAction === "poll" ? (
                <div className="mt-3">
                  <Input
                    label="Current bookmark"
                    value={importBookmark}
                    onChange={(event) => setImportBookmark(event.target.value)}
                  />
                </div>
              ) : null}
              <div className="mt-4">
                <Button
                  variant="primary"
                  disabled={
                    importDatabase.isPending ||
                    (importAction === "poll"
                      ? !importBookmark.trim()
                      : !importEtag.trim()) ||
                    (importAction === "ingest" && !importFilename.trim())
                  }
                  onClick={() => importDatabase.mutate()}
                >
                  <IconUpload size={16} />
                  {importDatabase.isPending ? "Working…" : "Continue import"}
                </Button>
              </div>
              {importDatabase.error ? (
                <div className="mt-4">
                  <ErrorState error={importDatabase.error} />
                </div>
              ) : null}
              {importResult !== null ? (
                <CodeBlock
                  className="bg-kumo-tint mt-4 max-h-64 overflow-auto rounded-md p-3 font-mono text-xs"
                  code={JSON.stringify(importResult, null, 2)}
                  language="json"
                />
              ) : null}
            </Panel>
          </Section>
        </div>
      ) : tab === "recovery" ? (
        <div className="mx-auto grid w-full max-w-6xl gap-6">
          <header className="grid gap-2">
            <h1 className="text-2xl font-semibold">Time travel</h1>
            <p>
              Select a date and time or enter a bookmark ID to restore an
              available database checkpoint.
            </p>
          </header>
          <div className="mb-10 grid gap-2">
            <div className="ring-kumo-line bg-kumo-tint text-kumo-subtle flex min-h-8 items-center justify-center rounded-md px-3 py-1 ring">
              {checkpoints.isLoading
                ? "Loading retained checkpoints…"
                : checkpoints.isError
                  ? "Unable to load retained checkpoints"
                  : `${checkpoints.data?.checkpoints_ms.length ?? 0} retained checkpoints`}
            </div>
            <p className="text-kumo-subtle">
              open-compute retains up to eight completed checkpoints, not a
              continuous recovery window. Choose a retained point below or enter
              a bookmark.
            </p>
            {checkpoints.data?.checkpoints_ms.length ? (
              <div className="flex flex-wrap gap-2">
                {checkpoints.data.checkpoints_ms.map((timestamp) => (
                  <Button
                    key={timestamp}
                    variant="secondary"
                    onClick={() => {
                      const date = new Date(timestamp);
                      setTime(
                        new Date(timestamp - date.getTimezoneOffset() * 60000)
                          .toISOString()
                          .slice(0, 23),
                      );
                      setSelectedCheckpointMs(timestamp);
                      setRestoreMode("date");
                    }}
                  >
                    {new Date(timestamp).toLocaleString()}
                  </Button>
                ))}
              </div>
            ) : null}
          </div>
          <LayerCard className="overflow-hidden p-0">
            <div className="border-kumo-line bg-kumo-tint border-b px-6 py-2 font-medium">
              Restore database
            </div>
            <div className="px-6 py-5">
              <Tabs
                variant="segmented"
                value={restoreMode}
                onValueChange={(value) =>
                  setRestoreMode(value === "bookmark" ? "bookmark" : "date")
                }
                tabs={[
                  { value: "date", label: "Date", className: "text-sm" },
                  {
                    value: "bookmark",
                    label: "Bookmark",
                    className: "text-sm",
                  },
                ]}
                className="mb-6 w-fit"
              />
              <div className="max-w-md">
                {restoreMode === "date" ? (
                  <Input
                    label="Choose date and time"
                    type="datetime-local"
                    step={0.001}
                    value={time}
                    onChange={(event) => {
                      setTime(event.target.value);
                      setSelectedCheckpointMs(null);
                    }}
                  />
                ) : (
                  <div className="grid gap-2">
                    <Input
                      label="Bookmark ID"
                      placeholder="Bookmark ID"
                      value={bookmark}
                      onChange={(event) => setBookmark(event.target.value)}
                    />
                    <p className="text-kumo-subtle">
                      Use a bookmark ID to restore your database to an available
                      checkpoint.
                    </p>
                  </div>
                )}
              </div>
            </div>
            <div className="border-kumo-line flex flex-wrap items-center justify-between gap-4 border-t px-6 py-4">
              <p className="text-kumo-subtle flex min-w-0 items-start gap-2">
                <span className="flex h-lh items-center">
                  <IconInfoCircle size={16} aria-hidden="true" />
                </span>
                Restoring to an older version overwrites the current database.
              </p>
              <Button
                variant="destructive"
                disabled={!canRestore}
                onClick={openRestorePointDialog}
              >
                Restore database
              </Button>
            </div>
          </LayerCard>
          <LayerCard className="overflow-hidden p-0">
            <div className="border-kumo-line bg-kumo-tint border-b px-6 py-3 font-medium">
              Get current bookmark ID
            </div>
            <div className="grid gap-4 px-6 py-5">
              <p>
                Get a bookmark for the database’s current state to restore it
                later.
              </p>
              <div>
                <Button
                  variant="secondary"
                  disabled={getBookmark.isPending}
                  onClick={() => getBookmark.mutate()}
                >
                  {getBookmark.isPending
                    ? "Getting bookmark…"
                    : "Get current bookmark ID"}
                </Button>
              </div>
              {getBookmark.error ? (
                <ErrorState error={getBookmark.error} />
              ) : null}
              {currentBookmark !== null ? (
                <div className="ring-kumo-line flex max-w-fit min-w-0 items-center overflow-hidden rounded-md ring">
                  <code className="min-w-0 overflow-x-auto px-3 py-2 text-xs">
                    {currentBookmark}
                  </code>
                  <Button
                    variant="ghost"
                    shape="square"
                    aria-label="Copy current bookmark ID"
                    onClick={() =>
                      void navigator.clipboard.writeText(currentBookmark).then(
                        () => feedback.success("Bookmark ID copied."),
                        (error: unknown) =>
                          feedback.failure(
                            error,
                            "Unable to copy the bookmark ID.",
                          ),
                      )
                    }
                  >
                    <IconCopy size={16} />
                  </Button>
                </div>
              ) : getBookmark.isSuccess ? (
                <p className="text-kumo-subtle">No bookmark ID was returned.</p>
              ) : null}
            </div>
          </LayerCard>
          <Section
            title="Backups · open-compute extension"
            description="Backups restore into a new D1 database."
          >
            <div className="mb-3">
              <Button
                variant="primary"
                disabled={createBackup.isPending}
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
                description="Create a backup to preserve the current database."
              />
            ) : (
              <BackupTable
                backups={backups.data ?? []}
                onRestore={openRestoreBackupDialog}
              />
            )}
          </Section>
        </div>
      ) : tab === "migrations" ? (
        <div className="grid gap-6">
          <Section
            title="Apply migrations"
            description="Migrations are ordered, checksummed SQL records. Existing IDs and hashes must remain stable."
          >
            <Panel>
              <Textarea
                aria-label="Migration JSON"
                className="min-h-52 w-full font-mono text-xs"
                value={migrationSource}
                onChange={(event) => setMigrationSource(event.target.value)}
              />
              <div className="mt-3">
                <Button
                  variant="primary"
                  disabled={
                    !migrationSource.trim() || applyMigrations.isPending
                  }
                  onClick={() => applyMigrations.mutate()}
                >
                  {applyMigrations.isPending ? "Applying…" : "Apply migrations"}
                </Button>
              </div>
              {applyMigrations.error ? (
                <div className="mt-4">
                  <ErrorState error={applyMigrations.error} />
                </div>
              ) : null}
            </Panel>
          </Section>
          <Section title="Applied migrations">
            {migrations.isLoading ? (
              <LoadingRows />
            ) : migrations.error ? (
              <ErrorState error={migrations.error} />
            ) : migrations.data?.length === 0 ? (
              <EmptyState
                title="No migrations"
                description="No managed migrations have been applied."
              />
            ) : (
              <LayerCard className="overflow-hidden p-0">
                <div className="overflow-x-auto">
                  <Table className="min-w-2xl">
                    <Table.Header variant="compact">
                      <Table.Row>
                        <Table.Head>ID</Table.Head>
                        <Table.Head>Name</Table.Head>
                        <Table.Head>SHA-256</Table.Head>
                        <Table.Head>Applied</Table.Head>
                      </Table.Row>
                    </Table.Header>
                    <Table.Body>
                      {migrations.data?.map((migration) => (
                        <Table.Row key={migration.id}>
                          <Table.Cell>{migration.id}</Table.Cell>
                          <Table.Cell>{migration.name}</Table.Cell>
                          <Table.Cell className="font-mono text-xs">
                            {migration.sha256}
                          </Table.Cell>
                          <Table.Cell>
                            {new Date(migration.applied_at_ms).toLocaleString()}
                          </Table.Cell>
                        </Table.Row>
                      ))}
                    </Table.Body>
                  </Table>
                </div>
              </LayerCard>
            )}
          </Section>
        </div>
      ) : (
        <div className="mx-auto grid max-w-4xl gap-6">
          <section>
            <h2 className="mb-3 text-base font-semibold">Database name</h2>
            <LayerCard className="flex flex-wrap items-center justify-between gap-3 px-5 py-4">
              <div className="grid gap-1">
                <span className="font-medium">
                  {database.data?.name ?? "—"}
                </span>
                <span className="text-kumo-subtle">
                  The UUID and Worker bindings do not change.
                </span>
              </div>
              <Button variant="secondary" onClick={openRenameDatabaseDialog}>
                Rename
              </Button>
            </LayerCard>
          </section>
          <section>
            <h2 className="mb-3 text-base font-semibold">Delete database</h2>
            <LayerCard className="flex flex-wrap items-center justify-between gap-3 px-5 py-4">
              <p className="text-kumo-subtle">
                Deleting this database permanently removes its data.
              </p>
              <Button variant="destructive" onClick={confirmDeleteDatabase}>
                Delete
              </Button>
            </LayerCard>
          </section>
        </div>
      )}
    </div>
  );
}

function RestorePointConfirmForm({
  canRestore,
  databaseName,
  restoreTarget,
  restoreTargetLabel,
  submit,
}: {
  canRestore: boolean;
  databaseName: string;
  restoreTarget: string;
  restoreTargetLabel: string;
  submit: () => Promise<void>;
}) {
  const [confirmation, setConfirmation] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);

  async function handleSubmit() {
    if (pending || !canRestore || confirmation !== databaseName) return;
    setPending(true);
    try {
      await submit();
      closeAlert();
    } catch (caught) {
      setError(caught);
    } finally {
      setPending(false);
    }
  }

  return (
    <>
      <div className="mt-4 grid gap-4">
        <p>
          Restore target: {restoreTargetLabel}{" "}
          <span className="font-mono text-xs break-all">{restoreTarget}</span>
        </p>
        <Input
          label={`Type “${databaseName}” to confirm`}
          value={confirmation}
          onChange={(event) => setConfirmation(event.target.value)}
        />
      </div>
      {error ? (
        <div className="mt-4">
          <ErrorState error={error} />
        </div>
      ) : null}
      <div className="border-kumo-line bg-kumo-tint -mx-8 mt-auto -mb-6 flex justify-end gap-2 border-t px-8 py-4">
        <Button
          variant="secondary"
          onClick={() => closeAlert()}
          disabled={pending}
        >
          Cancel
        </Button>
        <Button
          variant="destructive"
          onClick={() => void handleSubmit()}
          disabled={pending || !canRestore || confirmation !== databaseName}
        >
          {pending ? "Restoring…" : "Restore"}
        </Button>
      </div>
    </>
  );
}
