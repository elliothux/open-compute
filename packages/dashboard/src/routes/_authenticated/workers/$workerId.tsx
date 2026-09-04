import { useEffect, useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { Button } from "@cloudflare/kumo/components/button";
import { ConfirmActionDialog } from "../../../components/ConfirmActionDialog";
import { ConfirmDeleteResourceDialog } from "../../../components/ConfirmDeleteResourceDialog";
import { DataTable, ErrorState, LoadingState, PageHeader, SectionHeader, StatusBadge } from "../../../components/PageLayout";
import { useAuth } from "../../../features/auth/AuthProvider";
import { useMutationFeedback } from "../../../features/toast/useMutationFeedback";

export const Route = createFileRoute("/_authenticated/workers/$workerId")({ component: WorkerDetailPage });

interface LiveLogRow extends Record<string, string> {
  id: string;
  timestamp: string;
  level: string;
  source: string;
}

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function displaySource(value: unknown): string {
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value) ?? "null";
  } catch {
    return "[unavailable]";
  }
}

function liveLog(raw: unknown, fallbackID: string): LiveLogRow | undefined {
  if (!record(raw) || typeof raw.timestamp !== "number" || !Number.isFinite(raw.timestamp)) return undefined;
  const metadata = record(raw.$metadata) ? raw.$metadata : {};
  const id = typeof metadata.id === "string" ? metadata.id : fallbackID;
  const level = typeof metadata.level === "string"
    ? metadata.level
    : typeof metadata.type === "string" ? metadata.type : "event";
  return {
    id,
    timestamp: new Date(raw.timestamp).toISOString(),
    level,
    source: displaySource(raw.source),
  };
}

