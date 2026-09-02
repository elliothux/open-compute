import { useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import { ConfirmActionDialog } from "../../../components/ConfirmActionDialog";
import { DataTable, ErrorState, LoadingState, PageHeader, SectionHeader, StatusBadge } from "../../../components/PageLayout";
import { WorkflowDefinitionDialog } from "../../../components/WorkflowDefinitionDialog";
import { useAuth } from "../../../features/auth/AuthProvider";
import { useMutationFeedback } from "../../../features/toast/useMutationFeedback";

type WorkflowAction = "pause" | "resume" | "terminate" | "restart";

export const Route = createFileRoute("/_authenticated/workflows/$workflowId")({ component: WorkflowDetailPage });

function WorkflowDetailPage() {
  const { workflowId } = Route.useParams();
  const { client, accountId } = useAuth();
  const feedback = useMutationFeedback();
  const enabled = client !== null && accountId !== null;
  const [definitionOpen, setDefinitionOpen] = useState(false);
  const [selectedInstanceId, setSelectedInstanceId] = useState<string | null>(null);
  const [confirmedAction, setConfirmedAction] = useState<Extract<WorkflowAction, "terminate" | "restart"> | null>(null);
  const [eventType, setEventType] = useState("");
  const [eventBody, setEventBody] = useState("");
  const [mutationError, setMutationError] = useState<string | null>(null);
  const workflow = useQuery({
    queryKey: ["cloudflare-v4", "workflows", workflowId],
    queryFn: ({ signal }) => client!.cloudflare.workflows.get(workflowId, { account_id: accountId! }, { signal }),
    enabled,
  });
  const versions = useQuery({
    queryKey: ["cloudflare-v4", "workflows", workflowId, "versions"],
    queryFn: ({ signal }) => client!.cloudflare.workflows.versions.list(workflowId, { account_id: accountId! }, { signal }),
    enabled,
  });
  const instances = useQuery({
    queryKey: ["cloudflare-v4", "workflows", workflowId, "instances"],
    queryFn: ({ signal }) => client!.cloudflare.workflows.instances.list(workflowId, { account_id: accountId! }, { signal }),
    enabled,
  });
  const instance = useQuery({
    queryKey: ["cloudflare-v4", "workflows", workflowId, "instances", selectedInstanceId],
    queryFn: ({ signal }) => client!.cloudflare.workflows.instances.get(selectedInstanceId!, {
      account_id: accountId!,
      workflow_name: workflowId,
    }, { signal }),
    enabled: enabled && selectedInstanceId !== null,
  });
  const refreshRuntime = async () => {
    await Promise.all([instances.refetch(), selectedInstanceId === null ? Promise.resolve() : instance.refetch()]);
  };
  const definitionMutation = useMutation({
    mutationFn: (input: { scriptName: string; className: string }) => client!.cloudflare.workflows.update(workflowId, {
      account_id: accountId!,
      script_name: input.scriptName,
      class_name: input.className,
    }),
    onSuccess: async () => {
      setDefinitionOpen(false);
      setMutationError(null);
      await Promise.all([workflow.refetch(), versions.refetch()]);
      feedback.success("Workflow definition updated.");
    },
    onError: error => {
      setMutationError(error instanceof Error ? error.message : "Unable to update the workflow definition.");
      feedback.failure(error, "Unable to update the workflow definition.");
    },
  });
  const actionMutation = useMutation({
    mutationFn: (action: WorkflowAction) => client!.cloudflare.workflows.instances.status.edit(selectedInstanceId!, {
      account_id: accountId!,
      workflow_name: workflowId,
      status: action,
    }),
    onSuccess: async (_result, action) => {
      setConfirmedAction(null);
      setMutationError(null);
      await refreshRuntime();
      feedback.success(`Workflow instance ${action} completed.`);
    },
    onError: error => {
      setMutationError(error instanceof Error ? error.message : "Unable to update the workflow instance.");
      feedback.failure(error, "Unable to update the workflow instance.");
    },
  });
  const eventMutation = useMutation({
    mutationFn: () => {
      let body: unknown = undefined;
      if (eventBody.trim()) {
        body = JSON.parse(eventBody) as unknown;
      }
      return client!.cloudflare.workflows.instances.events.create(eventType.trim(), {
        account_id: accountId!,
        workflow_name: workflowId,
        instance_id: selectedInstanceId!,
        body,
      });
    },
    onSuccess: async () => {
      setEventBody("");
      await refreshRuntime();
      feedback.success("Workflow event sent.");
    },
    onError: error => {
      setMutationError(error instanceof Error ? error.message : "Unable to send the workflow event.");
      feedback.failure(error, "Unable to send the workflow event.");
    },
  });
  const selectedStatus = instance.data?.status;
  return <div>
    <PageHeader
      title={workflow.data?.name ?? workflowId}
      description="Manage the definition, versions, and instances through the official Workflows API."
      actions={<Button variant="primary" disabled={!workflow.data} onClick={() => setDefinitionOpen(true)}>Update definition</Button>}
    />
    <WorkflowDefinitionDialog
      mode="edit"
      open={definitionOpen}
      {...(workflow.data ? { initial: { scriptName: workflow.data.script_name, className: workflow.data.class_name } } : {})}
      errorMessage={definitionOpen ? mutationError : null}
      isPending={definitionMutation.isPending}
      onClose={() => { setDefinitionOpen(false); setMutationError(null); }}
      onSubmit={input => definitionMutation.mutate(input)}
    />
    <ConfirmActionDialog
      title={confirmedAction === "terminate" ? "Terminate workflow instance" : "Restart workflow instance"}
      description={confirmedAction === "terminate" ? "Termination stops this instance." : "Restart the selected workflow instance."}
      resourceLabel="instance ID"
      confirmValue={selectedInstanceId ?? ""}
      submitLabel={confirmedAction === "terminate" ? "Terminate instance" : "Restart instance"}
      submitVariant={confirmedAction === "terminate" ? "destructive" : "primary"}
      open={confirmedAction !== null}
      errorMessage={confirmedAction ? mutationError : null}
      isPending={actionMutation.isPending}
      onClose={() => { setConfirmedAction(null); setMutationError(null); }}
      onConfirm={() => { if (confirmedAction) actionMutation.mutate(confirmedAction); }}
    />
    {workflow.isLoading || versions.isLoading || instances.isLoading ? <LoadingState /> : workflow.error || versions.error || instances.error ? <ErrorState message="Unable to load Workflow details." /> : <>
      <SectionHeader title="Versions" />
      <DataTable columns={[
        { key: "id", label: "Version" },
        { key: "class", label: "Class" },
        { key: "language", label: "Language" },
        { key: "created", label: "Created" },
      ]} rows={(versions.data?.result ?? []).map(item => ({
        id: item.id,
        class: item.class_name,
        language: item.language,
        created: item.created_on,
      }))} emptyLabel="No versions found." />
      <div className="mt-6">
        <SectionHeader title="Instances" />
        <DataTable columns={[
          { key: "id", label: "Instance" },
          { key: "status", label: "Status" },
          { key: "created", label: "Created" },
          { key: "actions", label: "" },
        ]} rows={(instances.data?.result ?? []).map(item => ({
          id: item.id,
          status: <StatusBadge value={item.status} />,
          created: item.created_on,
          actions: <Button variant="secondary" onClick={() => setSelectedInstanceId(item.id)}>Inspect</Button>,
        }))} emptyLabel="No instances found." />
      </div>
      {selectedInstanceId ? <div className="mt-6">
        <SectionHeader title={`Instance ${selectedInstanceId}`} />
        {instance.isLoading ? <LoadingState /> : instance.error ? <ErrorState message="Unable to load the selected instance." /> : <>
          <div className="mb-4 flex flex-wrap gap-2">
            {selectedStatus === "paused" ? (
              <Button variant="primary" disabled={actionMutation.isPending} onClick={() => actionMutation.mutate("resume")}>Resume</Button>
            ) : (
              <Button variant="secondary" disabled={actionMutation.isPending || selectedStatus === "terminated" || selectedStatus === "complete"} onClick={() => actionMutation.mutate("pause")}>Pause</Button>
            )}
            <Button variant="secondary" disabled={actionMutation.isPending} onClick={() => setConfirmedAction("restart")}>Restart</Button>
            <Button variant="destructive" disabled={actionMutation.isPending || selectedStatus === "terminated"} onClick={() => setConfirmedAction("terminate")}>Terminate</Button>
          </div>
          <DataTable columns={[{ key: "name", label: "Step" }, { key: "type", label: "Type" }, { key: "status", label: "Status" }]} rows={(instance.data?.steps ?? []).map(step => ({
            name: "name" in step ? step.name : "termination",
            type: step.type,
            status: "success" in step ? (step.success === true ? "complete" : step.success === false ? "errored" : "running") : "finished" in step ? (step.finished ? "complete" : "waiting") : "terminated",
          }))} emptyLabel="No execution steps found." />
          <div className="mt-6 grid gap-3 sm:grid-cols-[1fr_2fr_auto] sm:items-end">
            <Input label="Event type" value={eventType} onChange={event => setEventType(event.target.value)} placeholder="approval" />
            <Input label="JSON body (optional)" value={eventBody} onChange={event => setEventBody(event.target.value)} placeholder='{"approved":true}' />
            <Button variant="primary" disabled={eventMutation.isPending || !eventType.trim()} onClick={() => eventMutation.mutate()}>Send event</Button>
          </div>
          {mutationError ? <p className="mt-3 text-sm text-kumo-danger" role="alert">{mutationError}</p> : null}
        </>}
      </div> : null}
    </>}
  </div>;
}
