import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import { Tabs } from "@cloudflare/kumo/components/tabs";
import {
  IconArrowRight,
  IconCircleCheck,
  IconExternalLink,
  IconPlayerPlay,
  IconStack2,
  IconTrash,
  IconWorld,
} from "@tabler/icons-react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useAtomValue, useSetAtom } from "jotai";
import { useEffect, useState, type ReactNode } from "react";
import {
  CloudflareProductIcon,
  productIcon,
} from "../../../components/cloudflare-product-icons";
import {
  ErrorState,
  LoadingRows,
  Notice,
  PageHeader,
  Panel,
  Section,
} from "../../../components/dashboard-page";
import { closeDialog, openDialog } from "../../../components/dialog-manager";
import {
  openConfirmDeleteDialog,
  openResourceNameDialog,
} from "../../../components/resource-dialog";
import { WorkerVersionHistory } from "../../../components/worker-version-history";
import { useAuth } from "../../../features/auth/auth-atoms";
import {
  clearLiveTailErrorAtom,
  failLiveTailAtom,
  liveTailAtom,
  prependLiveTailRowAtom,
  selectLiveTailWorkerAtom,
  setLiveTailEnabledAtom,
  setLiveTailStatusAtom,
  type LiveLogRow,
} from "../../../features/observability/live-tail-atoms";
import { useMutationFeedback } from "../../../features/toast/use-mutation-feedback";
import { currentEpochMs, normalizeRfc3339 } from "../../../lib/date-time";
import { workerDeploymentsQuery } from "../../../lib/query-options";
import { WorkerSettings } from "./-worker-settings";

export const Route = createFileRoute("/_authenticated/workers/$workerId")({
  validateSearch: (search: Record<string, unknown>): { tab?: Tab } =>
    search.tab === "deployments" ||
    search.tab === "observability" ||
    search.tab === "settings"
      ? { tab: search.tab }
      : {},
  loader: ({ context, params }) => {
    const { client, instanceId } = context.auth;
    if (!client || !instanceId) return;
    return context.queryClient.ensureQueryData(
      workerDeploymentsQuery(client, instanceId, params.workerId),
    );
  },
  component: WorkerDetailPage,
});

type Tab = "overview" | "deployments" | "observability" | "settings";
type DeleteTarget = { kind: "worker" | "deployment" | "secret"; id: string };
type ListRow = {
  id: string;
  title: ReactNode;
  detail?: ReactNode;
  actions?: ReactNode;
};

function asRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function liveLog(raw: unknown): LiveLogRow | undefined {
  if (!asRecord(raw) || typeof raw.timestamp !== "number") return undefined;
  const metadata = asRecord(raw.$metadata) ? raw.$metadata : {};
  return {
    id: typeof metadata.id === "string" ? metadata.id : crypto.randomUUID(),
    timestamp: normalizeRfc3339(raw.timestamp) ?? "—",
    level:
      typeof metadata.level === "string"
        ? metadata.level
        : typeof metadata.type === "string"
          ? metadata.type
          : "event",
    source:
      typeof raw.source === "string"
        ? raw.source
        : (JSON.stringify(raw.source) ?? "null"),
  };
}

function SecretForm({
  submit,
}: {
  submit: (input: { name: string; value: string }) => Promise<void>;
}) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);

  async function handleSubmit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (pending) return;
    const form = new FormData(event.currentTarget);
    const name = String(form.get("name") ?? "").trim();
    const value = String(form.get("value") ?? "");
    if (!name || !value) return;
    setPending(true);
    try {
      await submit({ name, value });
      closeDialog();
    } catch (caught) {
      setError(caught);
    } finally {
      setPending(false);
    }
  }

  return (
    <form onSubmit={(event) => void handleSubmit(event)}>
      <div className="mt-5 grid gap-4">
        <Input label="Variable name" name="name" required />
        <Input label="Secret value" type="password" name="value" required />
      </div>
      {error ? (
        <p className="text-kumo-danger mt-3">
          {error instanceof Error ? error.message : "The request failed."}
        </p>
      ) : null}
      <div className="mt-6 flex justify-end gap-2">
        <Button
          type="button"
          variant="secondary"
          onClick={() => closeDialog()}
          disabled={pending}
        >
          Cancel
        </Button>
        <Button type="submit" variant="primary" disabled={pending}>
          {pending ? "Saving…" : "Save"}
        </Button>
      </div>
    </form>
  );
}

