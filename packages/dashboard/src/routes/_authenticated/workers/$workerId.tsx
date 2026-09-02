import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { z } from "zod";
import { Button } from "@cloudflare/kumo/components/button";
import { Checkbox } from "@cloudflare/kumo/components/checkbox";
import { Input } from "@cloudflare/kumo/components/input";
import { Surface } from "@cloudflare/kumo/components/surface";
import { OperatorApiError, parseDeploymentId, parseRouteId, parseWorkerId } from "@open-compute/operator-sdk";
import { ConfirmActionDialog } from "../../../components/ConfirmActionDialog";
import { ConfirmDeleteResourceDialog } from "../../../components/ConfirmDeleteResourceDialog";
import { DetailTabs } from "../../../components/DetailTabs";
import { BackLink, DataTable, ErrorState, LoadingState, PageHeader, StatusBadge } from "../../../components/PageLayout";
import { docsLinks } from "../../../lib/docs";
import { StructuredSummaryPanel } from "../../../components/StructuredSummary";
import { useMutationFeedback } from "../../../features/toast/useMutationFeedback";
import { buildWorkerDeploymentMetadata } from "../../../lib/deployment";
import { uploadWorkerWithAssets } from "../../../lib/deployment-upload";
import { formatTimestamp } from "../../../lib/format";
import { useAuth } from "../../../features/auth/AuthProvider";
import { queryKeys } from "../../../queries/keys";
import { invalidateWorkersQueries } from "../../../queries/invalidate";

const workerDetailSearchSchema = z.object({
  tab: z.enum(["overview", "deployments", "routes", "cache", "upload"]).optional(),
});

export const Route = createFileRoute("/_authenticated/workers/$workerId")({
  validateSearch: search => workerDetailSearchSchema.parse(search),
  component: WorkerDetailPage,
});