function WorkerDetailPage() {
  const { workerId } = Route.useParams();
  const navigate = useNavigate();
  const { client, accountId } = useAuth();
  const feedback = useMutationFeedback();
  const enabled = client !== null && accountId !== null;
  const [activateTarget, setActivateTarget] = useState<string | null>(null);
  const [deleteDeploymentTarget, setDeleteDeploymentTarget] = useState<string | null>(null);
  const [deleteWorkerOpen, setDeleteWorkerOpen] = useState(false);
  const [mutationError, setMutationError] = useState<string | null>(null);
  const [liveEnabled, setLiveEnabled] = useState(false);
  const [liveStatus, setLiveStatus] = useState<"idle" | "connecting" | "live" | "error">("idle");
  const [liveError, setLiveError] = useState<string | null>(null);
  const [liveRows, setLiveRows] = useState<LiveLogRow[]>([]);
  const deployments = useQuery({
    queryKey: ["cloudflare-v4", "workers", workerId, "deployments"],
    queryFn: ({ signal }) => client!.cloudflare.workers.scripts.deployments.list(workerId, { account_id: accountId! }, { signal }),
    enabled,
  });
  const versions = useQuery({
    queryKey: ["cloudflare-v4", "workers", workerId, "versions"],
    queryFn: ({ signal }) => client!.cloudflare.workers.scripts.versions.list(workerId, { account_id: accountId! }, { signal }),
    enabled,
  });
  const endpoints = useQuery({
    queryKey: ["cloudflare-v4", "workers", workerId, "endpoints"],
    queryFn: ({ signal }) => client!.openCompute.workers.endpoints(accountId!, workerId, { signal }),
    enabled,
  });
  const systemStatus = useQuery({
    queryKey: ["cloudflare-v4", "open-compute", "system-status"],
    queryFn: ({ signal }) => client!.openCompute.system.status({ signal }),
    enabled,
  });
  const logs = useQuery({
    queryKey: ["cloudflare-v4", "workers", workerId, "logs"],
    queryFn: ({ signal }) => {
      const to = Date.now();
      const observability = systemStatus.data?.observability;
      const timeframe = Math.max(1, Math.floor(0.9 * Math.min(
        60 * 60 * 1_000,
        observability?.retention_ms ?? 60 * 60 * 1_000,
        observability?.query_max_timeframe_ms ?? 60 * 60 * 1_000,
      ) / 2));
      return client!.cloudflare.workers.observability.telemetry.query({
        account_id: accountId!,
        queryId: `dashboard-worker-${workerId}`,
        timeframe: { from: to - timeframe, to },
        view: "events",
        limit: 100,
        parameters: {
          datasets: ["cloudflare-workers"],
          filters: [{
            kind: "filter",
            key: "$workers.scriptName",
            operation: "eq",
            type: "string",
            value: workerId,
          }],
        },
      }, { signal });
    },
    enabled: enabled && systemStatus.data !== undefined,
    refetchInterval: 10_000,
  });
  useEffect(() => {
    if (!enabled || !liveEnabled || client === null || accountId === null) return;
    const abort = new AbortController();
    let disposed = false;
    let failed = false;
    let socket: WebSocket | undefined;
    let heartbeat: ReturnType<typeof setInterval> | undefined;
    const stopWithError = (message: string) => {
      if (disposed || failed) return;
      failed = true;
      setLiveError(message);
      setLiveStatus("error");
      setLiveEnabled(false);
      socket?.close(1011, "Live Tail stopped");
    };
    const sendHeartbeat = async () => {
      try {
        await client.cloudflare.workers.observability.telemetry.liveTailHeartbeat({
          account_id: accountId,
          scriptId: workerId,
        }, { signal: abort.signal });
      } catch {
        stopWithError("Live Tail eligibility heartbeat failed.");
      }
    };
    const connect = async () => {
      setLiveError(null);
      setLiveStatus("connecting");
      try {
        const prepared = await client.cloudflare.workers.observability.telemetry.liveTail({
          account_id: accountId,
          scriptId: workerId,
          filterCombination: "and",
          filters: [{
            key: "$workers.preview.slug",
            operation: "is_null",
            type: "string",
          }],
        }, { signal: abort.signal });
        if (disposed) return;
        socket = new WebSocket(prepared.wsUrl);
        socket.addEventListener("open", () => {
          if (disposed) return;
          setLiveStatus("live");
          void sendHeartbeat();
          heartbeat = setInterval(() => void sendHeartbeat(), 15_000);
        });
        socket.addEventListener("message", event => {
          try {
            const row = liveLog(JSON.parse(String(event.data)), crypto.randomUUID());
            if (row !== undefined) setLiveRows(current => [row, ...current].slice(0, 100));
          } catch {
            stopWithError("Live Tail returned an invalid event.");
          }
        });
        socket.addEventListener("error", () => stopWithError("Live Tail connection failed."));
        socket.addEventListener("close", event => {
          if (!disposed && !failed) {
            stopWithError(event.code === 1013
              ? "Live Tail stopped because this browser was too slow."
              : "Live Tail connection closed.");
          }
        });
      } catch {
        stopWithError("Unable to start Live Tail.");
      }
    };
    void connect();
    return () => {
      disposed = true;
      abort.abort();
      if (heartbeat !== undefined) clearInterval(heartbeat);
      socket?.close(1000, "Live Tail stopped");
    };
  }, [accountId, client, enabled, liveEnabled, workerId]);
  const activateMutation = useMutation({
    mutationFn: (deploymentID: string) => {
      const deployment = deployments.data?.deployments.find(item => item.id === deploymentID);
      if (!deployment) throw new Error("The selected deployment is no longer available.");
      return client!.cloudflare.workers.scripts.deployments.create(workerId, {
        account_id: accountId!,
        strategy: "percentage",
        versions: deployment.versions.map(version => ({
          version_id: version.version_id,
          percentage: version.percentage,
        })),
        annotations: { "workers/message": `Activate deployment ${deploymentID}` },
      });
    },
    onSuccess: async () => {
      setActivateTarget(null);
      setMutationError(null);
      await deployments.refetch();
      feedback.success("Worker deployment activated.");
    },
    onError: error => {
      setMutationError(error instanceof Error ? error.message : "Unable to activate the deployment.");
      feedback.failure(error, "Unable to activate the deployment.");
    },
  });
  const deleteDeploymentMutation = useMutation({
    mutationFn: (deploymentID: string) => client!.cloudflare.workers.scripts.deployments.delete(deploymentID, {
      account_id: accountId!,
      script_name: workerId,
    }),
    onSuccess: async () => {
      setDeleteDeploymentTarget(null);
      setMutationError(null);
      await deployments.refetch();
      feedback.success("Inactive Worker deployment deleted.");
    },
    onError: error => {
      setMutationError(error instanceof Error ? error.message : "Unable to delete the deployment.");
      feedback.failure(error, "Unable to delete the deployment.");
    },
  });
  const deleteWorkerMutation = useMutation({
    mutationFn: () => client!.cloudflare.workers.scripts.delete(workerId, { account_id: accountId! }),
    onSuccess: async () => {
      feedback.success("Worker deleted.");
      await navigate({ to: "/workers" });
    },
    onError: error => {
      setMutationError(error instanceof Error ? error.message : "Unable to delete the Worker.");
      feedback.failure(error, "Unable to delete the Worker.");
    },
  });
  const activeDeploymentID = deployments.data?.deployments[0]?.id;
  return <div>
    <PageHeader
      title={workerId}
      description="Deploy code and versions with the pinned Wrangler client; manage deployment traffic through the official Workers API."
      actions={<Button variant="destructive" onClick={() => setDeleteWorkerOpen(true)}>Delete Worker</Button>}
    />
    <ConfirmActionDialog
      title="Activate Worker deployment"
      description="Create a new active deployment with the selected deployment's version percentages."
      resourceLabel="deployment ID"
      confirmValue={activateTarget ?? ""}
      submitLabel="Activate deployment"
      open={activateTarget !== null}
      errorMessage={activateTarget ? mutationError : null}
      isPending={activateMutation.isPending}
      onClose={() => { setActivateTarget(null); setMutationError(null); }}
      onConfirm={() => { if (activateTarget) activateMutation.mutate(activateTarget); }}
    />
    <ConfirmDeleteResourceDialog
      title="Delete inactive deployment"
      description="The official API refuses deletion of the active deployment."
      resourceLabel="deployment ID"
      confirmValue={deleteDeploymentTarget ?? ""}
      open={deleteDeploymentTarget !== null}
      errorMessage={deleteDeploymentTarget ? mutationError : null}
      isPending={deleteDeploymentMutation.isPending}
      onClose={() => { setDeleteDeploymentTarget(null); setMutationError(null); }}
      onConfirm={() => { if (deleteDeploymentTarget) deleteDeploymentMutation.mutate(deleteDeploymentTarget); }}
    />
    <ConfirmDeleteResourceDialog
      title="Delete Worker"
      description="This deletes the Worker script through the official Workers API."
      resourceLabel="Worker name"
      confirmValue={workerId}
      open={deleteWorkerOpen}
      errorMessage={deleteWorkerOpen ? mutationError : null}
      isPending={deleteWorkerMutation.isPending}
      onClose={() => { setDeleteWorkerOpen(false); setMutationError(null); }}
      onConfirm={() => deleteWorkerMutation.mutate()}
    />
    {deployments.isLoading || versions.isLoading || endpoints.isLoading ? <LoadingState /> : deployments.error || versions.error || endpoints.error ? <ErrorState message="Unable to load Worker details." /> : <>
      <div className="mb-6">
        <div className="mb-4 flex flex-wrap items-start justify-between gap-4">
          <div>
            <SectionHeader title="Live Tail" description="Stream new events for this Worker through the official Telemetry Live Tail API. The stream is process-local and is not replayed." />
            <StatusBadge value={liveStatus} />
          </div>
          <Button
            variant={liveEnabled ? "secondary" : "primary"}
            onClick={() => {
              if (!liveEnabled) {
                setLiveRows([]);
                setLiveError(null);
              }
              setLiveEnabled(value => !value);
              if (liveEnabled) setLiveStatus("idle");
            }}
          >{liveEnabled ? "Stop Live Tail" : "Start Live Tail"}</Button>
        </div>
        {liveError ? <ErrorState message={liveError} /> : <DataTable columns={[
          { key: "timestamp", label: "Timestamp" },
          { key: "level", label: "Level" },
          { key: "source", label: "Event" },
        ]} rows={liveRows} emptyLabel="Start Live Tail to stream new events." />}
      </div>
      <SectionHeader title="Workers Logs" description="The latest 100 persisted events from the configured recent query window. Logs can contain application data; do not log secrets." />
      {systemStatus.isLoading || logs.isLoading ? <LoadingState label="Loading Workers Logs…" /> : systemStatus.error || logs.error ? <ErrorState message="Persisted Workers Logs are unavailable." /> : <DataTable columns={[
        { key: "timestamp", label: "Timestamp" },
        { key: "level", label: "Level" },
        { key: "source", label: "Event" },
      ]} rows={(logs.data?.events?.events ?? []).map((event, index) => ({
        id: event.$metadata.id ?? `${event.timestamp}-${index}`,
        timestamp: new Date(event.timestamp).toISOString(),
        level: event.$metadata.level ?? event.$metadata.type,
        source: typeof event.source === "string" ? event.source : JSON.stringify(event.source),
      }))} emptyLabel="No persisted events in the last hour." />}
      <SectionHeader title="Deployments" description="The first deployment is actively serving traffic." />
      <DataTable columns={[
        { key: "id", label: "Deployment" },
        { key: "created", label: "Created" },
        { key: "versions", label: "Traffic" },
        { key: "actions", label: "" },
      ]} rows={(deployments.data?.deployments ?? []).map(item => ({
        id: item.id,
        created: item.created_on,
        versions: item.versions.map(version => `${version.version_id} ${version.percentage}%`).join(", "),
        actions: item.id === activeDeploymentID ? "Active" : <div className="flex gap-2">
          <Button variant="secondary" onClick={() => setActivateTarget(item.id)}>Activate</Button>
          <Button variant="destructive" onClick={() => setDeleteDeploymentTarget(item.id)}>Delete</Button>
        </div>,
      }))} emptyLabel="No deployments found." />
      <div className="mt-6">
        <SectionHeader title="Versions" />
        <DataTable columns={[{ key: "id", label: "Version" }, { key: "number", label: "Number" }, { key: "source", label: "Source" }]} rows={(versions.data?.result.items ?? []).map(item => ({
          id: item.id ?? "unknown",
          number: item.number ?? "—",
          source: item.metadata?.source ?? "unknown",
        }))} emptyLabel="No versions found." />
      </div>
      <div className="mt-6">
        <SectionHeader title="open-compute endpoints" />
        <DataTable columns={[{ key: "id", label: "Endpoint" }, { key: "path", label: "Path" }, { key: "created", label: "Created" }]} rows={(endpoints.data ?? []).map(item => ({
          id: item.id,
          path: item.path,
          created: item.created_on,
        }))} emptyLabel="No platform endpoints found." />
      </div>
    </>}
  </div>;
}