function ListRows({
  rows,
  empty,
}: {
  rows: readonly ListRow[];
  empty: string;
}) {
  return (
    // prettier-ignore
    <Panel>
      {rows.length ? <div className="divide-kumo-line divide-y">{rows.map((row) => <div className="flex flex-wrap items-center gap-3 py-3 first:pt-0 last:pb-0" key={row.id}><div className="min-w-0 flex-1"><div>{row.title}</div>{row.detail ? <p className="text-kumo-subtle mt-1">{row.detail}</p> : null}</div>{row.actions}</div>)}</div> : <p className="text-kumo-subtle py-4 text-center">{empty}</p>}
    </Panel>
  );
}

function WorkerTopology({
  name,
  endpoints,
  queues,
  bindings,
  active,
  logsEnabled,
  onSettings,
}: {
  name: string;
  endpoints: number;
  queues: number;
  bindings: readonly { type: string; name?: string }[];
  active: boolean;
  logsEnabled: boolean | undefined;
  onSettings: () => void;
}) {
  return (
    <div className="bg-kumo-recessed grid min-h-64 items-center gap-5 rounded-xl p-5 py-8 lg:grid-cols-5">
      <div className="grid gap-2">
        {[
          { label: "Endpoints", count: endpoints, icon: IconWorld },
          { label: "Queues", count: queues, icon: productIcon("Queues") },
        ].map(({ label, count, icon: Icon }) => (
          <div
            key={label}
            className="bg-kumo-base ring-kumo-line flex items-center justify-between rounded-lg px-4 py-3 ring"
          >
            <span className="flex items-center gap-2">
              <Icon size={16} className="text-kumo-subtle" />
              {label}
            </span>
            <span className="bg-kumo-tint rounded px-1.5 text-xs">{count}</span>
          </div>
        ))}
      </div>
      <IconArrowRight
        size={20}
        className="text-kumo-subtle mx-auto hidden lg:block"
      />
      <div className="bg-kumo-base ring-kumo-line rounded-lg p-4 ring">
        <div className="flex min-w-0 items-center gap-2 font-medium">
          <CloudflareProductIcon product="Workers" size={18} />
          <span className="truncate">{name}</span>
          <span
            className={`ml-auto size-2 shrink-0 rounded-full ${active ? "bg-kumo-success" : "bg-kumo-subtle"}`}
            title={active ? "Deployed" : "Not deployed"}
          />
        </div>
        <div className="mt-4 grid gap-2">
          <div className="flex justify-between gap-2">
            <span>Workers Logs</span>
            <span className="text-kumo-subtle flex items-center gap-1">
              {logsEnabled ? (
                <IconCircleCheck size={14} className="text-kumo-success" />
              ) : null}
              {logsEnabled === undefined
                ? "Not configured"
                : logsEnabled
                  ? "Enabled"
                  : "Disabled"}
            </span>
          </div>
          <div className="flex justify-between gap-2">
            <span>Bindings</span>
            <button
              className="text-kumo-link hover:underline"
              onClick={onSettings}
            >
              View {bindings.length}
            </button>
          </div>
        </div>
      </div>
      <IconArrowRight
        size={20}
        className="text-kumo-subtle mx-auto hidden lg:block"
      />
      <div className="bg-kumo-base ring-kumo-line overflow-hidden rounded-lg ring">
        <div className="border-kumo-line flex items-center gap-2 border-b px-4 py-3 font-medium">
          <IconStack2 size={16} /> Bindings
          <span className="bg-kumo-tint ml-auto rounded px-1.5 text-xs">
            {bindings.length}
          </span>
        </div>
        {bindings.length ? (
          <div className="divide-kumo-line divide-y">
            {bindings.slice(0, 3).map((binding, index) => (
              <div
                key={`${binding.name}-${index}`}
                className="flex justify-between gap-2 px-4 py-2"
              >
                <span className="truncate">{binding.name ?? binding.type}</span>
                <span className="text-kumo-subtle truncate">
                  {binding.type}
                </span>
              </div>
            ))}
            {bindings.length > 3 ? (
              <button
                className="text-kumo-link px-4 py-2 hover:underline"
                onClick={onSettings}
              >
                View all bindings
              </button>
            ) : null}
          </div>
        ) : (
          <p className="text-kumo-subtle px-4 py-5">No bindings configured.</p>
        )}
      </div>
    </div>
  );
}

