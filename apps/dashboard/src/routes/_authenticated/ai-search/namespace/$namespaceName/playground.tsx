import { Button } from "@cloudflare/kumo/components/button";
import { useMutation, useQuery } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { CodeBlock } from "../../../../../components/code-block";
import {
  EmptyState,
  ErrorState,
  LoadingRows,
  PageTabs,
} from "../../../../../components/dashboard-page";
import { SearchInput } from "../../../../../components/search-input";
import { useAuth } from "../../../../../features/auth/auth-atoms";

export const Route = createFileRoute(
  "/_authenticated/ai-search/namespace/$namespaceName/playground",
)({
  component: NamespacePlaygroundPage,
});

function NamespacePlaygroundPage() {
  const { namespaceName } = Route.useParams();
  const { client, instanceId: selectedInstanceId } = useAuth();
  const navigate = useNavigate();
  const [query, setQuery] = useState("");
  const instances = useQuery({
    queryKey: ["ai-search", selectedInstanceId, namespaceName, "instances"],
    queryFn: ({ signal }) =>
      client!.aiSearch.namespaces.instances.list(
        namespaceName,
        { account_id: selectedInstanceId!, per_page: 100 },
        { signal },
      ),
    enabled: client !== null && selectedInstanceId !== null,
  });
  const search = useMutation({
    mutationFn: () =>
      client!.aiSearch.namespaces.search(namespaceName, {
        account_id: selectedInstanceId!,
        query: query.trim(),
        ai_search_options: {
          instance_ids: (instances.data?.result ?? []).map(
            (instance) => instance.id,
          ),
        },
      }),
  });
  const base = `/ai-search/namespace/${encodeURIComponent(namespaceName)}`;
  return (
    <div>
      <PageTabs
        active="Playground"
        items={[
          { label: "Playground", href: `${base}/playground` },
          { label: "Settings", href: `${base}/settings` },
        ]}
      />
      {instances.isLoading ? (
        <LoadingRows count={1} />
      ) : instances.error ? (
        <ErrorState error={instances.error} />
      ) : !instances.data?.result.length ? (
        <EmptyState
          title="Get started with AI Search"
          description="Build an AI-powered search experience."
          action={
            <Button onClick={() => void navigate({ to: "/ai-search/new" })}>
              Create instance
            </Button>
          }
        />
      ) : (
        <div className="grid max-w-3xl gap-4">
          <form
            className="flex"
            onSubmit={(event) => {
              event.preventDefault();
              if (query.trim()) search.mutate();
            }}
          >
            <SearchInput
              aria-label="Search query"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Ask a question about your indexed content"
              className="min-w-0 flex-1 rounded-r-none"
            />
            <Button
              className="rounded-l-none"
              type="submit"
              disabled={!query.trim() || search.isPending}
            >
              Search
            </Button>
          </form>
          {search.error ? <ErrorState error={search.error} /> : null}
          {search.data ? (
            <CodeBlock
              className="border-kumo-line overflow-x-auto rounded-lg border p-4 text-xs"
              code={JSON.stringify(search.data, null, 2)}
              language="json"
            />
          ) : null}
        </div>
      )}
    </div>
  );
}