function WorkerDetailPage() {
  const { workerId: workerIdParam } = Route.useParams();
  const { tab: tabParam } = Route.useSearch();
  const activeTab = tabParam ?? "overview";
  const navigate = useNavigate({ from: Route.fullPath });
  const workerId = parseWorkerId(workerIdParam);
  const { client, accountId } = useAuth();
  const queryClient = useQueryClient();
  const [promoteTarget, setPromoteTarget] = useState<string | null>(null);
  const [rollbackTarget, setRollbackTarget] = useState<string | null>(null);
  const [deleteDeploymentTarget, setDeleteDeploymentTarget] = useState<string | null>(null);
  const [mutationError, setMutationError] = useState<string | null>(null);
  const [bundleFile, setBundleFile] = useState<File | null>(null);
  const [assetFiles, setAssetFiles] = useState<File[]>([]);
  const [mainModule, setMainModule] = useState("index.js");
  const [promoteOnUpload, setPromoteOnUpload] = useState(false);
  const [routeHostname, setRouteHostname] = useState("");
  const [routePathPrefix, setRoutePathPrefix] = useState("/");
  const [routeEntrypoint, setRouteEntrypoint] = useState("");
  const [deleteRouteTarget, setDeleteRouteTarget] = useState<{ id: string; pattern: string } | null>(null);
  const [deleteWorkerOpen, setDeleteWorkerOpen] = useState(false);
  const [cachePurgeOpen, setCachePurgeOpen] = useState(false);
  const feedback = useMutationFeedback();

  const workerQuery = useQuery({
    queryKey: queryKeys.worker(accountId ?? "", workerIdParam),
    queryFn: ({ signal }) => client!.workers.get({ accountId: accountId!, workerId, signal }),
    enabled: Boolean(client && accountId),
  });
  const deploymentsQuery = useQuery({
    queryKey: queryKeys.deployments(accountId ?? "", workerIdParam),
    queryFn: ({ signal }) => client!.workers.listDeployments({ accountId: accountId!, workerId, signal }),
    enabled: Boolean(client && accountId) && activeTab === "deployments",
  });
  const routesQuery = useQuery({
    queryKey: queryKeys.routes(accountId ?? "", workerIdParam),
    queryFn: ({ signal }) => client!.workers.listRoutes({ accountId: accountId!, workerId, signal }),
    enabled: Boolean(client && accountId) && activeTab === "routes",
  });
  const cacheQuery = useQuery({
    queryKey: queryKeys.workerCache(accountId ?? "", workerIdParam),
    queryFn: ({ signal }) => client!.platform.workerCache({ accountId: accountId!, workerId, signal }),
    enabled: Boolean(client && accountId) && activeTab === "cache",
  });
  const promoteMutation = useMutation({
    mutationFn: (deploymentId: string) => client!.workers.promote({
      accountId: accountId!,
      workerId,
      targetDeploymentId: parseDeploymentId(deploymentId),
      expectedActiveDeploymentId: workerQuery.data?.worker.activeDeploymentId ?? null,
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.worker(accountId!, workerIdParam) }),
        queryClient.invalidateQueries({ queryKey: queryKeys.deployments(accountId!, workerIdParam) }),
        queryClient.invalidateQueries({ queryKey: queryKeys.routes(accountId!, workerIdParam) }),
      ]);
      setMutationError(null);
      setPromoteTarget(null);
      feedback.success("Deployment promoted.");
    },
    onError: error => {
      setMutationError(
        error instanceof OperatorApiError ? error.message : "Unable to promote the deployment.",
      );
      feedback.failure(error, "Unable to promote the deployment.");
    },
  });
  const rollbackMutation = useMutation({
    mutationFn: (deploymentId: string) => client!.workers.rollback({
      accountId: accountId!,
      workerId,
      targetDeploymentId: parseDeploymentId(deploymentId),
      expectedActiveDeploymentId: workerQuery.data?.worker.activeDeploymentId ?? null,
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.worker(accountId!, workerIdParam) }),
        queryClient.invalidateQueries({ queryKey: queryKeys.deployments(accountId!, workerIdParam) }),
        queryClient.invalidateQueries({ queryKey: queryKeys.routes(accountId!, workerIdParam) }),
      ]);
      setMutationError(null);
      setRollbackTarget(null);
      feedback.success("Deployment rolled back.");
    },
    onError: error => {
      setMutationError(
        error instanceof OperatorApiError ? error.message : "Unable to roll back the deployment.",
      );
      feedback.failure(error, "Unable to roll back the deployment.");
    },
  });
  const deleteDeploymentMutation = useMutation({
    mutationFn: (deploymentId: string) => client!.workers.deleteDeployment({
      accountId: accountId!,
      workerId,
      deploymentId: parseDeploymentId(deploymentId),
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: queryKeys.deployments(accountId!, workerIdParam) });
      setDeleteDeploymentTarget(null);
      setMutationError(null);
      feedback.success("Deployment deleted.");
    },
    onError: error => {
      const message = error instanceof OperatorApiError && error.code === "deployment_referenced"
        ? "This deployment is still referenced by retained operation history or product bindings. Retry after those references expire or are removed."
        : error instanceof OperatorApiError
          ? error.message
          : "Unable to delete the deployment.";
      setMutationError(message);
      feedback.failure(message, "Unable to delete the deployment.");
    },
  });
  const uploadMutation = useMutation({
    mutationFn: async () => {
      if (!bundleFile) throw new Error("Select a bundle file before uploading.");
      const bytes = new Uint8Array(await bundleFile.arrayBuffer());
      if (assetFiles.length > 0) {
        return uploadWorkerWithAssets({
          client: client!,
          accountId: accountId!,
          workerId,
          bundleBytes: bytes,
          assetFiles,
          mainModule,
          promote: promoteOnUpload,
        });
      }
      return client!.workers.createDeployment({
        accountId: accountId!,
        workerId,
        bundle: bytes,
        metadata: buildWorkerDeploymentMetadata({ mainModule, promote: promoteOnUpload }),
        idempotencyKey: crypto.randomUUID(),
      });
    },
    onSuccess: async result => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: queryKeys.worker(accountId!, workerIdParam) }),
        queryClient.invalidateQueries({ queryKey: queryKeys.deployments(accountId!, workerIdParam) }),
      ]);
      setBundleFile(null);
      setAssetFiles([]);
      feedback.success(
        result.promoted
          ? "Deployment uploaded and promoted."
          : "Deployment uploaded. Promote it from the Deployments tab when ready.",
      );
      void navigate({ search: prev => ({ ...prev, tab: "deployments" }) });
    },
    onError: error => {
      feedback.failure(error, "Unable to upload the deployment bundle.");
    },
  });
  const createRouteMutation = useMutation({
    mutationFn: () => client!.workers.createRoute({
      accountId: accountId!,
      workerId,
      hostname: routeHostname.trim(),
      pathPrefix: routePathPrefix.trim(),
      ...(routeEntrypoint.trim() ? { entrypoint: routeEntrypoint.trim() } : {}),
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: queryKeys.routes(accountId!, workerIdParam) });
      setRouteHostname("");
      setRoutePathPrefix("/");
      setRouteEntrypoint("");
      setMutationError(null);
      feedback.success("Route created.");
    },
    onError: error => {
      setMutationError(
        error instanceof OperatorApiError ? error.message : "Unable to create the route.",
      );
      feedback.failure(error, "Unable to create the route.");
    },
  });
  const deleteRouteMutation = useMutation({
    mutationFn: (routeId: string) => client!.workers.deleteRoute({
      accountId: accountId!,
      workerId,
      routeId: parseRouteId(routeId),
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: queryKeys.routes(accountId!, workerIdParam) });
      setDeleteRouteTarget(null);
      setMutationError(null);
      feedback.success("Route deleted.");
    },
    onError: error => {
      setMutationError(
        error instanceof OperatorApiError ? error.message : "Unable to delete the route.",
      );
      feedback.failure(error, "Unable to delete the route.");
    },
  });
  const deleteWorkerMutation = useMutation({
    mutationFn: () => client!.workers.delete({
      accountId: accountId!,
      workerId,
      idempotencyKey: crypto.randomUUID(),
    }),
    onSuccess: async () => {
      await invalidateWorkersQueries(queryClient, accountId!);
      feedback.success("Worker deleted.");
      void navigate({ to: "/workers" });
    },
    onError: error => {
      setMutationError(
        error instanceof OperatorApiError ? error.message : "Unable to delete the Worker.",
      );
      feedback.failure(error, "Unable to delete the Worker.");
    },
  });
  const purgeCacheMutation = useMutation({
    mutationFn: () => client!.platform.purgeWorkerCache({ accountId: accountId!, workerId }),
    onSuccess: async result => {
      await queryClient.invalidateQueries({ queryKey: queryKeys.workerCache(accountId!, workerIdParam) });
      setCachePurgeOpen(false);
      feedback.success(`Purged ${result.deleted} cache entr${result.deleted === 1 ? "y" : "ies"}.`);
    },
    onError: error => feedback.failure(error, "Unable to purge the Worker cache."),
  });

  const activeDeploymentId = workerQuery.data?.worker.activeDeploymentId ?? null;
  const mutationPending = promoteMutation.isPending || rollbackMutation.isPending || deleteDeploymentMutation.isPending || createRouteMutation.isPending || deleteRouteMutation.isPending || deleteWorkerMutation.isPending || purgeCacheMutation.isPending;
  const worker = workerQuery.data?.worker;

  return (
    <div>
      <PageHeader
        title={worker?.name ?? workerIdParam}
        description="Deployment history and route bindings for this Worker."
        docsUrl={docsLinks.workers}
        resourceId={workerIdParam}
        resourceLabel="Worker ID"
        actions={<BackLink to="/workers" label="Back to Workers" />}
      />
      <DetailTabs
        tabs={[
          { id: "overview", label: "Overview" },
          { id: "deployments", label: "Deployments" },
          { id: "routes", label: "Routes" },
          { id: "cache", label: "Cache" },
          { id: "upload", label: "Upload" },
        ]}
        activeTab={activeTab}
        onTabChange={tabId => {
          void navigate({ search: prev => ({ ...prev, tab: tabId as "overview" | "deployments" | "routes" | "cache" | "upload" }) });
        }}
      />
      {workerQuery.isLoading ? <LoadingState /> : null}
      {workerQuery.error ? <ErrorState message="Unable to load Worker details." /> : null}
      <ConfirmActionDialog
        title="Promote deployment"
        description="This switches the active deployment pointer for this Worker."
        resourceLabel="the deployment ID"
        confirmValue={promoteTarget ?? ""}
        submitLabel="Promote"
        open={Boolean(promoteTarget)}
        errorMessage={promoteTarget ? mutationError : null}
        isPending={promoteMutation.isPending}
        onClose={() => {
          setPromoteTarget(null);
          setMutationError(null);
        }}
        onConfirm={() => {
          if (!promoteTarget) return;
          promoteMutation.mutate(promoteTarget);
        }}
      />
      <ConfirmActionDialog
        title="Rollback deployment"
        description="This moves the active deployment pointer back to the selected ready deployment."
        resourceLabel="the deployment ID"
        confirmValue={rollbackTarget ?? ""}
        submitLabel="Rollback"
        open={Boolean(rollbackTarget)}
        errorMessage={rollbackTarget ? mutationError : null}
        isPending={rollbackMutation.isPending}
        onClose={() => {
          setRollbackTarget(null);
          setMutationError(null);
        }}
        onConfirm={() => {
          if (!rollbackTarget) return;
          rollbackMutation.mutate(rollbackTarget);
        }}
      />
      <ConfirmActionDialog
        title="Delete route"
        description="This tombstones the route binding. Traffic matching this pattern will stop routing to the Worker."
        resourceLabel="the route ID"
        confirmValue={deleteRouteTarget?.id ?? ""}
        submitLabel="Delete route"
        open={Boolean(deleteRouteTarget)}
        errorMessage={deleteRouteTarget ? mutationError : null}
        isPending={deleteRouteMutation.isPending}
        onClose={() => {
          setDeleteRouteTarget(null);
          setMutationError(null);
        }}
        onConfirm={() => {
          if (!deleteRouteTarget) return;
          deleteRouteMutation.mutate(deleteRouteTarget.id);
        }}
      />
      <ConfirmActionDialog
        title="Delete deployment"
        description="This permanently retires the inactive deployment. Active deployments and deployments retained by operation history or product bindings cannot be deleted."
        resourceLabel="the deployment ID"
        confirmValue={deleteDeploymentTarget ?? ""}
        submitLabel="Delete deployment"
        submitVariant="destructive"
        open={Boolean(deleteDeploymentTarget)}
        errorMessage={deleteDeploymentTarget ? mutationError : null}
        isPending={deleteDeploymentMutation.isPending}
        onClose={() => {
          setDeleteDeploymentTarget(null);
          setMutationError(null);
        }}
        onConfirm={() => {
          if (!deleteDeploymentTarget) return;
          deleteDeploymentMutation.mutate(deleteDeploymentTarget);
        }}
      />
      <ConfirmDeleteResourceDialog
        title="Delete Worker"
        description="This tombstones the Worker and drains active deployments. Routes and deployments become unavailable."
        resourceLabel="the Worker name"
        confirmValue={worker?.name ?? workerIdParam}
        open={deleteWorkerOpen}
        errorMessage={deleteWorkerOpen ? mutationError : null}
        isPending={deleteWorkerMutation.isPending}
        onClose={() => {
          setDeleteWorkerOpen(false);
          setMutationError(null);
        }}
        onConfirm={() => deleteWorkerMutation.mutate()}
      />
      <ConfirmActionDialog
        title="Purge Worker cache"
        description="This permanently deletes cached responses for this Worker. New requests will repopulate the cache."
        resourceLabel="the Worker name"
        confirmValue={worker?.name ?? workerIdParam}
        submitLabel="Purge cache"
        submitVariant="destructive"
        open={cachePurgeOpen}
        errorMessage={purgeCacheMutation.error instanceof Error ? purgeCacheMutation.error.message : null}
        isPending={purgeCacheMutation.isPending}
        onClose={() => setCachePurgeOpen(false)}
        onConfirm={() => purgeCacheMutation.mutate()}
      />
      {activeTab === "overview" && worker ? (
        <Surface className="p-6">
          <dl className="grid gap-4 sm:grid-cols-2">
            <div>
              <dt className="text-xs font-medium text-kumo-subtle">Worker ID</dt>
              <dd className="mt-1 font-mono text-sm">{worker.id}</dd>
            </div>
            <div>
              <dt className="text-xs font-medium text-kumo-subtle">Name</dt>
              <dd className="mt-1 text-sm">{worker.name}</dd>
            </div>
            <div>
              <dt className="text-xs font-medium text-kumo-subtle">Active deployment</dt>
              <dd className="mt-1 font-mono text-sm">{worker.activeDeploymentId ?? "—"}</dd>
            </div>
            <div>
              <dt className="text-xs font-medium text-kumo-subtle">Updated</dt>
              <dd className="mt-1 text-sm">{formatTimestamp(worker.updatedAtMs ?? worker.createdAtMs)}</dd>
            </div>
          </dl>
          <div className="mt-6">
            <Button variant="destructive" onClick={() => setDeleteWorkerOpen(true)}>
              Delete Worker
            </Button>
          </div>
        </Surface>
      ) : null}
      {activeTab === "deployments" ? (
        <section>
          {deploymentsQuery.isLoading ? (
            <LoadingState />
          ) : deploymentsQuery.error ? (
            <ErrorState message="Unable to load deployments." />
          ) : (
            <DataTable
              columns={[
                { key: "id", label: "Deployment" },
                { key: "state", label: "State" },
                { key: "ready", label: "Ready at" },
                { key: "actions", label: "" },
              ]}
              rows={(deploymentsQuery.data?.deployments ?? []).map(deployment => {
                const isActive = deployment.id === activeDeploymentId;
                const canPromote = deployment.state === "ready" && !isActive;
                return {
                  id: (
                    <div className="flex items-center gap-2">
                      <code className="[font-size:0.9em]">{deployment.id}</code>
                      {isActive ? <StatusBadge value="active" /> : null}
                    </div>
                  ),
                  state: <StatusBadge value={deployment.state} />,
                  ready: formatTimestamp(deployment.readyAtMs ?? undefined),
                  actions: !isActive ? (
                    <div className="flex flex-wrap gap-2">
                      {canPromote ? (
                        <>
                      <Button
                        variant="primary"
                        disabled={mutationPending}
                        onClick={() => {
                          setMutationError(null);
                          setPromoteTarget(deployment.id);
                        }}
                      >
                        Promote
                      </Button>
                      {activeDeploymentId ? (
                        <Button
                          variant="secondary"
                          disabled={mutationPending}
                          onClick={() => {
                            setMutationError(null);
                            setRollbackTarget(deployment.id);
                          }}
                        >
                          Rollback
                        </Button>
                      ) : null}
                        </>
                      ) : null}
                      <Button
                        variant="destructive"
                        disabled={mutationPending || deployment.state === "deleting"}
                        onClick={() => {
                          setMutationError(null);
                          setDeleteDeploymentTarget(deployment.id);
                        }}
                      >
                        Delete
                      </Button>
                    </div>
                  ) : "—",
                };
              })}
              emptyLabel="This Worker has no deployments yet."
            />
          )}
        </section>
      ) : null}
      {activeTab === "routes" ? (
        <section className="space-y-4">
          <Surface className="p-6">
            <div className="space-y-4">
              <p className="text-sm text-kumo-subtle">
                Bind a hostname and path prefix to this Worker. Named entrypoints require an active deployment.
              </p>
              <Input
                label="Hostname"
                value={routeHostname}
                onChange={event => setRouteHostname(event.target.value)}
                placeholder="example.localhost"
              />
              <Input
                label="Path prefix"
                value={routePathPrefix}
                onChange={event => setRoutePathPrefix(event.target.value)}
                placeholder="/"
              />
              <Input
                label="Entrypoint (optional)"
                value={routeEntrypoint}
                onChange={event => setRouteEntrypoint(event.target.value)}
                placeholder="fetch"
              />
              <Button
                variant="primary"
                disabled={!routeHostname.trim() || !routePathPrefix.trim() || createRouteMutation.isPending}
                onClick={() => createRouteMutation.mutate()}
              >
                {createRouteMutation.isPending ? "Creating…" : "Create route"}
              </Button>
            </div>
          </Surface>
          {routesQuery.isLoading ? (
            <LoadingState />
          ) : routesQuery.error ? (
            <ErrorState message="Unable to load routes." />
          ) : (
            <DataTable
              columns={[
                { key: "pattern", label: "Pattern" },
                { key: "kind", label: "Kind" },
                { key: "deployment", label: "Deployment" },
                { key: "actions", label: "" },
              ]}
              rows={(routesQuery.data?.routes ?? []).map(route => ({
                pattern: route.hostnameAscii ? `${route.hostnameAscii}${route.pathPrefix}` : route.pathPrefix,
                kind: route.kind,
                deployment: route.deploymentId ?? "—",
                actions: (
                  <Button
                    variant="destructive"
                    disabled={deleteRouteMutation.isPending}
                    onClick={() => {
                      setMutationError(null);
                      setDeleteRouteTarget({ id: route.id, pattern: route.pathPrefix });
                    }}
                  >
                    Delete
                  </Button>
                ),
              }))}
              emptyLabel="No routes are bound to this Worker."
            />
          )}
        </section>
      ) : null}
      {activeTab === "cache" ? (
        <div className="space-y-4">
          <StructuredSummaryPanel title="Worker cache" query={cacheQuery} />
          <div className="flex justify-end">
            <Button
              variant="destructive"
              disabled={purgeCacheMutation.isPending}
              onClick={() => setCachePurgeOpen(true)}
            >
              Purge cache
            </Button>
          </div>
        </div>
      ) : null}
      {activeTab === "upload" ? (
        <Surface className="p-6">
          <div className="space-y-4">
            <p className="text-sm text-kumo-subtle">
              Upload a compiled Worker bundle through the Operator API. Bundle-only deployments use direct upload; add static assets to use the resumable upload session flow.
            </p>
            <Input
              label="Main module"
              value={mainModule}
              onChange={event => setMainModule(event.target.value)}
              placeholder="index.js"
            />
            <div>
              <label className="mb-2 block text-sm font-medium" htmlFor="worker-bundle-file">
                Bundle file
              </label>
              <input
                id="worker-bundle-file"
                type="file"
                accept=".zip,.bin,application/octet-stream,application/zip"
                onChange={event => setBundleFile(event.target.files?.[0] ?? null)}
              />
              {bundleFile ? (
                <div className="mt-2 text-sm text-kumo-subtle">
                  {bundleFile.name} ({bundleFile.size.toLocaleString()} bytes)
                </div>
              ) : null}
            </div>
            <div>
              <label className="mb-2 block text-sm font-medium" htmlFor="worker-asset-files">
                Static assets (optional)
              </label>
              <input
                id="worker-asset-files"
                type="file"
                multiple
                {...({ webkitdirectory: "", directory: "" } as Record<string, string>)}
                onChange={event => setAssetFiles(Array.from(event.target.files ?? []))}
              />
              {assetFiles.length > 0 ? (
                <div className="mt-2 text-sm text-kumo-subtle">
                  {assetFiles.length} file{assetFiles.length === 1 ? "" : "s"} selected
                </div>
              ) : (
                <p className="mt-2 text-xs text-kumo-subtle">
                  Select a directory or multiple files to include static assets in a resumable upload session.
                </p>
              )}
            </div>
            <Checkbox
              label="Promote immediately after upload"
              checked={promoteOnUpload}
              onCheckedChange={checked => setPromoteOnUpload(checked === true)}
            />
            <Button
              variant="primary"
              disabled={!bundleFile || uploadMutation.isPending}
              onClick={() => uploadMutation.mutate()}
            >
              {uploadMutation.isPending
                ? "Uploading…"
                : assetFiles.length > 0
                  ? "Upload via session"
                  : "Upload deployment"}
            </Button>
          </div>
        </Surface>
      ) : null}
    </div>
  );
}
