import { createFileRoute } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { DataTable, ErrorState, LoadingState, PageHeader } from "../../../components/PageLayout";
import { useAuth } from "../../../features/auth/AuthProvider";

export const Route = createFileRoute("/_authenticated/durable-objects/$namespaceId")({ component: DurableObjectDetailPage });

function DurableObjectDetailPage() {
  const { namespaceId } = Route.useParams();
  const { client, accountId } = useAuth();
  const objects = useQuery({ queryKey: ["cloudflare-v4", "durable-objects", namespaceId], queryFn: ({ signal }) => client!.openCompute.durableObjects.objects(accountId!, namespaceId, { signal }), enabled: client !== null && accountId !== null });
  return <div><PageHeader title="Durable Object namespace" resourceId={namespaceId} description="Namespaces are managed by Worker exports and migrations; object inventory is read-only." />
    {objects.isLoading ? <LoadingState /> : objects.error ? <ErrorState message="Unable to load Durable Object inventory." /> : <DataTable columns={[{ key: "id", label: "Object ID" }, { key: "created", label: "Created" }]} rows={(objects.data ?? []).map(item => ({ id: item.id, created: item.created_on }))} emptyLabel="No objects found." />}
  </div>;
}
