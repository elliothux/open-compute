import { Badge } from "@cloudflare/kumo/components/badge";
import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Table } from "@cloudflare/kumo/components/table";
import { Tabs } from "@cloudflare/kumo/components/tabs";
import { IconRefresh } from "@tabler/icons-react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { AISearchItems } from "../../../../components/ai-search-items";
import { AISearchPlayground } from "../../../../components/ai-search-playground";
import {
  DefinitionList,
  EmptyState,
  ErrorState,
  LoadingRows,
  PageHeader,
  Panel,
  Section,
  StatGrid,
} from "../../../../components/dashboard-page";
import { openConfirmDeleteDialog } from "../../../../components/resource-dialog";
import { useAuth } from "../../../../features/auth/auth-atoms";
import { useMutationFeedback } from "../../../../features/toast/use-mutation-feedback";

type Tab = "overview" | "playground" | "items" | "jobs" | "settings";

export const Route = createFileRoute(
  "/_authenticated/ai-search/$namespaceName/$instanceId",
)({
  validateSearch: (search: Record<string, unknown>): { tab?: Tab } =>
    search.tab === "playground" ||
    search.tab === "items" ||
    search.tab === "jobs" ||
    search.tab === "settings"
      ? { tab: search.tab }
      : {},
  component: AISearchDetailPage,
});

