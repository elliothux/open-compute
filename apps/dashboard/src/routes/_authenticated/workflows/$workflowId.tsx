import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Input, Textarea } from "@cloudflare/kumo/components/input";
import { Select } from "@cloudflare/kumo/components/select";
import { Tabs } from "@cloudflare/kumo/components/tabs";
import {
  IconPlayerPause,
  IconPlayerPlay,
  IconPlayerStop,
  IconPlus,
  IconRotateClockwise,
  IconSettings,
  IconTrash,
} from "@tabler/icons-react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import type {
  InstanceGetResponse,
  WorkflowsInstancesInstancesInstanceListResponse as InstanceListRecord,
  WorkflowsVersionsVersionGetResponse as VersionDetail,
  WorkflowsVersionsVersionListResponse as VersionRecord,
  WorkflowGetResponse,
} from "@open-compute/sdk";
import {
  DefinitionList,
  ErrorState,
  LoadingRows,
  PageHeader,
  Panel,
  Section,
  StatGrid,
} from "../../../components/dashboard-page";
import { DataTable, StatusBadge } from "../../../components/page-layout";
import { openConfirmDeleteDialog } from "../../../components/resource-dialog";
import { useAuth } from "../../../features/auth/auth-atoms";
import { useMutationFeedback } from "../../../features/toast/use-mutation-feedback";
import { formatDateTime as formatDate } from "../../../lib/format";
import { workflowQuery } from "../../../lib/query-options";

type DefinitionDraft = {
  scriptName: string;
  className: string;
  successRetention: string;
  errorRetention: string;
};
type InstanceRequest =
  | { mode: "single"; id: string; params: string }
  | { mode: "batch"; body: string };
type WorkflowAction = "pause" | "resume" | "restart" | "terminate";
type Tab = "overview" | "versions" | "instances" | "settings";

export const Route = createFileRoute("/_authenticated/workflows/$workflowId")({
  validateSearch: (search: Record<string, unknown>): { tab?: Tab } =>
    search.tab === "versions" ||
    search.tab === "instances" ||
    search.tab === "settings"
      ? { tab: search.tab }
      : {},
  loader: ({ context, params }) => {
    const { client, instanceId } = context.auth;
    if (!client || !instanceId) return;
    return context.queryClient.ensureQueryData(
      workflowQuery(client, instanceId, params.workflowId),
    );
  },
  component: WorkflowDetailPage,
});

