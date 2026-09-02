import { createFileRoute } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { DataTable, ErrorState, LoadingState, PageHeader } from "../../../components/PageLayout";
import { useAuth } from "../../../features/auth/AuthProvider";

export const Route = createFileRoute("/_authenticated/r2/$bucketId")({ component: R2DetailPage });

function R2DetailPage() {
  const { bucketId } = Route.useParams();
  const { client, accountId } = useAuth();
  const bucket = useQuery({ queryKey: ["cloudflare-v4", "r2", bucketId], queryFn: ({ signal }) => client!.cloudflare.r2.buckets.get(bucketId, { account_id: accountId! }, { signal }), enabled: client !== null && accountId !== null });
  return <div><PageHeader title={bucket.data?.name ?? bucketId} description="Bucket properties from the official R2 API." />
    {bucket.isLoading ? <LoadingState /> : bucket.error ? <ErrorState message="Unable to load R2 bucket details." /> : <DataTable columns={[{ key: "property", label: "Property" }, { key: "value", label: "Value" }]} rows={[{ property: "Created", value: bucket.data?.creation_date ?? "unknown" }, { property: "Location", value: bucket.data?.location ?? "default" }, { property: "Storage class", value: bucket.data?.storage_class ?? "Standard" }]} />}
  </div>;
}
