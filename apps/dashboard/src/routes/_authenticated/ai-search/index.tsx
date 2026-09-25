import { Badge } from "@cloudflare/kumo/components/badge";
import { Button, LinkButton } from "@cloudflare/kumo/components/button";
import { Select } from "@cloudflare/kumo/components/select";
import {
  IconCaretRight,
  IconPlayerPlay,
  IconSettings,
} from "@tabler/icons-react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
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
import { openResourceNameDialog } from "../../../components/resource-dialog";
import { useAuth } from "../../../features/auth/auth-atoms";
import { useMutationFeedback } from "../../../features/toast/use-mutation-feedback";

export const Route = createFileRoute("/_authenticated/ai-search/")({
  validateSearch: (search: Record<string, unknown>): { namespace?: string } =>
    typeof search.namespace === "string" ? { namespace: search.namespace } : {},
  component: AISearchPage,
});

function AISearchPage() {
  const navigate = useNavigate();
  const { namespace } = Route.useSearch();
  const { client, instanceId: selectedInstanceId } = useAuth();
  const queryClient = useQueryClient();
  const feedback = useMutationFeedback();
  const [filter, setFilter] = useState("");

  const namespaces = useQuery({
    queryKey: ["ai-search", selectedInstanceId, "namespaces"],
    queryFn: ({ signal }) =>
      client!.aiSearch.namespaces.list(
        { account_id: selectedInstanceId!, per_page: 100 },
        { signal },
      ),
    enabled: client !== null && selectedInstanceId !== null,
  });
  const selectedNamespace =
    namespaces.data?.result.find((item) => item.name === namespace)?.name ??
    namespaces.data?.result[0]?.name ??
    "";
  const catalog = useQuery({
    queryKey: ["ai-search", selectedInstanceId, selectedNamespace, "instances"],
    queryFn: ({ signal }) =>
      client!.aiSearch.namespaces.instances.list(
        selectedNamespace,
        { account_id: selectedInstanceId!, per_page: 100 },
        { signal },
      ),
    enabled:
      client !== null &&
      selectedInstanceId !== null &&
      selectedNamespace.length > 0,
  });
  function openCreateNamespaceDialog() {
    openResourceNameDialog({
      title: "Create namespace",
      description:
        "Namespaces group related AI Search instances and public endpoints.",
      label: "Namespace name",
      placeholder: "production",
      submitLabel: "Create",
      submit: async (name) => {
        let createdName: string;
        try {
          const created = await client!.aiSearch.namespaces.create({
            account_id: selectedInstanceId!,
            name,
          });
          createdName = created.name;
        } catch (error) {
          feedback.failure(error, "Unable to create the namespace.");
          throw error;
        }
        await queryClient.invalidateQueries({
          queryKey: ["ai-search", selectedInstanceId, "namespaces"],
        });
        feedback.success("AI Search namespace created.");
        await navigate({
          to: "/ai-search",
          search: { namespace: createdName },
        });
      },
    });
  }

  const needle = filter.trim().toLowerCase();
  const instances = (catalog.data?.result ?? []).filter((instance) =>
    `${instance.id} ${instance.source ?? ""}`.toLowerCase().includes(needle),
  );
  const goNamespace = (view: "settings" | "playground") => {
    if (!selectedNamespace) return;
    void navigate({
      to:
        view === "settings"
          ? "/ai-search/namespace/$namespaceName/settings"
          : "/ai-search/namespace/$namespaceName/playground",
      params: { namespaceName: selectedNamespace },
    });
  };

  return (
    <div>
      <PageHeader
        title="AI Search"
        description="Use your data to create AI-powered search experiences and natural-language answers in your apps."
        actions={
          <>
            <LinkButton
              href="https://developers.cloudflare.com/ai-search/"
              external
            >
              Docs
            </LinkButton>
            <Button
              variant="primary"
              onClick={() => void navigate({ to: "/ai-search/new" })}
            >
              Create instance
            </Button>
          </>
        }
      />
      <div className="grid gap-6 xl:grid-cols-3">
        <div className="min-w-0 xl:col-span-2">
          <div className="border-kumo-line mb-5 flex flex-wrap items-end justify-between gap-3 border-b pb-4">
            <Select
              className="min-w-48"
              label="Namespace"
              value={selectedNamespace}
              placeholder={
                (namespaces.data?.result ?? []).length === 0
                  ? "No namespaces"
                  : "Select a namespace"
              }
              items={(namespaces.data?.result ?? []).map((item) => ({
                label: item.name,
                value: item.name,
              }))}
              onValueChange={(value) => {
                if (value)
                  void navigate({
                    to: "/ai-search",
                    search: { namespace: value },
                  });
              }}
            />
            <div className="flex flex-wrap gap-2">
              <Button variant="secondary" onClick={openCreateNamespaceDialog}>
                Create namespace
              </Button>
              <Button
                variant="secondary"
                disabled={!selectedNamespace}
                onClick={() => goNamespace("playground")}
              >
                <IconPlayerPlay size={16} /> Playground
              </Button>
              <Button
                variant="secondary"
                disabled={!selectedNamespace}
                onClick={() => goNamespace("settings")}
              >
                <IconSettings size={16} /> Settings
              </Button>
            </div>
          </div>
          <CatalogToolbar
            value={filter}
            onChange={setFilter}
            onRefresh={() => void catalog.refetch()}
            refreshing={catalog.isFetching}
            placeholder="Search instances"
          />
          {namespaces.isLoading || catalog.isLoading ? (
            <LoadingRows />
          ) : namespaces.error ? (
            <ErrorState error={namespaces.error} />
          ) : catalog.error ? (
            <ErrorState error={catalog.error} />
          ) : instances.length === 0 ? (
            <EmptyState
              title={
                needle ? "No matching instances" : "Get started with AI Search"
              }
              description={
                needle
                  ? "Try a different search term."
                  : "Build an AI-powered search experience."
              }
              action={
                !needle ? (
                  <Button
                    onClick={() => void navigate({ to: "/ai-search/new" })}
                  >
                    Create instance
                  </Button>
                ) : undefined
              }
            />
          ) : (
            <ResourceList>
              {instances.map((instance) => (
                <ResourceRow
                  key={instance.id}
                  href={`/ai-search/${encodeURIComponent(selectedNamespace)}/${encodeURIComponent(instance.id)}`}
                  icon={<CloudflareProductIcon product="AI Search" size={20} />}
                  title={instance.id}
                  description={instance.source || "Manual source"}
                  meta={
                    <Badge
                      variant={
                        instance.paused
                          ? "warning"
                          : instance.status === "error"
                            ? "error"
                            : "success"
                      }
                      appearance="dot"
                    >
                      {instance.paused ? "paused" : instance.status}
                    </Badge>
                  }
                  footer={`Modified ${instance.modified_at}`}
                />
              ))}
            </ResourceList>
          )}
        </div>
        <aside
          className="grid content-start gap-2"
          aria-label="AI Search resources"
        >
          {(
            [
              [
                "Get started with AI Search",
                "https://developers.cloudflare.com/ai-search/get-started/",
              ],
              [
                "Add AI Search to Workers",
                "https://developers.cloudflare.com/ai-search/usage/workers-binding/",
              ],
              [
                "Join the Discord server",
                "https://discord.com/channels/595317990191398933/1356674457355423895",
              ],
            ] as const
          ).map(([label, href]) => (
            <a
              key={href}
              href={href}
              target="_blank"
              rel="noreferrer"
              className="border-kumo-line hover:bg-kumo-tint flex items-center justify-between rounded-lg border px-4 py-4"
            >
              <span>{label}</span>
              <IconCaretRight size={16} />
            </a>
          ))}
        </aside>
      </div>
    </div>
  );
}
