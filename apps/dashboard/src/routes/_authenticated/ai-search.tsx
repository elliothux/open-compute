import { LinkButton } from "@cloudflare/kumo/components/button";
import { useQuery } from "@tanstack/react-query";
import { createFileRoute, Outlet } from "@tanstack/react-router";
import {
  EmptyState,
  ErrorState,
  LoadingRows,
  PageHeader,
} from "../../components/dashboard-page";
import { useAuth } from "../../features/auth/auth-atoms";
import { capabilityQuery } from "../../lib/query-options";

export const Route = createFileRoute("/_authenticated/ai-search")({
  loader: ({ context }) => {
    const { client, instanceId } = context.auth;
    if (!client || !instanceId) return;
    return context.queryClient.ensureQueryData(
      capabilityQuery(client, instanceId),
    );
  },
  component: AISearchLayout,
});

function AISearchLayout() {
  const { client, instanceId } = useAuth();
  const capabilities = useQuery(capabilityQuery(client, instanceId));

  if (capabilities.isLoading) return <LoadingRows count={2} />;
  if (capabilities.error) return <ErrorState error={capabilities.error} />;
  if (!capabilities.data?.configuration.ai_search) {
    return (
      <div>
        <PageHeader
          title="AI Search"
          description="Create search and answer experiences from your own data."
        />
        <EmptyState
          title="Configure an AI provider"
          description="AI Search needs a default embedding model in this instance's OCD configuration. Configure the provider, profile, and model mapping, then restart OCD."
          action={
            <LinkButton
              href="https://open-compute.dev/docs/ocd/configuration/#ai-provider-backends-and-embedding-profiles"
              external
            >
              View configuration docs
            </LinkButton>
          }
        />
      </div>
    );
  }
  return <Outlet />;
}