function AISearchDetailPage() {
  const { namespaceName, instanceId } = Route.useParams();
  const { tab: searchTab } = Route.useSearch();
  const tab = searchTab ?? "overview";
  const { client, instanceId: selectedInstanceId } = useAuth();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const [selectedJobId, setSelectedJobId] = useState<string | null>(null);

  const instance = useQuery({
    queryKey: ["ai-search", selectedInstanceId, namespaceName, instanceId],
    queryFn: ({ signal }) =>
      Promise.all([
        client!.aiSearch.namespaces.instances.read(
          instanceId,
          { account_id: selectedInstanceId!, name: namespaceName },
          { signal },
        ),
        client!.aiSearch.namespaces.instances.stats(
          instanceId,
          { account_id: selectedInstanceId!, name: namespaceName },
          { signal },
        ),
        client!.aiSearch.namespaces.read(
          namespaceName,
          { account_id: selectedInstanceId! },
          { signal },
        ),
      ]),
    enabled: client !== null && selectedInstanceId !== null,
  });
  const jobs = useQuery({
    queryKey: [
      "ai-search",
      selectedInstanceId,
      namespaceName,
      instanceId,
      "jobs",
    ],
    queryFn: async ({ signal }) =>
      (
        await client!.aiSearch.namespaces.instances.jobs.list(
          instanceId,
          {
            account_id: selectedInstanceId!,
            name: namespaceName,
            per_page: 50,
          },
          { signal },
        )
      ).result,
    enabled: client !== null && selectedInstanceId !== null && tab === "jobs",
  });
  const lastJob = jobs.data?.[0];
  const activeJobId = selectedJobId ?? lastJob?.id;
  const jobLogs = useQuery({
    queryKey: [
      "ai-search",
      selectedInstanceId,
      namespaceName,
      instanceId,
      "job-logs",
      activeJobId,
    ],
    queryFn: ({ signal }) =>
      client!.aiSearch.namespaces.instances.jobs.logs(
        activeJobId!,
        {
          account_id: selectedInstanceId!,
          name: namespaceName,
          id: instanceId,
          per_page: 100,
        },
        { signal },
      ),
    enabled:
      client !== null &&
      selectedInstanceId !== null &&
      tab === "jobs" &&
      !!activeJobId,
  });

  const createJob = useMutation({
    mutationFn: () =>
      client!.aiSearch.namespaces.instances.jobs.create(instanceId, {
        account_id: selectedInstanceId!,
        name: namespaceName,
        description: "Manual dashboard sync",
      }),
    onSuccess: async () => {
      await jobs.refetch();
      feedback.success("Indexing job started.");
    },
    onError: (error) => feedback.failure(error, "Unable to start indexing."),
  });
  function confirmDeleteInstance() {
    openConfirmDeleteDialog({
      name: instanceId,
      confirm: async () => {
        try {
          await client!.aiSearch.namespaces.instances.delete(instanceId, {
            account_id: selectedInstanceId!,
            name: namespaceName,
          });
        } catch (error) {
          feedback.failure(error, "Unable to delete the instance.");
          throw error;
        }
        await queryClient.invalidateQueries({
          queryKey: ["ai-search", selectedInstanceId],
        });
        feedback.success("AI Search instance deleted.");
        await navigate({ to: "/ai-search" });
      },
    });
  }

  const data = instance.data?.[0];
  const stats = instance.data?.[1];
  const namespace = instance.data?.[2];
  const tabs = ["Overview", "Playground", "Jobs", "Items", "Settings"] as const;

  return (
    <div>
      <PageHeader
        title={instanceId}
        description={`AI Search instance in ${namespaceName}`}
      />
      <nav
        className={tab === "playground" ? "mb-0" : "mb-6"}
        aria-label="Resource tabs"
      >
        <Tabs
          variant="underline"
          value={tab}
          tabs={tabs.map((label) => ({
            value: label.toLowerCase(),
            label,
          }))}
          onValueChange={(value) =>
            void navigate({
              to: "/ai-search/$namespaceName/$instanceId",
              params: { namespaceName, instanceId },
              search:
                value === "overview"
                  ? {}
                  : { tab: value as Exclude<Tab, "overview"> },
            })
          }
        />
      </nav>
      {instance.isLoading ? (
        <LoadingRows />
      ) : instance.error ? (
        <ErrorState error={instance.error} />
      ) : null}

      {tab === "overview" && instance.data ? (
        <div className="grid gap-8">
          <StatGrid
            items={[
              { label: "Indexed", value: stats?.completed ?? 0 },
              { label: "Queued", value: stats?.queued ?? 0 },
              { label: "Running", value: stats?.running ?? 0 },
              { label: "Errors", value: stats?.error ?? 0 },
            ]}
          />
          <Section title="Instance details">
            <Panel>
              <DefinitionList
                items={[
                  {
                    label: "Namespace",
                    value: namespace?.name ?? namespaceName,
                  },
                  { label: "Source", value: data?.source ?? "Manual uploads" },
                  { label: "Source type", value: data?.type ?? "builtin" },
                  {
                    label: "Embedding model",
                    value: data?.embedding_model || "Default",
                  },
                  {
                    label: "Search model",
                    value: data?.ai_search_model || "Default",
                  },
                  { label: "Last activity", value: data?.last_activity ?? "—" },
                ]}
              />
            </Panel>
          </Section>
        </div>
      ) : null}

      {tab === "playground" ? (
        <AISearchPlayground
          namespaceName={namespaceName}
          instanceId={instanceId}
          metadataFields={data?.custom_metadata ?? []}
          generationModel={data?.ai_search_model}
        />
      ) : null}

      {tab === "items" ? (
        <AISearchItems
          namespaceName={namespaceName}
          instanceId={instanceId}
          source={data?.source}
          sourceType={data?.type}
          metadataFields={data?.custom_metadata ?? []}
        />
      ) : null}

      {tab === "jobs" ? (
        <div className="grid gap-7">
          <section className="grid gap-3">
            <div className="flex items-center justify-between gap-3">
              <h2 className="text-base font-semibold">Last job</h2>
              <Button
                disabled={createJob.isPending}
                onClick={() => createJob.mutate()}
              >
                <IconRefresh size={16} />
                {createJob.isPending ? "Syncing…" : "Sync"}
              </Button>
            </div>
            <Panel className="!p-0">
              {lastJob ? (
                <>
                  <JobFacts job={lastJob} />
                  <JobLogLines
                    logs={jobLogs.data}
                    loading={jobLogs.isLoading}
                  />
                </>
              ) : jobs.isLoading ? (
                <LoadingRows count={1} />
              ) : (
                <p className="text-kumo-subtle px-4 py-6 text-sm">
                  No jobs yet. Sync to index the configured source.
                </p>
              )}
            </Panel>
          </section>
          <Section
            title="Jobs"
            description="Run a full scan of all documents to update the index."
          >
            {jobs.isLoading ? (
              <LoadingRows />
            ) : jobs.error ? (
              <ErrorState error={jobs.error} />
            ) : !jobs.data?.length ? (
              <EmptyState
                title="No indexing jobs"
                description="Start a manual indexing job to synchronize the configured source."
              />
            ) : (
              <div className="ring-kumo-line overflow-x-auto rounded-lg ring">
                <Table className="min-w-3xl">
                  <Table.Header variant="compact">
                    <Table.Row>
                      {[
                        "ID",
                        "Status",
                        "Source",
                        "Started",
                        "Duration",
                        "Last sync",
                      ].map((heading) => (
                        <Table.Head key={heading}>{heading}</Table.Head>
                      ))}
                    </Table.Row>
                  </Table.Header>
                  <Table.Body>
                    {jobs.data.map((job) => (
                      <Table.Row key={job.id}>
                        <Table.Cell>
                          <button
                            className="text-kumo-link underline"
                            aria-label={job.id}
                            onClick={() => setSelectedJobId(job.id)}
                          >
                            {job.id.slice(0, 8)}
                          </button>
                        </Table.Cell>
                        <Table.Cell>
                          <JobStatus job={job} />
                        </Table.Cell>
                        <Table.Cell>
                          {job.source === "schedule" ? "Scheduled" : "Manual"}
                        </Table.Cell>
                        <Table.Cell>{formatTime(job.started_at)}</Table.Cell>
                        <Table.Cell>
                          {formatDuration(job.started_at, job.ended_at)}
                        </Table.Cell>
                        <Table.Cell>{formatTime(job.ended_at)}</Table.Cell>
                      </Table.Row>
                    ))}
                  </Table.Body>
                </Table>
              </div>
            )}
          </Section>
          <Dialog.Root
            open={selectedJobId !== null}
            onOpenChange={(open) => {
              if (!open) setSelectedJobId(null);
            }}
          >
            <Dialog className="px-5 py-4" size="xl">
              <Dialog.Title>Job {selectedJobId?.slice(0, 8)}</Dialog.Title>
              <div className="ring-kumo-line mt-5 overflow-hidden rounded-lg ring">
                {jobs.data?.find((job) => job.id === selectedJobId) ? (
                  <JobFacts
                    job={jobs.data.find((job) => job.id === selectedJobId)!}
                  />
                ) : null}
                <JobLogLines logs={jobLogs.data} loading={jobLogs.isLoading} />
              </div>
              <div className="mt-5 flex justify-end">
                <Button
                  variant="secondary"
                  onClick={() => setSelectedJobId(null)}
                >
                  Close
                </Button>
              </div>
            </Dialog>
          </Dialog.Root>
        </div>
      ) : null}

      {tab === "settings" ? (
        <div className="grid gap-8">
          <Section title="Namespace and endpoint">
            <Panel>
              <DefinitionList
                items={[
                  { label: "Namespace", value: namespaceName },
                  {
                    label: "Description",
                    value: namespace?.description || "—",
                  },
                  {
                    label: "Public endpoint",
                    value: namespace?.public_endpoint_id || "Disabled",
                  },
                  { label: "Created", value: data?.created_at ?? "—" },
                  { label: "Modified", value: data?.modified_at ?? "—" },
                ]}
              />
            </Panel>
          </Section>
          <Section
            title="Delete instance"
            description="Permanently remove the instance and all indexed data."
          >
            <Panel>
              <Button
                variant="destructive"
                onClick={() => confirmDeleteInstance()}
              >
                Delete instance
              </Button>
            </Panel>
          </Section>
        </div>
      ) : null}
    </div>
  );
}

