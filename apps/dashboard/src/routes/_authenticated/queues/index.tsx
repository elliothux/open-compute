import { Badge } from "@cloudflare/kumo/components/badge";
import { Button } from "@cloudflare/kumo/components/button";
import { IconList, IconPlus } from "@tabler/icons-react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMemo } from "react";
import { CloudflareProductIcon } from "../../../components/cloudflare-product-icons";
import {
  CatalogToolbar,
  EmptyState,
  ErrorState,
  LoadingRows,
  PageHeader,
  ResourceList,
  ResourceRow,
} from "../../../components/dashboard-page";
import { openConfirmDeleteDialog } from "../../../components/resource-dialog";
import { RowActionsMenu } from "../../../components/row-actions-menu";
import { useAuth } from "../../../features/auth/auth-atoms";
import { useMutationFeedback } from "../../../features/toast/use-mutation-feedback";
import { formatDate } from "../../../lib/format";

export const Route = createFileRoute("/_authenticated/queues/")({
  validateSearch: (search: Record<string, unknown>): { q?: string } =>
    typeof search.q === "string" && search.q ? { q: search.q } : {},
  component: QueuesPage,
});

function QueuesPage() {
  const navigate = useNavigate();
  const { q: search = "" } = Route.useSearch();
  const { client, instanceId: selectedInstanceId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const enabled = client !== null && selectedInstanceId !== null;

  const queues = useQuery({
    queryKey: ["cloudflare-v4", "queues", selectedInstanceId],
    queryFn: ({ signal }) =>
      client!.queues.list({ account_id: selectedInstanceId! }, { signal }),
    enabled,
  });
  const refresh = () =>
    queryClient.invalidateQueries({
      queryKey: ["cloudflare-v4", "queues", selectedInstanceId],
    });
  function confirmDeleteQueue(id: string, name: string) {
    openConfirmDeleteDialog({
      name,
      confirm: async () => {
        try {
          await client!.queues.delete(id, { account_id: selectedInstanceId! });
        } catch (error) {
          feedback.failure(error, "Unable to delete the queue.");
          throw error;
        }
        await refresh();
        feedback.success("Queue deleted.");
      },
    });
  }

  const rows = useMemo(() => {
    const query = search.trim().toLowerCase();
    return (queues.data?.result ?? []).filter((queue) =>
      (queue.queue_name ?? "").toLowerCase().includes(query),
    );
  }, [queues.data, search]);

  return (
    <div className="text-sm leading-5">
      <PageHeader
        title="Queues"
        description="Send and receive messages with guaranteed delivery across your applications."
        actions={
          <Button
            variant="primary"
            icon={<IconPlus size={16} />}
            onClick={() => void navigate({ to: "/queues/new" })}
            disabled={!enabled}
          >
            Create queue
          </Button>
        }
      />
      <CatalogToolbar
        value={search}
        onChange={(value) =>
          void navigate({
            to: "/queues",
            search: value ? { q: value } : {},
            replace: true,
          })
        }
        onRefresh={() => void queues.refetch()}
        refreshing={queues.isFetching}
        placeholder="Search queues"
      />

      {queues.isLoading ? (
        <LoadingRows />
      ) : queues.error ? (
        <ErrorState error={queues.error} />
      ) : rows.length === 0 ? (
        <EmptyState
          title={search ? "No matching queues" : "Create a queue"}
          description={
            search
              ? "Try a different search term."
              : "Create a queue to build an event-driven system with asynchronous message delivery."
          }
          action={
            search ? undefined : (
              <Button
                variant="primary"
                onClick={() => void navigate({ to: "/queues/new" })}
                disabled={!enabled}
              >
                Create queue
              </Button>
            )
          }
        />
      ) : (
        <ResourceList>
          {rows.map((queue) => {
            const id = queue.queue_id ?? "unknown";
            const name = queue.queue_name ?? "Unnamed queue";
            const paused = queue.settings?.delivery_paused === true;
            return (
              <ResourceRow
                key={id}
                href={`/queues/${encodeURIComponent(id)}`}
                icon={<CloudflareProductIcon product="Queues" size={18} />}
                title={name}
                description={`Created ${formatDate(queue.created_on)}`}
                meta={
                  <Badge
                    variant={paused ? "warning" : "success"}
                    appearance="dot"
                  >
                    {paused ? "Paused" : "Active"}
                  </Badge>
                }
                footer={
                  <div className="flex items-center justify-between gap-3">
                    <span className="flex items-center gap-1.5">
                      <IconList size={16} />
                      {queue.consumers_total_count ?? 0} consumers
                    </span>
                    <RowActionsMenu
                      label={name}
                      actions={[
                        {
                          id: "delete",
                          label: "Delete queue",
                          variant: "danger",
                          onSelect: () => confirmDeleteQueue(id, name),
                        },
                      ]}
                    />
                  </div>
                }
              />
            );
          })}
        </ResourceList>
      )}
    </div>
  );
}
