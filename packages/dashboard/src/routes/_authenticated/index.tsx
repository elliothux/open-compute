import { createFileRoute } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { DataTable, ErrorState, LoadingState, PageHeader, StatusBadge } from "../../components/PageLayout";
import { useAuth } from "../../features/auth/AuthProvider";

export const Route = createFileRoute("/_authenticated/")({ component: OverviewPage });

function OverviewPage() {
  const { client } = useAuth();
  const query = useQuery({
    queryKey: ["cloudflare-v4", "open-compute", "overview"],
    queryFn: async ({ signal }) => Promise.all([
      client!.openCompute.capabilities.get({ signal }),
      client!.openCompute.system.status({ signal }),
    ]),
    enabled: client !== null,
  });
  return <div><PageHeader title="Overview" description="Release capabilities and installation health from the v4 API." />
    {query.isLoading ? <LoadingState /> : query.error ? <ErrorState message="Unable to load platform overview." /> : (
      <DataTable columns={[{ key: "name", label: "Property" }, { key: "value", label: "Value" }]} rows={[
        { name: "Release", value: query.data?.[0].release ?? "unknown" },
        { name: "Certified Wrangler", value: query.data?.[0].wrangler_version ?? "unknown" },
        { name: "System", value: <StatusBadge value={query.data?.[1].state ?? "unknown"} /> },
      ]} />
    )}
  </div>;
}