type Job = {
  id: string;
  source: "user" | "schedule";
  started_at?: string;
  ended_at?: string;
  end_reason?: string;
};
type JobLog = { id: number; created_at: number; message: string };

function formatTime(value?: string) {
  return value ? new Date(value).toLocaleString() : "—";
}

function formatDuration(start?: string, end?: string) {
  if (!start || !end) return "—";
  const seconds = Math.max(
    0,
    Math.round((Date.parse(end) - Date.parse(start)) / 1000),
  );
  return Number.isFinite(seconds) ? `~ ${seconds} s` : "—";
}

function JobStatus({ job }: { job: Job }) {
  const failed = job.end_reason && job.end_reason !== "completed";
  return (
    <Badge
      variant={failed ? "error" : job.ended_at ? "success" : "warning"}
      appearance="dot"
    >
      {failed ? "Error" : job.ended_at ? "Completed" : "Running"}
    </Badge>
  );
}

function JobFacts({ job }: { job: Job }) {
  const facts = [
    ["Status", <JobStatus key="status" job={job} />],
    ["Source", job.source === "schedule" ? "Scheduled" : "Manual"],
    ["Started", formatTime(job.started_at)],
    ["Duration", formatDuration(job.started_at, job.ended_at)],
    ["Last sync", formatTime(job.ended_at)],
  ] as const;
  return (
    <div className="grid grid-cols-2 sm:grid-cols-5">
      {facts.map(([label, value]) => (
        <div
          key={label}
          className="border-kumo-line grid gap-2 border-b px-4 py-4 sm:border-r sm:last:border-r-0"
        >
          <span className="text-kumo-subtle text-sm">{label}</span>
          <span className="text-sm">{value}</span>
        </div>
      ))}
    </div>
  );
}

function JobLogLines({
  logs,
  loading,
}: {
  logs: JobLog[] | undefined;
  loading: boolean;
}) {
  return (
    <div className="bg-kumo-base max-h-64 min-h-44 overflow-auto px-4 py-3 font-mono text-xs">
      {loading
        ? "Loading logs…"
        : logs?.length
          ? logs.map((log) => (
              <div key={log.id}>
                {new Date(log.created_at).toLocaleString()}　{log.message}
              </div>
            ))
          : "No events"}
    </div>
  );
}