function WorkerDetailPage() {
  const { workerId } = Route.useParams();
  const { tab: searchTab } = Route.useSearch();
  const tab = searchTab ?? "overview";
  const navigate = useNavigate();
  const { client, instanceId: selectedInstanceId } = useAuth();
  const feedback = useMutationFeedback();
  const enabled = client !== null && selectedInstanceId !== null;
  const setTab = (next: Tab) =>
    void navigate({
      to: "/workers/$workerId",
      params: { workerId },
      search: next === "overview" ? {} : { tab: next },
    });
  const [publicDraft, setPublicDraft] = useState(workerId);
  const liveTail = useAtomValue(liveTailAtom);
  const selectLiveWorker = useSetAtom(selectLiveTailWorkerAtom);
  const setLiveEnabled = useSetAtom(setLiveTailEnabledAtom);
  const setLiveStatus = useSetAtom(setLiveTailStatusAtom);
  const failLiveTail = useSetAtom(failLiveTailAtom);
  const clearLiveError = useSetAtom(clearLiveTailErrorAtom);
  const prependLiveRow = useSetAtom(prependLiveTailRowAtom);

  useEffect(() => selectLiveWorker(workerId), [selectLiveWorker, workerId]);

  const deployments = useQuery(
    workerDeploymentsQuery(client, selectedInstanceId, workerId),
  );
  const versions = useQuery({
    queryKey: [
      "cloudflare-v4",
      "workers",
      selectedInstanceId,
      workerId,
      "versions",
    ],
    queryFn: ({ signal }) =>
      client!.workers.scripts.versions.list(
        workerId,
        { account_id: selectedInstanceId!, deployable: true },
        { signal },
      ),
    enabled: enabled && tab === "deployments",
    staleTime: 0,
  });
  const endpoints = useQuery({
    queryKey: [
      "cloudflare-v4",
      "workers",
      selectedInstanceId,
      workerId,
      "endpoints",
    ],
    queryFn: ({ signal }) =>
      client!.openCompute.workers.endpoints(selectedInstanceId!, workerId, {
        signal,
      }),
    enabled,
  });
  const publicOrigin = useQuery({
    queryKey: [
      "cloudflare-v4",
      "workers",
      selectedInstanceId,
      workerId,
      "public-origin",
    ],
    queryFn: ({ signal }) =>
      client!.openCompute.workers.publicOrigin.get(
        selectedInstanceId!,
        workerId,
        {
          signal,
        },
      ),
    enabled,
  });
  const queueConsumers = useQuery({
    queryKey: [
      "cloudflare-v4",
      "workers",
      selectedInstanceId,
      workerId,
      "queue-consumers",
    ],
    queryFn: ({ signal }) =>
      client!.openCompute.workers.queueConsumers(
        selectedInstanceId!,
        workerId,
        {
          signal,
        },
      ),
    enabled,
  });
  const versionSettings = useQuery({
    queryKey: [
      "cloudflare-v4",
      "workers",
      selectedInstanceId,
      workerId,
      "version-settings",
    ],
    queryFn: ({ signal }) =>
      client!.workers.scripts.scriptAndVersionSettings.get(
        workerId,
        { account_id: selectedInstanceId! },
        { signal },
      ),
    enabled,
  });
  const settings = useQuery({
    queryKey: [
      "cloudflare-v4",
      "workers",
      selectedInstanceId,
      workerId,
      "settings",
    ],
    queryFn: ({ signal }) =>
      client!.workers.scripts.settings.get(
        workerId,
        { account_id: selectedInstanceId! },
        { signal },
      ),
    enabled: enabled && (tab === "overview" || tab === "settings"),
  });
  const secrets = useQuery({
    queryKey: [
      "cloudflare-v4",
      "workers",
      selectedInstanceId,
      workerId,
      "secrets",
    ],
    queryFn: ({ signal }) =>
      client!.workers.scripts.secrets.list(
        workerId,
        { account_id: selectedInstanceId! },
        { signal },
      ),
    enabled: enabled && tab === "settings",
  });
  const schedules = useQuery({
    queryKey: [
      "cloudflare-v4",
      "workers",
      selectedInstanceId,
      workerId,
      "schedules",
    ],
    queryFn: ({ signal }) =>
      client!.workers.scripts.schedules.get(
        workerId,
        { account_id: selectedInstanceId! },
        { signal },
      ),
    enabled: enabled && tab === "settings",
  });
  const logs = useQuery({
    queryKey: [
      "cloudflare-v4",
      "workers",
      selectedInstanceId,
      workerId,
      "logs",
    ],
    queryFn: ({ signal }) => {
      const to = currentEpochMs();
      return client!.workers.observability.telemetry.query(
        {
          account_id: selectedInstanceId!,
          queryId: `dashboard-${workerId}`,
          timeframe: { from: to - 3_600_000, to },
          view: "events",
          limit: 100,
          parameters: {
            datasets: ["cloudflare-workers"],
            filters: [
              {
                kind: "filter",
                key: "$workers.scriptName",
                operation: "eq",
                type: "string",
                value: workerId,
              },
            ],
          },
        },
        { signal },
      );
    },
    enabled: enabled && tab === "observability",
    refetchInterval: 10_000,
  });

  useEffect(() => {
    if (
      !enabled ||
      tab !== "observability" ||
      !liveTail.enabled ||
      !client ||
      !selectedInstanceId
    )
      return;
    const abort = new AbortController();
    let socket: WebSocket | undefined;
    let heartbeat: ReturnType<typeof setInterval> | undefined;
    let disposed = false;
    const fail = (message: string) => {
      if (!disposed) failLiveTail(message);
      socket?.close(1011, "Live Tail stopped");
    };
    const beat = () =>
      client.workers.observability.telemetry
        .liveTailHeartbeat(
          { account_id: selectedInstanceId, scriptId: workerId },
          { signal: abort.signal },
        )
        .catch(() => fail("Live Tail heartbeat failed."));
    clearLiveError();
    setLiveStatus("connecting");
    client.workers.observability.telemetry
      .liveTail(
        {
          account_id: selectedInstanceId,
          scriptId: workerId,
          filterCombination: "and",
          filters: [
            {
              key: "$workers.preview.slug",
              operation: "is_null",
              type: "string",
            },
          ],
        },
        { signal: abort.signal },
      )
      .then((prepared) => {
        if (disposed) return;
        socket = new WebSocket(prepared.wsUrl);
        socket.addEventListener("open", () => {
          setLiveStatus("live");
          void beat();
          heartbeat = setInterval(() => void beat(), 15_000);
        });
        socket.addEventListener("message", (event) => {
          try {
            const row = liveLog(JSON.parse(String(event.data)));
            if (row) prependLiveRow(row);
          } catch {
            fail("Live Tail returned an invalid event.");
          }
        });
        socket.addEventListener("error", () =>
          fail("Live Tail connection failed."),
        );
        socket.addEventListener("close", () => {
          if (!disposed) fail("Live Tail connection closed.");
        });
      })
      .catch(() => fail("Unable to start Live Tail."));
    return () => {
      disposed = true;
      abort.abort();
      if (heartbeat) clearInterval(heartbeat);
      socket?.close(1000, "Live Tail stopped");
    };
  }, [
    selectedInstanceId,
    clearLiveError,
    client,
    enabled,
    failLiveTail,
    liveTail.enabled,
    prependLiveRow,
    setLiveStatus,
    tab,
    workerId,
  ]);

  const promote = useMutation({
    mutationFn: ({
      versionId,
      message,
    }: {
      versionId: string;
      message: string;
    }) =>
      client!.workers.scripts.deployments.create(workerId, {
        account_id: selectedInstanceId!,
        strategy: "percentage",
        versions: [{ version_id: versionId, percentage: 100 }],
        ...(message.trim()
          ? { annotations: { "workers/message": message.trim() } }
          : {}),
      }),
    onSuccess: async () => {
      await Promise.all([deployments.refetch(), versions.refetch()]);
      feedback.success("Version promoted.");
    },
    onError: (error) =>
      feedback.failure(error, "Unable to promote the version."),
  });
  function confirmDeleteWorker(target: DeleteTarget) {
    openConfirmDeleteDialog({
      name: target.id,
      confirm: async () => {
        let deleted: unknown;
        try {
          if (target.kind === "worker")
            deleted = await client!.workers.scripts.delete(workerId, {
              account_id: selectedInstanceId!,
            });
          else if (target.kind === "deployment")
            deleted = await client!.workers.scripts.deployments.delete(
              target.id,
              {
                account_id: selectedInstanceId!,
                script_name: workerId,
              },
            );
          else
            deleted = await client!.workers.scripts.secrets.delete(target.id, {
              account_id: selectedInstanceId!,
              script_name: workerId,
            });
          void deleted;
        } catch (error) {
          feedback.failure(error, "Unable to delete the resource.");
          throw error;
        }
        if (target.kind === "worker") {
          feedback.success("Worker deleted.");
          await navigate({ to: "/workers" });
          return;
        }
        await (target.kind === "deployment"
          ? deployments.refetch()
          : secrets.refetch());
        feedback.success(
          target.kind === "secret" ? "Secret deleted." : "Deployment deleted.",
        );
      },
    });
  }

  function openAddSecretDialog() {
    openDialog({
      title: "Add secret",
      description:
        "The value is encrypted and cannot be read after it is saved.",
      size: "lg",
      contentClassName: "px-6 py-5",
      content: <SecretForm submit={saveSecret} />,
    });
  }

  async function saveSecret({ name, value }: { name: string; value: string }) {
    try {
      await client!.workers.scripts.secrets.update(workerId, {
        account_id: selectedInstanceId!,
        name,
        text: value,
        type: "secret_text",
      });
    } catch (error) {
      feedback.failure(error, "Unable to save secret.");
      throw error;
    }
    await secrets.refetch();
    feedback.success("Secret saved.");
  }

  async function updateSchedules(items: string[]) {
    try {
      await client!.workers.scripts.schedules.update(workerId, {
        account_id: selectedInstanceId!,
        body: items.map((cron) => ({ cron })),
      });
    } catch (error) {
      feedback.failure(error, "Unable to update schedules.");
      throw error;
    }
    await schedules.refetch();
    feedback.success("Schedules updated.");
  }

  function openAddScheduleDialog() {
    openResourceNameDialog({
      title: "Add schedule",
      description: "Enter a five-field cron expression.",
      label: "Cron expression",
      placeholder: "0 * * * *",
      submitLabel: "Add schedule",
      submit: async (cron) => {
        await updateSchedules([
          ...(schedules.data?.schedules.map((item) => item.cron) ?? []),
          cron,
        ]);
      },
    });
  }

  const saveOrigin = useMutation({
    mutationFn: (name: string | null) =>
      name === null
        ? client!.openCompute.workers.publicOrigin.delete(
            selectedInstanceId!,
            workerId,
          )
        : client!.openCompute.workers.publicOrigin.set(
            selectedInstanceId!,
            workerId,
            {
              name,
            },
          ),
    onSuccess: async () => {
      await Promise.all([publicOrigin.refetch(), endpoints.refetch()]);
      feedback.success("Public origin updated.");
    },
    onError: (error) =>
      feedback.failure(error, "Unable to save public origin."),
  });
  const toggleObservability = useMutation({
    mutationFn: (value: boolean) =>
      client!.workers.scripts.settings.edit(workerId, {
        account_id: selectedInstanceId!,
        observability: {
          enabled: value,
          logs: { enabled: value, invocation_logs: value, persist: value },
        },
      }),
    onSuccess: async () => {
      await settings.refetch();
      feedback.success("Observability settings updated.");
    },
    onError: (error) =>
      feedback.failure(error, "Unable to update observability."),
  });

  const activeDeployment = deployments.data?.deployments[0];
  const publicUrl =
    publicOrigin.data?.url ??
    endpoints.data?.find((item) => item.kind === "local_origin")?.url;
  const loading =
    deployments.isLoading ||
    endpoints.isLoading ||
    queueConsumers.isLoading ||
    versionSettings.isLoading ||
    (tab === "overview" && settings.isLoading);
  const loadError =
    deployments.error ??
    endpoints.error ??
    queueConsumers.error ??
    versionSettings.error ??
    (tab === "overview" ? settings.error : null);

  return (
    <>
      <PageHeader
        title={workerId}
        description="Worker service"
        actions={
          <>
            <Button
              variant="secondary"
              disabled={!publicUrl}
              onClick={() =>
                publicUrl &&
                window.open(publicUrl, "_blank", "noopener,noreferrer")
              }
            >
              <IconExternalLink size={16} />
              Visit
            </Button>
            <Button
              variant="destructive"
              onClick={() =>
                confirmDeleteWorker({ kind: "worker", id: workerId })
              }
            >
              <IconTrash size={16} />
              Delete
            </Button>
          </>
        }
      />
      <div className="mb-6 overflow-x-auto">
        <Tabs
          variant="underline"
          tabs={[
            { value: "overview", label: "Overview" },
            { value: "deployments", label: "Deployments" },
            { value: "observability", label: "Observability" },
            { value: "settings", label: "Settings" },
          ]}
          value={tab}
          onValueChange={(value) => setTab(value as Tab)}
        />
      </div>
      {loading ? (
        <LoadingRows />
      ) : loadError ? (
        <ErrorState error={loadError} />
      ) : tab === "overview" ? (
        <div className="grid gap-5">
          <Panel className="flex flex-wrap items-center justify-between gap-3 py-3">
            <div className="flex min-w-0 items-center gap-2">
              <IconWorld size={18} className="text-kumo-subtle shrink-0" />
              {publicUrl ? (
                <a
                  className="text-kumo-link truncate hover:underline"
                  href={publicUrl}
                  target="_blank"
                  rel="noreferrer"
                >
                  {publicUrl}
                </a>
              ) : (
                <span className="text-kumo-subtle">No reachable endpoint</span>
              )}
            </div>
            <span className="text-kumo-subtle text-sm">
              {activeDeployment
                ? `Active deployment ${activeDeployment.id.slice(0, 8)}`
                : "Not deployed"}
            </span>
          </Panel>
          <WorkerTopology
            name={workerId}
            endpoints={endpoints.data?.length ?? 0}
            queues={queueConsumers.data?.length ?? 0}
            bindings={versionSettings.data?.bindings ?? []}
            active={activeDeployment !== undefined}
            logsEnabled={settings.data?.observability?.logs?.enabled}
            onSettings={() => setTab("settings")}
          />
          <div className="grid items-start gap-5 xl:grid-cols-3">
            <div className="grid gap-5 xl:col-span-2">
              <Section title="Deployments">
                <ListRows
                  rows={(deployments.data?.deployments ?? [])
                    .slice(0, 1)
                    .map((item) => ({
                      id: item.id,
                      title: (
                        <code className="text-xs">{item.id.slice(0, 8)}</code>
                      ),
                      detail: item.created_on,
                      actions: (
                        <Button
                          variant="secondary"
                          onClick={() => setTab("deployments")}
                        >
                          View all
                        </Button>
                      ),
                    }))}
                  empty="No deployments found."
                />
              </Section>
              <Section title="Queue consumers">
                {/* prettier-ignore */}
                <ListRows rows={(queueConsumers.data ?? []).map((consumer) => ({ id: consumer.consumer_id, title: consumer.queue_name, detail: `Batch ${consumer.settings.batch_size} · ${consumer.settings.max_retries} retries` }))} empty="No Queues currently deliver to this Worker." />
              </Section>
            </div>
            <Section title="Domains and routes">
              {/* prettier-ignore */}
              <ListRows rows={(endpoints.data ?? []).map((endpoint) => ({ id: endpoint.id, title: endpoint.kind === "public_origin" ? "Public origin" : "Local origin", detail: <a className="text-kumo-link break-all hover:underline" href={endpoint.url}>{endpoint.url}</a> }))} empty="No Worker endpoints are reachable." />
            </Section>
          </div>
        </div>
      ) : tab === "deployments" ? (
        <div className="grid gap-6">
          <Section
            title="Deployments"
            description="The first deployment is actively serving traffic."
          >
            {/* prettier-ignore */}
            <ListRows rows={(deployments.data?.deployments ?? []).map((item, index) => ({ id: item.id, title: <code className="text-xs">{item.id}</code>, detail: `${item.created_on} · ${item.versions.map((version) => `${version.percentage}% ${version.version_id.slice(0, 8)}`).join(", ")}`, actions: index === 0 ? <span className="text-kumo-success font-medium">Active</span> : <><Button variant="secondary" disabled={promote.isPending} onClick={() => { const versionId = item.versions[0]?.version_id; if (versionId) promote.mutate({ versionId, message: `Promote ${versionId.slice(0, 8)}` }); }}><IconPlayerPlay size={16} />Promote</Button><Button variant="ghost" shape="square" aria-label={`Delete deployment ${item.id}`} onClick={() => confirmDeleteWorker({ kind: "deployment", id: item.id })}><IconTrash size={16} /></Button></> }))} empty="No deployments found." />
          </Section>
          {versions.isLoading ? (
            <LoadingRows />
          ) : versions.error ? (
            <ErrorState error={versions.error} />
          ) : (
            <WorkerVersionHistory
              versions={(versions.data?.result.items ?? [])
                .filter((version) => version.id)
                .map((version, index) => ({
                  id: version.id!,
                  createdOn: version.metadata?.created_on ?? "",
                  ...(index === 0 &&
                  versionSettings.data?.annotations?.["workers/message"]
                    ? {
                        message:
                          versionSettings.data.annotations["workers/message"],
                      }
                    : {}),
                }))}
              activeVersionId={
                activeDeployment?.versions[0]?.version_id ?? null
              }
              pending={promote.isPending}
              onPromote={(versionId, message) =>
                promote.mutateAsync({ versionId, message }).then(() => {})
              }
            />
          )}
        </div>
      ) : tab === "observability" ? (
        <div className="grid gap-6">
          <Section
            title="Live Tail"
            description="Stream new events from this Worker."
          >
            <Panel className="grid gap-4">
              <div className="flex flex-wrap items-center justify-between gap-3">
                <span className="font-medium">Status: {liveTail.status}</span>
                <Button
                  variant={liveTail.enabled ? "secondary" : "primary"}
                  onClick={() => {
                    if (!liveTail.enabled) clearLiveError();
                    setLiveEnabled(!liveTail.enabled);
                  }}
                >
                  {liveTail.enabled ? "Stop Live Tail" : "Start Live Tail"}
                </Button>
              </div>
              {liveTail.error ? (
                <Notice tone="danger">{liveTail.error}</Notice>
              ) : null}
              <EventRows
                rows={liveTail.rows}
                empty="Start Live Tail to stream events."
              />
            </Panel>
          </Section>
          <Section
            title="Workers Logs"
            description="Persisted events from the last hour."
          >
            {logs.isLoading ? (
              <LoadingRows count={3} />
            ) : logs.error ? (
              <ErrorState error={logs.error} />
            ) : (
              <Panel>
                <EventRows
                  rows={(logs.data?.events?.events ?? []).map(
                    (event, index) => ({
                      id: event.$metadata.id ?? `${event.timestamp}-${index}`,
                      timestamp: normalizeRfc3339(event.timestamp) ?? "—",
                      level:
                        event.$metadata.level ??
                        event.$metadata.type ??
                        "event",
                      source:
                        typeof event.source === "string"
                          ? event.source
                          : JSON.stringify(event.source),
                    }),
                  )}
                  empty="No events in this time range."
                />
              </Panel>
            )}
          </Section>
        </div>
      ) : (
        <WorkerSettings
          workerId={workerId}
          bindings={versionSettings.data?.bindings ?? []}
          secrets={secrets.data?.result ?? []}
          secretsLoading={secrets.isLoading}
          secretsError={secrets.error}
          schedules={schedules.data?.schedules ?? []}
          queueConsumers={queueConsumers.data ?? []}
          observabilityEnabled={settings.data?.observability?.enabled !== false}
          observabilityLoading={settings.isLoading}
          observabilityPending={toggleObservability.isPending}
          compatibilityDate={versionSettings.data?.compatibility_date}
          compatibilityFlags={versionSettings.data?.compatibility_flags}
          cpuTimeLimit={versionSettings.data?.limits?.cpu_ms}
          publicOrigin={publicOrigin.data?.url}
          publicDraft={publicDraft}
          onPublicDraftChange={setPublicDraft}
          publicOriginPending={saveOrigin.isPending}
          onSavePublicOrigin={() => saveOrigin.mutate(publicDraft)}
          onDisablePublicOrigin={() => saveOrigin.mutate(null)}
          onAddSecret={openAddSecretDialog}
          onDeleteSecret={(name) =>
            confirmDeleteWorker({ kind: "secret", id: name })
          }
          onAddSchedule={openAddScheduleDialog}
          onDeleteSchedule={(cron) =>
            void updateSchedules(
              (schedules.data?.schedules ?? [])
                .filter((item) => item.cron !== cron)
                .map((item) => item.cron),
            )
          }
          onToggleObservability={(value) => toggleObservability.mutate(value)}
          onDeleteWorker={() =>
            confirmDeleteWorker({ kind: "worker", id: workerId })
          }
        />
      )}
    </>
  );
}

function EventRows({
  rows,
  empty,
}: {
  rows: readonly LiveLogRow[];
  empty: string;
}) {
  if (!rows.length)
    return <p className="text-kumo-subtle py-4 text-center">{empty}</p>;
  return (
    <div className="divide-kumo-line max-h-128 divide-y overflow-auto">
      {rows.map((row) => (
        <div className="grid gap-1 py-3 sm:grid-cols-3 sm:gap-3" key={row.id}>
          <span className="text-kumo-subtle">{row.timestamp}</span>
          <span className="text-kumo-link">{row.level}</span>
          <span className="break-all">{row.source}</span>
        </div>
      ))}
    </div>
  );
}
