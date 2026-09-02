import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { z } from "zod";
import { OperatorApiError, parseDeploymentId, parseWorkflowId } from "@open-compute/operator-sdk";
import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import { Surface } from "@cloudflare/kumo/components/surface";
import { ConfirmActionDialog } from "../../../components/ConfirmActionDialog";
import { CreateWorkflowVersionDialog } from "../../../components/CreateWorkflowVersionDialog";
import { DetailTabs } from "../../../components/DetailTabs";
import { BackLink, DataTable, ErrorState, LoadingState, PageHeader, StatusBadge } from "../../../components/PageLayout";
import { docsLinks } from "../../../lib/docs";
import { formatTimestamp } from "../../../lib/format";
import { useAuth } from "../../../features/auth/AuthProvider";
import { useMutationFeedback } from "../../../features/toast/useMutationFeedback";
import { queryKeys } from "../../../queries/keys";

const workflowDetailSearchSchema = z.object({
  tab: z.enum(["overview", "versions", "instances"]).optional(),
  instance: z.string().optional(),
});

export const Route = createFileRoute("/_authenticated/workflows/$workflowId")({
  validateSearch: search => workflowDetailSearchSchema.parse(search),
  component: WorkflowDetailPage,
});

function WorkflowDetailPage() {
  const { workflowId: workflowIdParam } = Route.useParams();
  const { tab: tabParam, instance: selectedInstanceId = "" } = Route.useSearch();
  const activeTab = tabParam ?? "overview";
  const navigate = useNavigate({ from: Route.fullPath });
  const workflowId = parseWorkflowId(workflowIdParam);
  const { client, accountId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const [createVersionOpen, setCreateVersionOpen] = useState(false);
  const [confirmAction, setConfirmAction] = useState<{ action: "terminate" | "restart"; instanceId: string } | null>(null);
  const [eventType, setEventType] = useState("");
  const [eventPayload, setEventPayload] = useState("");
  const [mutationError, setMutationError] = useState<string | null>(null);

  const workflowQuery = useQuery({
    queryKey: queryKeys.workflow(accountId ?? "", workflowIdParam),
    queryFn: ({ signal }) => client!.workflows.get({ accountId: accountId!, workflowId, signal }),
    enabled: Boolean(client && accountId),
  });

  const versionsQuery = useQuery({
    queryKey: queryKeys.workflowVersions(accountId ?? "", workflowIdParam),
    queryFn: ({ signal }) => client!.workflows.listVersions({
      accountId: accountId!,
      workflowId,
      after: 0,
      limit: 100,
      signal,
    }),
    enabled: Boolean(client && accountId) && activeTab === "versions",
  });

  const instancesQuery = useQuery({
    queryKey: queryKeys.workflowInstances(accountId ?? "", workflowIdParam),
    queryFn: ({ signal }) => client!.workflows.listInstances({
      accountId: accountId!,
      workflowId,
      limit: 100,
      signal,
    }),
    enabled: Boolean(client && accountId) && activeTab === "instances",
  });

  const selectedInstanceQuery = useQuery({
    queryKey: queryKeys.workflowInstance(accountId ?? "", workflowIdParam, selectedInstanceId),
    queryFn: ({ signal }) => client!.workflows.getInstance({
      accountId: accountId!,
      workflowId,
      instanceId: selectedInstanceId,
      signal,
    }),
    enabled: Boolean(client && accountId && selectedInstanceId && activeTab === "instances"),
  });

  const stepsQuery = useQuery({
    queryKey: queryKeys.workflowSteps(accountId ?? "", workflowIdParam, selectedInstanceId),
    queryFn: ({ signal }) => client!.workflows.listSteps({
      accountId: accountId!,
      workflowId,
      instanceId: selectedInstanceId,
      limit: 100,
      signal,
    }),
    enabled: Boolean(client && accountId && selectedInstanceId && activeTab === "instances"),
  });

  const invalidateWorkflowRuntime = async (instanceId?: string) => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: queryKeys.workflow(accountId!, workflowIdParam) }),
      queryClient.invalidateQueries({ queryKey: queryKeys.workflowVersions(accountId!, workflowIdParam) }),
      queryClient.invalidateQueries({ queryKey: queryKeys.workflowInstances(accountId!, workflowIdParam) }),
      ...(instanceId ? [
        queryClient.invalidateQueries({ queryKey: queryKeys.workflowInstance(accountId!, workflowIdParam, instanceId) }),
        queryClient.invalidateQueries({ queryKey: queryKeys.workflowSteps(accountId!, workflowIdParam, instanceId) }),
      ] : []),
    ]);
  };

  const createVersionMutation = useMutation({
    mutationFn: (input: { deploymentId: string; className: string }) => client!.workflows.createVersion({
      accountId: accountId!,
      workflowId,
      deploymentId: parseDeploymentId(input.deploymentId),
      className: input.className,
    }),
    onSuccess: async () => {
      await invalidateWorkflowRuntime();
      setCreateVersionOpen(false);
      setMutationError(null);
      feedback.success("Workflow version created.");
    },
    onError: error => {
      setMutationError(error instanceof OperatorApiError ? error.message : "Unable to create the workflow version.");
      feedback.failure(error, "Unable to create the workflow version.");
    },
  });

  const actionMutation = useMutation({
    mutationFn: (input: { action: "pause" | "resume" | "terminate" | "restart"; instanceId: string }) => {
      const params = { accountId: accountId!, workflowId, instanceId: input.instanceId };
      switch (input.action) {
        case "pause": return client!.workflows.pauseInstance(params);
        case "resume": return client!.workflows.resumeInstance(params);
        case "terminate": return client!.workflows.terminateInstance(params);
        case "restart": return client!.workflows.restartInstance(params);
      }
    },
    onSuccess: async (_result, input) => {
      await invalidateWorkflowRuntime(input.instanceId);
      setConfirmAction(null);
      setMutationError(null);
      feedback.success(`Workflow instance ${input.action} completed.`);
    },
    onError: error => {
      setMutationError(error instanceof OperatorApiError ? error.message : "Unable to update the workflow instance.");
      feedback.failure(error, "Unable to update the workflow instance.");
    },
  });

  const sendEventMutation = useMutation({
    mutationFn: () => client!.workflows.sendEvent({
      accountId: accountId!,
      workflowId,
      instanceId: selectedInstanceId,
      eventType: eventType.trim(),
      payloadBase64: eventPayload.trim(),
    }),
    onSuccess: async () => {
      await invalidateWorkflowRuntime(selectedInstanceId);
      setEventType("");
      setEventPayload("");
      setMutationError(null);
      feedback.success("Workflow event delivered.");
    },
    onError: error => {
      setMutationError(error instanceof OperatorApiError ? error.message : "Unable to deliver the workflow event.");
      feedback.failure(error, "Unable to deliver the workflow event.");
    },
  });

  const definition = workflowQuery.data?.definition;

  return (
    <div>
      <PageHeader
        title={definition?.name ?? workflowIdParam}
        description="Workflow definition, validated versions, and bounded instance inventory."
        docsUrl={docsLinks.platform}
        resourceId={workflowIdParam}
        resourceLabel="Workflow ID"
        actions={<BackLink to="/workflows" label="Back to Workflows" />}
      />
      <DetailTabs
        tabs={[
          { id: "overview", label: "Overview" },
          { id: "versions", label: "Versions" },
          { id: "instances", label: "Instances" },
        ]}
        activeTab={activeTab}
        onTabChange={tabId => {
          void navigate({ search: prev => ({ ...prev, tab: tabId as "overview" | "versions" | "instances" }) });
        }}
      />
      <CreateWorkflowVersionDialog
        open={createVersionOpen}
        errorMessage={createVersionOpen ? mutationError : null}
        isPending={createVersionMutation.isPending}
        onClose={() => {
          setCreateVersionOpen(false);
          setMutationError(null);
        }}
        onSubmit={input => createVersionMutation.mutate(input)}
      />
      <ConfirmActionDialog
        title={confirmAction?.action === "terminate" ? "Terminate workflow instance" : "Restart workflow instance"}
        description={confirmAction?.action === "terminate"
          ? "Termination is durable and prevents additional events until the instance is restarted."
          : "Restart creates a new generation from the current workflow version."}
        resourceLabel="the instance ID"
        confirmValue={confirmAction?.instanceId ?? ""}
        submitLabel={confirmAction?.action === "terminate" ? "Terminate instance" : "Restart instance"}
        submitVariant={confirmAction?.action === "terminate" ? "destructive" : "primary"}
        open={Boolean(confirmAction)}
        errorMessage={confirmAction ? mutationError : null}
        isPending={actionMutation.isPending}
        onClose={() => {
          setConfirmAction(null);
          setMutationError(null);
        }}
        onConfirm={() => {
          if (confirmAction) actionMutation.mutate(confirmAction);
        }}
      />
      {workflowQuery.isLoading ? <LoadingState /> : null}
      {workflowQuery.error ? <ErrorState message="Unable to load the Workflow definition." /> : null}
      {activeTab === "overview" && definition ? (
        <dl className="grid gap-4 rounded-lg border border-kumo-line bg-kumo-elevated p-6 sm:grid-cols-2">
          <div>
            <dt className="text-xs font-medium text-kumo-subtle">Definition ID</dt>
            <dd className="mt-1 font-mono text-sm">{definition.id}</dd>
          </div>
          <div>
            <dt className="text-xs font-medium text-kumo-subtle">State</dt>
            <dd className="mt-1"><StatusBadge value={definition.state} /></dd>
          </div>
          <div>
            <dt className="text-xs font-medium text-kumo-subtle">Current version</dt>
            <dd className="mt-1 font-mono text-sm">{definition.currentVersionId ?? "—"}</dd>
          </div>
          <div>
            <dt className="text-xs font-medium text-kumo-subtle">Referrers</dt>
            <dd className="mt-1 text-sm">{workflowQuery.data?.referrerCount ?? 0}</dd>
          </div>
          <div>
            <dt className="text-xs font-medium text-kumo-subtle">Updated</dt>
            <dd className="mt-1 text-sm">{formatTimestamp(definition.updatedAtMs)}</dd>
          </div>
        </dl>
      ) : null}
      {activeTab === "versions" ? (
        versionsQuery.isLoading ? (
          <LoadingState />
        ) : versionsQuery.error ? (
          <ErrorState message="Unable to load Workflow versions." />
        ) : (
          <section className="space-y-4">
            <Button variant="primary" onClick={() => setCreateVersionOpen(true)}>Create version</Button>
            <DataTable
            columns={[
              { key: "version", label: "Version" },
              { key: "state", label: "State" },
              { key: "class", label: "Class" },
              { key: "deployment", label: "Deployment" },
              { key: "created", label: "Created" },
            ]}
            rows={(versionsQuery.data ?? []).map(version => ({
              version: String(version.versionNumber),
              state: <StatusBadge value={version.state} />,
              class: version.target.className,
              deployment: <code className="[font-size:0.9em]">{version.target.deploymentId}</code>,
              created: formatTimestamp(version.createdAtMs),
            }))}
            emptyLabel="No validated versions exist for this Workflow."
            />
          </section>
        )
      ) : null}
      {activeTab === "instances" ? (
        instancesQuery.isLoading ? (
          <LoadingState />
        ) : instancesQuery.error ? (
          <ErrorState message="Unable to load Workflow instances." />
        ) : (
          <section className="space-y-4">
            <DataTable
            columns={[
              { key: "id", label: "Instance ID" },
              { key: "external", label: "External ID" },
              { key: "status", label: "Status" },
              { key: "steps", label: "Steps" },
              { key: "created", label: "Created" },
              { key: "actions", label: "" },
            ]}
            rows={(instancesQuery.data ?? []).map(instance => ({
              id: <code className="[font-size:0.9em]">{instance.id}</code>,
              external: instance.externalInstanceId,
              status: <StatusBadge value={instance.status} />,
              steps: `${instance.completedStepCount ?? 0}/${instance.stepCount ?? 0}`,
              created: formatTimestamp(instance.createdAtMs),
              actions: (
                <Button
                  variant="secondary"
                  onClick={() => {
                    setMutationError(null);
                    void navigate({ search: prev => ({ ...prev, instance: instance.id }) });
                  }}
                >
                  Inspect
                </Button>
              ),
            }))}
            emptyLabel="No instances are registered for this Workflow."
            />
            {selectedInstanceId ? (
              <Surface className="p-5">
                {selectedInstanceQuery.isLoading ? <LoadingState label="Loading instance…" /> : null}
                {selectedInstanceQuery.error ? <ErrorState message="Unable to inspect the workflow instance." /> : null}
                {selectedInstanceQuery.data ? (
                  <div className="space-y-5">
                    <div className="flex flex-wrap items-start justify-between gap-3">
                      <div>
                        <h2 className="text-base font-semibold">Instance {selectedInstanceQuery.data.externalInstanceId}</h2>
                        <p className="mt-1 text-sm text-kumo-subtle"><code className="text-[0.9em]">{selectedInstanceId}</code> · generation {selectedInstanceQuery.data.generation}</p>
                      </div>
                      <div className="flex flex-wrap gap-2">
                        {selectedInstanceQuery.data.status === "paused" ? (
                          <Button variant="primary" disabled={actionMutation.isPending} onClick={() => actionMutation.mutate({ action: "resume", instanceId: selectedInstanceId })}>Resume</Button>
                        ) : (
                          <Button variant="secondary" disabled={actionMutation.isPending || ["terminated", "complete", "errored"].includes(selectedInstanceQuery.data.status)} onClick={() => actionMutation.mutate({ action: "pause", instanceId: selectedInstanceId })}>Pause</Button>
                        )}
                        <Button variant="secondary" disabled={actionMutation.isPending} onClick={() => setConfirmAction({ action: "restart", instanceId: selectedInstanceId })}>Restart</Button>
                        <Button variant="destructive" disabled={actionMutation.isPending || selectedInstanceQuery.data.status === "terminated"} onClick={() => setConfirmAction({ action: "terminate", instanceId: selectedInstanceId })}>Terminate</Button>
                      </div>
                    </div>
                    {mutationError ? <p className="text-sm text-kumo-danger" role="alert">{mutationError}</p> : null}
                    <div className="grid gap-3 sm:grid-cols-[minmax(0,0.4fr)_minmax(0,1fr)_auto]">
                      <Input label="Event type" value={eventType} onChange={event => setEventType(event.target.value)} placeholder="approval" />
                      <Input label="Payload (base64)" value={eventPayload} onChange={event => setEventPayload(event.target.value)} />
                      <div className="flex items-end">
                        <Button variant="primary" disabled={sendEventMutation.isPending || !eventType.trim() || !eventPayload.trim()} onClick={() => sendEventMutation.mutate()}>Send event</Button>
                      </div>
                    </div>
                    {stepsQuery.isLoading ? <LoadingState label="Loading steps…" /> : stepsQuery.error ? <ErrorState message="Unable to load workflow steps." /> : (
                      <DataTable
                        columns={[
                          { key: "ordinal", label: "Step" },
                          { key: "name", label: "Name" },
                          { key: "kind", label: "Kind" },
                          { key: "state", label: "State" },
                          { key: "attempt", label: "Attempt" },
                        ]}
                        rows={(stepsQuery.data ?? []).map(step => ({
                          ordinal: String(step.ordinal),
                          name: step.name,
                          kind: step.kind,
                          state: <StatusBadge value={step.state} />,
                          attempt: String(step.attempt),
                        }))}
                        emptyLabel="This instance has not persisted any workflow steps."
                      />
                    )}
                  </div>
                ) : null}
              </Surface>
            ) : null}
          </section>
        )
      ) : null}
    </div>
  );
}
