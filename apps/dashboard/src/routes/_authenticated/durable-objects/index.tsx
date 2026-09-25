import { Badge } from "@cloudflare/kumo/components/badge";
import { useQuery } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { useState } from "react";
import { CloudflareProductIcon } from "../../../components/cloudflare-product-icons";
import {
  CatalogToolbar,
  EmptyState,
  ErrorState,
  LoadingRows,
  Notice,
  PageHeader,
  ResourceList,
  ResourceRow,
} from "../../../components/dashboard-page";
import { useAuth } from "../../../features/auth/auth-atoms";

export const Route = createFileRoute("/_authenticated/durable-objects/")({
  component: DurableObjectsPage,
});

function DurableObjectsPage() {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const [filter, setFilter] = useState("");
  const namespaces = useQuery({
    queryKey: ["durable-objects", selectedInstanceId],
    queryFn: ({ signal }) =>
      client!.openCompute.durableObjects.list(selectedInstanceId!, {
        signal,
        query: { per_page: 100 },
      }),
    enabled: client !== null && selectedInstanceId !== null,
  });
  const needle = filter.trim().toLowerCase();
  const items = (namespaces.data?.items ?? []).filter((namespace) =>
    `${namespace.name} ${namespace.script_name} ${namespace.class_name}`
      .toLowerCase()
      .includes(needle),
  );

  return (
    <div>
      <PageHeader
        title="Durable Objects"
        description="Inspect namespaces created by Worker exports and migrations."
      />
      <Notice>
        Namespaces are managed from Worker code and deployments. This inventory
        is read-only.
      </Notice>
      <div className="mt-6">
        <CatalogToolbar
          value={filter}
          onChange={setFilter}
          onRefresh={() => void namespaces.refetch()}
          refreshing={namespaces.isFetching}
          placeholder="Search namespaces"
        />
      </div>
      {namespaces.isLoading ? (
        <LoadingRows />
      ) : namespaces.error ? (
        <ErrorState error={namespaces.error} />
      ) : items.length === 0 ? (
        <EmptyState
          title={
            needle ? "No matching namespaces" : "No Durable Object namespaces"
          }
          description={
            needle
              ? "Try a different search term."
              : "Export a Durable Object class from a Worker and deploy a migration to create a namespace."
          }
        />
      ) : (
        <ResourceList>
          {items.map((namespace) => (
            <ResourceRow
              key={namespace.id}
              href={`/durable-objects/${encodeURIComponent(namespace.id)}`}
              icon={
                <CloudflareProductIcon product="Durable Objects" size={20} />
              }
              title={namespace.class_name}
              description={`${namespace.script_name} · ${namespace.name}`}
              meta={
                <Badge
                  variant={
                    namespace.availability === "healthy"
                      ? "success"
                      : namespace.availability === "degraded"
                        ? "warning"
                        : "error"
                  }
                  appearance="dot"
                >
                  {namespace.availability}
                </Badge>
              }
              footer={`Schema ${namespace.schema_version} · Generation ${namespace.spec_generation} · ${namespace.state}`}
            />
          ))}
        </ResourceList>
      )}
      {namespaces.data?.next_cursor ? (
        <p className="text-kumo-subtle mt-4 text-sm">
          More namespaces are available. Refine the search or use the API cursor
          for the next page.
        </p>
      ) : null}
    </div>
  );
}