function WorkflowDetailPage() {
  const { workflowId } = Route.useParams();
  const { tab: searchTab } = Route.useSearch();
  const tab = searchTab ?? "overview";
  const navigate = useNavigate();
  const { client, instanceId: selectedInstanceId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const enabled = client !== null && selectedInstanceId !== null;
  const [definitionOpen, setDefinitionOpen] = useState(false);
  const [instanceOpen, setInstanceOpen] = useState(false);
  const [selectedVersion, setSelectedVersion] = useState<string | null>(null);
  const [selectedInstance, setSelectedInstance] = useState<string | null>(null);
  const [confirmedAction, setConfirmedAction] = useState<WorkflowAction | null>(
    null,
  );
  const [eventType, setEventType] = useState("");
  const [eventBody, setEventBody] = useState("");

  const workflow = useQuery(
    workflowQuery(client, selectedInstanceId, workflowId),
  );
  const versions = useQuery({
    queryKey: [
      "cloudflare-v4",
      "workflows",
      selectedInstanceId,
      workflowId,
      "versions",
    ],
    queryFn: ({ signal }) =>
      client!.workflows.versions.list(
        workflowId,
        { account_id: selectedInstanceId! },
        { signal },
      ),
    enabled,
  });
  const version = useQuery({
    queryKey: [
      "cloudflare-v4",
      "workflows",
      workflowId,
      "versions",
      selectedVersion,
    ],
    queryFn: ({ signal }) =>
      client!.workflows.versions.get(
        selectedVersion!,
        { account_id: selectedInstanceId!, workflow_name: workflowId },
        { signal },
      ),
    enabled: enabled && selectedVersion !== null,
  });
  const instances = useQuery({
    queryKey: [
      "cloudflare-v4",
      "workflows",
      selectedInstanceId,
      workflowId,
      "instances",
    ],
    queryFn: ({ signal }) =>
      client!.workflows.instances.list(
        workflowId,
        { account_id: selectedInstanceId! },
        { signal },
      ),
    enabled,
  });
  const instance = useQuery({
    queryKey: [
      "cloudflare-v4",
      "workflows",
      workflowId,
      "instances",
      selectedInstance,
    ],
    queryFn: ({ signal }) =>
      client!.workflows.instances.get(
        selectedInstance!,
        { account_id: selectedInstanceId!, workflow_name: workflowId },
        { signal },
      ),
    enabled: enabled && selectedInstance !== null,
  });
  const settings = useQuery({
    queryKey: ["open-compute", "workflows", "settings", selectedInstanceId],
    queryFn: ({ signal }) =>
      client!.openCompute.workflows.settings(selectedInstanceId!, { signal }),
    enabled,
  });

  const refreshDefinition = () =>
    Promise.all([
      queryClient.invalidateQueries({
        queryKey: [
          "cloudflare-v4",
          "workflows",
          selectedInstanceId,
          workflowId,
        ],
      }),
      queryClient.invalidateQueries({
        queryKey: [
          "cloudflare-v4",
          "workflows",
          selectedInstanceId,
          workflowId,
          "versions",
        ],
      }),
    ]);
  const refreshInstances = () =>
    queryClient.invalidateQueries({
      queryKey: [
        "cloudflare-v4",
        "workflows",
        selectedInstanceId,
        workflowId,
        "instances",
      ],
    });

  const saveDefinition = useMutation({
    mutationFn: (draft: DefinitionDraft) =>
      client!.workflows.update(workflowId, {
        account_id: selectedInstanceId!,
        script_name: draft.scriptName,
        class_name: draft.className,
        ...retentionParams(draft),
      }),
    onSuccess: async () => {
      setDefinitionOpen(false);
      await refreshDefinition();
      feedback.success("Workflow definition updated.");
    },
    onError: (error) =>
      feedback.failure(error, "Unable to update the workflow."),
  });
  const createInstance = useMutation({
    mutationFn: async (request: InstanceRequest) => {
      if (request.mode === "batch") {
        const parsed = JSON.parse(request.body) as unknown;
        if (!Array.isArray(parsed))
          throw new Error("Batch input must be a JSON array.");
        await client!.workflows.instances.bulk(workflowId, {
          account_id: selectedInstanceId!,
          body: parsed.map((entry) => {
            if (
              typeof entry !== "object" ||
              entry === null ||
              Array.isArray(entry)
            ) {
              throw new Error("Each batch item must be an object.");
            }
            const item = entry as { instance_id?: unknown; params?: unknown };
            if (
              item.instance_id !== undefined &&
              typeof item.instance_id !== "string"
            ) {
              throw new Error("instance_id must be a string.");
            }
            return {
              ...(item.instance_id ? { instance_id: item.instance_id } : {}),
              ...(item.params === undefined
                ? {}
                : { params: JSON.stringify(item.params) }),
            };
          }),
        });
        return;
      }
      await client!.workflows.instances.create(workflowId, {
        account_id: selectedInstanceId!,
        ...(request.id.trim() ? { instance_id: request.id.trim() } : {}),
        ...(request.params.trim()
          ? { params: JSON.stringify(JSON.parse(request.params) as unknown) }
          : {}),
      });
    },
    onSuccess: async () => {
      setInstanceOpen(false);
      await refreshInstances();
      feedback.success("Workflow instance created.");
    },
    onError: (error) =>
      feedback.failure(error, "Unable to create the instance."),
  });
  const status = useMutation({
    mutationFn: (action: WorkflowAction) =>
      client!.workflows.instances.status.edit(selectedInstance!, {
        account_id: selectedInstanceId!,
        workflow_name: workflowId,
        status: action,
      }),
    onSuccess: async (_data, action) => {
      setConfirmedAction(null);
      await refreshInstances();
      feedback.success(`Instance ${action} request completed.`);
    },
    onError: (error) =>
      feedback.failure(error, "Unable to update instance status."),
  });
  const sendEvent = useMutation({
    mutationFn: () =>
      client!.workflows.instances.events.create(eventType.trim(), {
        account_id: selectedInstanceId!,
        workflow_name: workflowId,
        instance_id: selectedInstance!,
        ...(eventBody.trim() ? { body: JSON.parse(eventBody) as unknown } : {}),
      }),
    onSuccess: async () => {
      setEventBody("");
      await refreshInstances();
      feedback.success("Event sent.");
    },
    onError: (error) => feedback.failure(error, "Unable to send the event."),
  });
  function confirmDeleteWorkflow() {
    openConfirmDeleteDialog({
      name: workflow.data?.name ?? workflowId,
      confirm: async () => {
        try {
          await client!.workflows.delete(workflowId, {
            account_id: selectedInstanceId!,
          });
        } catch (error) {
          feedback.failure(error, "Unable to delete the workflow.");
          throw error;
        }
        feedback.success("Workflow deleted.");
        await navigate({ to: "/workflows" });
      },
    });
  }

  const loading =
    workflow.isLoading || versions.isLoading || instances.isLoading;
  const error = workflow.error ?? versions.error ?? instances.error;
  const currentVersion = versions.data?.result[0];
  return (
    <div className="text-sm leading-5">
      <PageHeader
        title={workflow.data?.name ?? workflowId}
        description="Inspect versions and manage durable workflow instances."
        actions={
          <Button
            variant="primary"
            icon={<IconPlus size={16} />}
            disabled={!workflow.data}
            onClick={() => setInstanceOpen(true)}
          >
            Create instance
          </Button>
        }
      />
      <div className="mb-6 overflow-x-auto">
        <Tabs
          variant="underline"
          value={tab}
          onValueChange={(value) =>
            void navigate({
              to: "/workflows/$workflowId",
              params: { workflowId },
              search: value === "overview" ? {} : { tab: value as Tab },
            })
          }
          tabs={[
            { value: "overview", label: "Overview" },
            { value: "versions", label: "Versions" },
            { value: "instances", label: "Instances" },
            { value: "settings", label: "Settings" },
          ]}
          listClassName="min-w-max"
        />
      </div>

      {loading ? (
        <LoadingRows />
      ) : error ? (
        <ErrorState error={error} />
      ) : tab === "overview" ? (
        <OverviewTab workflow={workflow.data!} />
      ) : tab === "versions" ? (
        <VersionsTab
          versions={versions.data?.result ?? []}
          selected={selectedVersion}
          detail={version.data}
          loading={version.isLoading}
          error={version.error}
          onSelect={setSelectedVersion}
        />
      ) : tab === "instances" ? (
        <InstancesTab
          instances={instances.data?.result ?? []}
          selectedId={selectedInstance}
          detail={instance.data}
          loading={instance.isLoading}
          error={instance.error}
          actionPending={status.isPending}
          eventType={eventType}
          eventBody={eventBody}
          eventPending={sendEvent.isPending}
          onSelect={setSelectedInstance}
          onCreate={() => setInstanceOpen(true)}
          onAction={setConfirmedAction}
          onEventType={setEventType}
          onEventBody={setEventBody}
          onSendEvent={() => sendEvent.mutate()}
        />
      ) : (
        <SettingsTab
          workflow={workflow.data!}
          version={currentVersion}
          defaults={settings.data?.default_retention}
          onEdit={() => setDefinitionOpen(true)}
          onDelete={() => confirmDeleteWorkflow()}
        />
      )}

      <DefinitionDialog
        open={definitionOpen}
        initial={{
          scriptName: workflow.data?.script_name ?? "",
          className: workflow.data?.class_name ?? "",
          successRetention: String(
            currentVersion?.default_retention?.success_retention ?? "",
          ),
          errorRetention: String(
            currentVersion?.default_retention?.error_retention ?? "",
          ),
        }}
        pending={saveDefinition.isPending}
        error={saveDefinition.error}
        onOpenChange={setDefinitionOpen}
        onSubmit={(draft) => saveDefinition.mutate(draft)}
      />
      <InstanceDialog
        open={instanceOpen}
        pending={createInstance.isPending}
        error={createInstance.error}
        onOpenChange={setInstanceOpen}
        onSubmit={(request) => createInstance.mutate(request)}
      />
      <StatusDialog
        open={confirmedAction !== null}
        action={confirmedAction ?? "pause"}
        instanceId={selectedInstance ?? ""}
        pending={status.isPending}
        error={status.error}
        onOpenChange={(open) => {
          if (!open) setConfirmedAction(null);
        }}
        onConfirm={() => {
          if (confirmedAction) status.mutate(confirmedAction);
        }}
      />
    </div>
  );
}

function OverviewTab({ workflow }: { workflow: WorkflowSummary }) {
  return (
    <div className="grid gap-6">
      <StatGrid
        items={[
          { label: "Running", value: workflow.instances.running ?? 0 },
          { label: "Completed", value: workflow.instances.complete ?? 0 },
          { label: "Errored", value: workflow.instances.errored ?? 0 },
          { label: "Paused", value: workflow.instances.paused ?? 0 },
        ]}
      />
      <Section
        title="Definition"
        description="The Worker entrypoint currently associated with this workflow."
      >
        <Panel>
          <DefinitionList
            items={[
              { label: "Worker script", value: workflow.script_name },
              { label: "Exported class", value: workflow.class_name },
              { label: "Created", value: formatDate(workflow.created_on) },
              {
                label: "Last modified",
                value: formatDate(workflow.modified_on),
              },
            ]}
          />
        </Panel>
      </Section>
    </div>
  );
}

function VersionsTab({
  versions,
  selected,
  detail,
  loading,
  error,
  onSelect,
}: {
  versions: readonly VersionRecord[];
  selected: string | null;
  detail: VersionDetail | undefined;
  loading: boolean;
  error: Error | null;
  onSelect: (id: string) => void;
}) {
  return (
    <div className="grid gap-6">
      <Section
        title="Versions"
        description="Every definition update creates an immutable workflow version."
      >
        <DataTable
          columns={[
            { key: "version", label: "Version" },
            { key: "class", label: "Class" },
            { key: "language", label: "Language" },
            { key: "created", label: "Created" },
            { key: "actions", label: "" },
          ]}
          rows={versions.map((item) => ({
            version: <code className="font-mono text-xs">{item.id}</code>,
            class: item.class_name,
            language: item.language,
            created: formatDate(item.created_on),
            actions: (
              <Button variant="secondary" onClick={() => onSelect(item.id)}>
                View details
              </Button>
            ),
          }))}
          emptyLabel="No versions found."
        />
      </Section>
      {selected ? (
        <Section title="Version details">
          {loading ? (
            <LoadingRows count={1} />
          ) : error ? (
            <ErrorState error={error} />
          ) : detail ? (
            <Panel>
              <DefinitionList
                items={[
                  {
                    label: "Version ID",
                    value: (
                      <code className="font-mono text-xs">{detail.id}</code>
                    ),
                  },
                  { label: "Class", value: detail.class_name },
                  { label: "Language", value: detail.language },
                  {
                    label: "Maximum steps",
                    value: detail.limits?.steps ?? "Default",
                  },
                  {
                    label: "Success retention",
                    value: formatDuration(
                      detail.default_retention?.success_retention,
                    ),
                  },
                  {
                    label: "Error retention",
                    value: formatDuration(
                      detail.default_retention?.error_retention,
                    ),
                  },
                ]}
              />
            </Panel>
          ) : null}
        </Section>
      ) : null}
    </div>
  );
}

function InstancesTab(props: {
  instances: readonly InstanceListRecord[];
  selectedId: string | null;
  detail: InstanceGetResponse | undefined;
  loading: boolean;
  error: Error | null;
  actionPending: boolean;
  eventType: string;
  eventBody: string;
  eventPending: boolean;
  onSelect: (id: string) => void;
  onCreate: () => void;
  onAction: (action: WorkflowAction) => void;
  onEventType: (value: string) => void;
  onEventBody: (value: string) => void;
  onSendEvent: () => void;
}) {
  return (
    <div className="grid gap-6">
      <Section
        title="Instances"
        description="Executions created for this workflow definition."
      >
        <div className="flex justify-end">
          <Button
            variant="primary"
            icon={<IconPlus size={16} />}
            onClick={props.onCreate}
          >
            Create instance
          </Button>
        </div>
        <DataTable
          columns={[
            { key: "id", label: "Instance" },
            { key: "status", label: "Status" },
            { key: "trigger", label: "Trigger" },
            { key: "created", label: "Created" },
            { key: "actions", label: "" },
          ]}
          rows={props.instances.map((item) => ({
            id: <code className="font-mono text-xs">{item.id}</code>,
            status: <StatusBadge value={item.status} />,
            trigger: item.trigger_source ?? "unknown",
            created: formatDate(item.created_on),
            actions: (
              <Button
                variant="secondary"
                onClick={() => props.onSelect(item.id)}
              >
                Inspect
              </Button>
            ),
          }))}
          emptyLabel="No instances found."
        />
      </Section>
      {props.selectedId ? (
        <Section title="Instance details" description={props.selectedId}>
          {props.loading ? (
            <LoadingRows count={2} />
          ) : props.error ? (
            <ErrorState error={props.error} />
          ) : props.detail ? (
            <div className="grid gap-4">
              <div className="flex flex-wrap items-center gap-2">
                <StatusBadge value={props.detail.status} />
                <Button
                  variant="secondary"
                  icon={
                    props.detail.status === "paused" ? (
                      <IconPlayerPlay size={16} />
                    ) : (
                      <IconPlayerPause size={16} />
                    )
                  }
                  disabled={
                    props.actionPending ||
                    ["complete", "terminated", "errored"].includes(
                      props.detail.status,
                    )
                  }
                  onClick={() =>
                    props.onAction(
                      props.detail?.status === "paused" ? "resume" : "pause",
                    )
                  }
                >
                  {props.detail.status === "paused" ? "Resume" : "Pause"}
                </Button>
                <Button
                  variant="secondary"
                  icon={<IconRotateClockwise size={16} />}
                  disabled={props.actionPending}
                  onClick={() => props.onAction("restart")}
                >
                  Restart
                </Button>
                <Button
                  variant="destructive"
                  icon={<IconPlayerStop size={16} />}
                  disabled={
                    props.actionPending || props.detail.status === "terminated"
                  }
                  onClick={() => props.onAction("terminate")}
                >
                  Terminate
                </Button>
              </div>
              <Panel>
                <DefinitionList
                  items={[
                    {
                      label: "Version",
                      value: (
                        <code className="font-mono text-xs">
                          {props.detail.versionId}
                        </code>
                      ),
                    },
                    { label: "Queued", value: formatDate(props.detail.queued) },
                    {
                      label: "Started",
                      value: props.detail.start
                        ? formatDate(props.detail.start)
                        : "Not started",
                    },
                    {
                      label: "Ended",
                      value: props.detail.end
                        ? formatDate(props.detail.end)
                        : "In progress",
                    },
                    { label: "Steps", value: props.detail.step_count },
                  ]}
                />
              </Panel>
              <DataTable
                columns={[
                  { key: "name", label: "Step" },
                  { key: "type", label: "Type" },
                  { key: "status", label: "Status" },
                ]}
                rows={props.detail.steps.map((step) => ({
                  name: "name" in step ? step.name : "Termination",
                  type: step.type,
                  status: <StatusBadge value={stepStatus(step)} />,
                }))}
                emptyLabel="No execution steps found."
              />
              <Panel className="grid gap-4">
                <div className="grid gap-1">
                  <h3 className="font-medium">Send event</h3>
                  <p className="text-kumo-subtle">
                    Deliver an event to a running instance waiting for this
                    event type.
                  </p>
                </div>
                <div className="grid gap-3 md:grid-cols-4 md:items-end">
                  <Input
                    label="Event type"
                    value={props.eventType}
                    onChange={(event) => props.onEventType(event.target.value)}
                  />
                  <Input
                    className="md:col-span-2"
                    label="JSON body (optional)"
                    value={props.eventBody}
                    onChange={(event) => props.onEventBody(event.target.value)}
                  />
                  <Button
                    variant="primary"
                    disabled={
                      !props.eventType.trim() ||
                      props.eventPending ||
                      ["complete", "terminated", "errored"].includes(
                        props.detail.status,
                      )
                    }
                    onClick={props.onSendEvent}
                  >
                    Send event
                  </Button>
                </div>
              </Panel>
            </div>
          ) : null}
        </Section>
      ) : null}
    </div>
  );
}

function SettingsTab({
  workflow,
  version,
  defaults,
  onEdit,
  onDelete,
}: {
  workflow: WorkflowSummary;
  version: VersionRecord | undefined;
  defaults: { success_retention: number; error_retention: number } | undefined;
  onEdit: () => void;
  onDelete: () => void;
}) {
  return (
    <div className="grid gap-6">
      <Section
        title="Definition settings"
        description="Worker binding and per-version retention configuration."
      >
        <Panel>
          <DefinitionList
            items={[
              { label: "Worker script", value: workflow.script_name },
              { label: "Exported class", value: workflow.class_name },
              {
                label: "Success retention",
                value: formatDuration(
                  version?.default_retention?.success_retention ??
                    defaults?.success_retention,
                ),
              },
              {
                label: "Error retention",
                value: formatDuration(
                  version?.default_retention?.error_retention ??
                    defaults?.error_retention,
                ),
              },
            ]}
          />
          <div className="border-kumo-line mt-4 border-t pt-4">
            <Button
              variant="primary"
              icon={<IconSettings size={16} />}
              onClick={onEdit}
            >
              Edit definition
            </Button>
          </div>
        </Panel>
      </Section>
      <Section
        title="Delete workflow"
        description="Instances and version history for this workflow will no longer be available."
      >
        <Panel className="flex flex-wrap items-center justify-between gap-4">
          <p className="text-kumo-subtle">This action cannot be undone.</p>
          <Button
            variant="destructive"
            icon={<IconTrash size={16} />}
            onClick={onDelete}
          >
            Delete workflow
          </Button>
        </Panel>
      </Section>
    </div>
  );
}

// Dialog roots and content stay mounted so Kumo can animate them.
function DefinitionDialog({
  open,
  initial,
  pending,
  error,
  onOpenChange,
  onSubmit,
}: {
  open: boolean;
  initial: DefinitionDraft;
  pending: boolean;
  error: unknown;
  onOpenChange: (open: boolean) => void;
  onSubmit: (draft: DefinitionDraft) => void;
}) {
  const [draft, setDraft] = useState(initial);
  useEffect(() => {
    if (open) {
      // oxlint-disable-next-line react/set-state-in-effect -- reset the mounted form when it opens
      setDraft(initial);
    }
  }, [initial, open]);
  const valid = Boolean(draft.scriptName.trim() && draft.className.trim());
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog className="px-6 py-5" size="lg">
        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (valid && !pending) onSubmit(draft);
          }}
        >
          <Dialog.Title>Edit workflow definition</Dialog.Title>
          <Dialog.Description>
            Update the Worker entrypoint and default instance retention.
          </Dialog.Description>
          <div className="mt-5 grid gap-4 md:grid-cols-2">
            <Input
              label="Worker script"
              value={draft.scriptName}
              onChange={(event) =>
                setDraft({ ...draft, scriptName: event.target.value })
              }
            />
            <Input
              label="Exported class"
              value={draft.className}
              onChange={(event) =>
                setDraft({ ...draft, className: event.target.value })
              }
            />
            <Input
              label="Success retention (ms)"
              type="number"
              min={1}
              value={draft.successRetention}
              onChange={(event) =>
                setDraft({ ...draft, successRetention: event.target.value })
              }
            />
            <Input
              label="Error retention (ms)"
              type="number"
              min={1}
              value={draft.errorRetention}
              onChange={(event) =>
                setDraft({ ...draft, errorRetention: event.target.value })
              }
            />
          </div>
          <DialogError error={error} />
          <DialogActions
            pending={pending}
            valid={valid}
            onCancel={() => onOpenChange(false)}
            submit="Save definition"
          />
        </form>
      </Dialog>
    </Dialog.Root>
  );
}

