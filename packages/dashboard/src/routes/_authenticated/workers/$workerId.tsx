import { useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { Button } from "@cloudflare/kumo/components/button";
import { ConfirmActionDialog } from "../../../components/ConfirmActionDialog";
import { ConfirmDeleteResourceDialog } from "../../../components/ConfirmDeleteResourceDialog";
import { DataTable, ErrorState, LoadingState, PageHeader, SectionHeader } from "../../../components/PageLayout";
import { useAuth } from "../../../features/auth/AuthProvider";
import { useMutationFeedback } from "../../../features/toast/useMutationFeedback";

export const Route = createFileRoute("/_authenticated/workers/$workerId")({ component: WorkerDetailPage });

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
