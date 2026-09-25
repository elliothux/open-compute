import { Badge } from "@cloudflare/kumo/components/badge";
import { Button } from "@cloudflare/kumo/components/button";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import {
  DefinitionList,
  EmptyState,
  ErrorState,
  LoadingRows,
  PageHeader,
  Panel,
  ResourceList,
  Section,
} from "../../../components/dashboard-page";
import { useAuth } from "../../../features/auth/auth-atoms";

export const Route = createFileRoute(
  "/_authenticated/durable-objects/$namespaceId",
)({ component: DurableObjectDetailPage });

function DurableObjectDetailPage() {
  const { namespaceId } = Route.useParams();
  const { client, instanceId: selectedInstanceId } = useAuth();
  const namespaces = useQuery({
    queryKey: ["durable-objects", selectedInstanceId],
    queryFn: ({ signal }) =>
      client!.openCompute.durableObjects.list(selectedInstanceId!, {
        signal,
        query: { per_page: 100 },
      }),
    enabled: client !== null && selectedInstanceId !== null,
  });
  const objects = useInfiniteQuery({
    queryKey: ["cloudflare-v4", "durable-objects", namespaceId],
    queryFn: ({ signal, pageParam }) =>
      client!.openCompute.durableObjects.objects(
        selectedInstanceId!,
        namespaceId,
        {
          signal,
          query: { per_page: 100, ...(pageParam ? { cursor: pageParam } : {}) },
        },
      ),
    initialPageParam: "",
    getNextPageParam: (page) => page.next_cursor,
    enabled: client !== null && selectedInstanceId !== null,
  });
  const namespace = namespaces.data?.items.find(
    (item) => item.id === namespaceId,
  );
  const records = objects.data?.pages.flatMap((page) => page.items) ?? [];
  return (
    <div className="grid gap-8">
      <PageHeader
        title={namespace?.class_name ?? "Durable Object namespace"}
        description="Namespaces are managed by Worker exports and migrations; object inventory is read-only."
      />
      <Section title="Namespace details">
        <Panel>
          <DefinitionList
            items={[
              { label: "Namespace ID", value: namespaceId },
              { label: "Worker", value: namespace?.script_name ?? "Loading…" },
              { label: "Class", value: namespace?.class_name ?? "Loading…" },
              {
                label: "State",
                value: namespace ? (
                  <Badge
                    variant={
                      namespace.availability === "healthy"
                        ? "success"
                        : "warning"
                    }
                    appearance="dot"
                  >
                    {namespace.state}
                  </Badge>
                ) : (
                  "Loading…"
                ),
              },
              {
                label: "Schema version",
                value: namespace?.schema_version ?? "—",
              },
              { label: "Modified", value: namespace?.modified_on ?? "—" },
            ]}
          />
        </Panel>
      </Section>
      <Section
        title="Objects"
        description="Object IDs and lifecycle records currently known to this namespace."
      >
        {objects.isLoading ? (
          <LoadingRows />
        ) : objects.error ? (
          <ErrorState error={objects.error} />
        ) : records.length === 0 ? (
          <EmptyState
            title="No objects found"
            description="Objects appear here after the namespace receives traffic and creates state."
          />
        ) : (
          <ResourceList>
            {records.map((record) => (
              <Panel
                key={`${record.id}-${record.generation}`}
                className="grid gap-2"
              >
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <span className="min-w-0 font-medium break-all">
                    {record.id}
                  </span>
                  <Badge variant="neutral">{record.state}</Badge>
                </div>
                <p className="text-kumo-subtle text-sm">
                  Created {record.created_on} · Modified {record.modified_on} ·
                  Generation {record.generation}
                </p>
              </Panel>
            ))}
          </ResourceList>
        )}
        {objects.hasNextPage ? (
          <Button
            variant="secondary"
            disabled={objects.isFetchingNextPage}
            onClick={() => void objects.fetchNextPage()}
          >
            {objects.isFetchingNextPage ? "Loading…" : "Load more"}
          </Button>
        ) : null}
      </Section>
    </div>
  );
}