function InstanceDialog({
  open,
  pending,
  error,
  onOpenChange,
  onSubmit,
}: {
  open: boolean;
  pending: boolean;
  error: unknown;
  onOpenChange: (open: boolean) => void;
  onSubmit: (request: InstanceRequest) => void;
}) {
  const [mode, setMode] = useState<"single" | "batch">("single");
  const [id, setId] = useState("");
  const [params, setParams] = useState("");
  const [batch, setBatch] = useState(
    '[\n  { "instance_id": "example", "params": {} }\n]',
  );
  useEffect(() => {
    if (open) {
      // oxlint-disable-next-line react/set-state-in-effect -- reset the mounted form when it opens
      setMode("single");
      setId("");
      setParams("");
    }
  }, [open]);
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog className="px-6 py-5" size="lg">
        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (!pending)
              onSubmit(
                mode === "single"
                  ? { mode, id, params }
                  : { mode, body: batch },
              );
          }}
        >
          <Dialog.Title>Create workflow instance</Dialog.Title>
          <Dialog.Description>
            Start one instance or submit a JSON batch.
          </Dialog.Description>
          <div className="mt-5 grid gap-4">
            <Select
              label="Creation mode"
              value={mode}
              onValueChange={(value) => setMode(value ?? "single")}
            >
              <Select.Option value="single">Single instance</Select.Option>
              <Select.Option value="batch">Batch</Select.Option>
            </Select>
            {mode === "single" ? (
              <>
                <Input
                  label="Instance ID (optional)"
                  value={id}
                  onChange={(event) => setId(event.target.value)}
                />
                <Textarea
                  label="JSON parameters (optional)"
                  value={params}
                  onChange={(event) => setParams(event.target.value)}
                  rows={6}
                />
              </>
            ) : (
              <Textarea
                label="Batch JSON array"
                value={batch}
                onChange={(event) => setBatch(event.target.value)}
                rows={10}
              />
            )}
          </div>
          <DialogError error={error} />
          <DialogActions
            pending={pending}
            valid={mode === "single" || Boolean(batch.trim())}
            onCancel={() => onOpenChange(false)}
            submit={mode === "single" ? "Create instance" : "Create batch"}
          />
        </form>
      </Dialog>
    </Dialog.Root>
  );
}

