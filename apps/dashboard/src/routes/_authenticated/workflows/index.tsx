import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import {
  IconAlertCircle,
  IconCircleCheck,
  IconPlus,
} from "@tabler/icons-react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { CloudflareProductIcon } from "../../../components/cloudflare-product-icons";
import {
  CatalogToolbar,
  EmptyState,
  ErrorState,
  LoadingRows,
  PageHeader,
  Panel,
  ResourceList,
  ResourceRow,
} from "../../../components/dashboard-page";
import { closeDialog, openDialog } from "../../../components/dialog-manager";
import { openConfirmDeleteDialog } from "../../../components/resource-dialog";
import { RowActionsMenu } from "../../../components/row-actions-menu";
import { useAuth } from "../../../features/auth/auth-atoms";
import { useMutationFeedback } from "../../../features/toast/use-mutation-feedback";
import { formatDate } from "../../../lib/format";

type WorkflowDraft = {
  name: string;
  scriptName: string;
  className: string;
  successRetention: string;
  errorRetention: string;
};

export const Route = createFileRoute("/_authenticated/workflows/")({
  validateSearch: (search: Record<string, unknown>): { q?: string } =>
    typeof search.q === "string" && search.q ? { q: search.q } : {},
  component: WorkflowsPage,
});

function WorkflowsPage() {
  const navigate = useNavigate();
  const { q: search = "" } = Route.useSearch();
  const { client, instanceId: selectedInstanceId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const enabled = client !== null && selectedInstanceId !== null;

  const workflows = useQuery({
    queryKey: ["cloudflare-v4", "workflows", selectedInstanceId],
    queryFn: ({ signal }) =>
      client!.workflows.list({ account_id: selectedInstanceId! }, { signal }),
    enabled,
  });
  const settings = useQuery({
    queryKey: ["open-compute", "workflows", "settings", selectedInstanceId],
    queryFn: ({ signal }) =>
      client!.openCompute.workflows.settings(selectedInstanceId!, { signal }),
    enabled,
  });
  const refresh = () =>
    queryClient.invalidateQueries({
      queryKey: ["cloudflare-v4", "workflows", selectedInstanceId],
    });
  function openWorkflowEditor(initial: WorkflowDraft) {
    openDialog({
      title: "Edit workflow",
      description:
        "Connect a Worker class and optionally override instance retention.",
      size: "lg",
      contentClassName: "px-6 py-5",
      content: (
        <WorkflowForm
          initial={initial}
          submit={async (draft) => {
            try {
              await client!.workflows.update(draft.name, {
                account_id: selectedInstanceId!,
                script_name: draft.scriptName,
                class_name: draft.className,
                ...retentionParams(draft),
              });
            } catch (error) {
              feedback.failure(error, "Unable to save the workflow.");
              throw error;
            }
            await refresh();
            feedback.success("Workflow saved.");
          }}
        />
      ),
    });
  }

  function confirmDeleteWorkflow(name: string) {
    openConfirmDeleteDialog({
      name,
      confirm: async () => {
        try {
          await client!.workflows.delete(name, {
            account_id: selectedInstanceId!,
          });
        } catch (error) {
          feedback.failure(error, "Unable to delete the workflow.");
          throw error;
        }
        await refresh();
        feedback.success("Workflow deleted.");
      },
    });
  }

  const rows = useMemo(() => {
    const query = search.trim().toLowerCase();
    return (workflows.data?.result ?? []).filter((workflow) =>
      workflow.name.toLowerCase().includes(query),
    );
  }, [search, workflows.data]);
  const totals = (workflows.data?.result ?? []).reduce(
    (sum, workflow) => ({
      complete: sum.complete + (workflow.instances.complete ?? 0),
      errored: sum.errored + (workflow.instances.errored ?? 0),
    }),
    { complete: 0, errored: 0 },
  );

  return (
    <div className="text-sm leading-5">
      <PageHeader
        title="Workflows"
        description="Build durable, multi-step applications on Cloudflare Workers."
        actions={
          <Button
            variant="primary"
            icon={<IconPlus size={16} />}
            onClick={() => void navigate({ to: "/workflows/new" })}
          >
            Create workflow
          </Button>
        }
      />

      <div className="grid gap-6 md:grid-cols-3">
        <div className="min-w-0 md:col-span-2">
          <CatalogToolbar
            value={search}
            onChange={(value) =>
              void navigate({
                to: "/workflows",
                search: value ? { q: value } : {},
                replace: true,
              })
            }
            onRefresh={() => void workflows.refetch()}
            refreshing={workflows.isFetching}
            placeholder="Search workflows"
          />
          {workflows.isLoading ? (
            <LoadingRows />
          ) : workflows.error ? (
            <ErrorState error={workflows.error} />
          ) : rows.length === 0 ? (
            <EmptyState
              title={search ? "No matching workflows" : "No workflows found"}
              description={
                search
                  ? "Try a different search term."
                  : "Create a workflow and begin building durable applications."
              }
              action={
                search ? undefined : (
                  <Button
                    variant="primary"
                    onClick={() => void navigate({ to: "/workflows/new" })}
                  >
                    Create workflow
                  </Button>
                )
              }
            />
          ) : (
            <ResourceList>
              {rows.map((workflow) => (
                <ResourceRow
                  key={workflow.id}
                  href={`/workflows/${encodeURIComponent(workflow.name)}`}
                  icon={<CloudflareProductIcon product="Workflows" size={18} />}
                  title={workflow.name}
                  description={`${workflow.script_name} · ${workflow.class_name}`}
                  meta={`${workflow.instances.running ?? 0} running`}
                  footer={
                    <div className="flex items-center justify-between gap-3">
                      <span>Updated {formatDate(workflow.modified_on)}</span>
                      <RowActionsMenu
                        label={workflow.name}
                        actions={[
                          {
                            id: "edit",
                            label: "Edit definition",
                            onSelect: () =>
                              openWorkflowEditor({
                                name: workflow.name,
                                scriptName: workflow.script_name,
                                className: workflow.class_name,
                                successRetention: "",
                                errorRetention: "",
                              }),
                          },
                          {
                            id: "delete",
                            label: "Delete workflow",
                            variant: "danger",
                            onSelect: () =>
                              confirmDeleteWorkflow(workflow.name),
                          },
                        ]}
                      />
                    </div>
                  }
                />
              ))}
            </ResourceList>
          )}
        </div>

        <aside className="grid content-start gap-4">
          <div className="grid gap-1">
            <h2 className="text-base font-semibold">Usage</h2>
            <p className="text-kumo-subtle">Across all workflow definitions</p>
          </div>
          <div className="grid grid-cols-2 gap-3 md:grid-cols-1 xl:grid-cols-2">
            <Panel className="grid gap-1">
              <span className="text-kumo-subtle flex items-center gap-1.5">
                <IconCircleCheck size={16} /> Completed
              </span>
              <span className="text-xl font-semibold">{totals.complete}</span>
            </Panel>
            <Panel className="grid gap-1">
              <span className="text-kumo-subtle flex items-center gap-1.5">
                <IconAlertCircle size={16} /> Errored
              </span>
              <span className="text-xl font-semibold">{totals.errored}</span>
            </Panel>
          </div>
          <Panel className="grid gap-3">
            <div className="grid gap-1">
              <h2 className="text-base font-semibold">Default retention</h2>
              <p className="text-kumo-subtle">Applied to new instances</p>
            </div>
            {settings.isLoading ? (
              <span className="text-kumo-subtle">Loading…</span>
            ) : settings.error ? (
              <span className="text-kumo-danger">Unable to load settings.</span>
            ) : (
              <dl className="grid gap-2">
                <div className="flex justify-between gap-3">
                  <dt className="text-kumo-subtle">Success</dt>
                  <dd>
                    {formatDuration(
                      settings.data?.default_retention.success_retention,
                    )}
                  </dd>
                </div>
                <div className="flex justify-between gap-3">
                  <dt className="text-kumo-subtle">Error</dt>
                  <dd>
                    {formatDuration(
                      settings.data?.default_retention.error_retention,
                    )}
                  </dd>
                </div>
              </dl>
            )}
          </Panel>
        </aside>
      </div>
    </div>
  );
}

function WorkflowForm({
  initial,
  submit,
}: {
  initial: WorkflowDraft;
  submit: (draft: WorkflowDraft) => Promise<void>;
}) {
  const [draft, setDraft] = useState(initial);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const valid = Boolean(
    draft.name.trim() && draft.scriptName.trim() && draft.className.trim(),
  );
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        void handleSubmit();
      }}
    >
      <div className="mt-5 grid gap-4 md:grid-cols-2">
        <div className="md:col-span-2">
          <Input label="Workflow name" value={draft.name} disabled />
        </div>
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
      {error ? (
        <p className="text-kumo-danger mt-3" role="alert">
          {error instanceof Error ? error.message : "The request failed."}
        </p>
      ) : null}
      <div className="mt-6 flex justify-end gap-2">
        <Button
          type="button"
          variant="secondary"
          disabled={pending}
          onClick={() => closeDialog()}
        >
          Cancel
        </Button>
        <Button type="submit" variant="primary" disabled={!valid || pending}>
          {pending ? "Saving…" : "Save changes"}
        </Button>
      </div>
    </form>
  );

  async function handleSubmit() {
    if (!valid || pending) return;
    setPending(true);
    try {
      await submit(draft);
      closeDialog();
    } catch (caught) {
      setError(caught);
    } finally {
      setPending(false);
    }
  }
}

function retentionParams(draft: WorkflowDraft) {
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

function formatDuration(value?: number) {
  return value === undefined
    ? "Unknown"
    : `${Math.round(value / 86_400_000)} days`;
}