function StatusDialog({
  open,
  action,
  instanceId,
  pending,
  error,
  onOpenChange,
  onConfirm,
}: {
  open: boolean;
  action: WorkflowAction;
  instanceId: string;
  pending: boolean;
  error: unknown;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog className="px-6 py-5" size="lg">
        <Dialog.Title>{actionLabel(action)} instance</Dialog.Title>
        <Dialog.Description>
          {actionLabel(action)} instance{" "}
          <code className="font-mono text-xs">{instanceId}</code>?
        </Dialog.Description>
        <DialogError error={error} />
        <div className="mt-6 flex justify-end gap-2">
          <Button
            variant="secondary"
            disabled={pending}
            onClick={() => onOpenChange(false)}
          >
            Cancel
          </Button>
          <Button
            variant={action === "terminate" ? "destructive" : "primary"}
            disabled={pending}
            onClick={onConfirm}
          >
            {pending ? "Working…" : actionLabel(action)}
          </Button>
        </div>
      </Dialog>
    </Dialog.Root>
  );
}

function DialogError({ error }: { error: unknown }) {
  return error ? (
    <p className="text-kumo-danger mt-3" role="alert">
      {error instanceof Error ? error.message : "The request failed."}
    </p>
  ) : null;
}
function DialogActions({
  pending,
  valid,
  onCancel,
  submit,
}: {
  pending: boolean;
  valid: boolean;
  onCancel: () => void;
  submit: string;
}) {
  return (
    <div className="mt-6 flex justify-end gap-2">
      <Button
        type="button"
        variant="secondary"
        disabled={pending}
        onClick={onCancel}
      >
        Cancel
      </Button>
      <Button type="submit" variant="primary" disabled={!valid || pending}>
        {pending ? "Saving…" : submit}
      </Button>
    </div>
  );
}

type WorkflowSummary = WorkflowGetResponse;

function retentionParams(draft: DefinitionDraft) {
  return draft.successRetention || draft.errorRetention
    ? {
        default_retention: {
          ...(draft.successRetention
            ? { success_retention: Number(draft.successRetention) }
            : {}),
          ...(draft.errorRetention
            ? { error_retention: Number(draft.errorRetention) }
            : {}),
        },
      }
    : {};
}
function actionLabel(action: WorkflowAction) {
  return `${action[0]?.toUpperCase()}${action.slice(1)}`;
}
function formatDuration(value?: number) {
  return value === undefined
    ? "Platform default"
    : `${Math.round(value / 86_400_000)} days`;
}
function stepStatus(step: InstanceGetResponse["steps"][number]) {
  if (step.type === "termination") return "terminated";
  if (
    ("success" in step && step.success === true) ||
    ("finished" in step && step.finished === true)
  )
    return "complete";
  if (
    ("success" in step && step.success === false) ||
    ("error" in step && step.error)
  )
    return "errored";
  return "finished" in step && step.finished === false ? "waiting" : "running";
}
