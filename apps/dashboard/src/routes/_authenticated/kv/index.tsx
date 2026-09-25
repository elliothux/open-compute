import { Button } from "@cloudflare/kumo/components/button";
import { DropdownMenu } from "@cloudflare/kumo/components/dropdown";
import { InlineCopyText } from "@cloudflare/kumo/components/inline-copy-text";
import { LayerCard } from "@cloudflare/kumo/components/layer-card";
import { Table } from "@cloudflare/kumo/components/table";
import { IconDots, IconPlus } from "@tabler/icons-react";
import { useQuery } from "@tanstack/react-query";
import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useMemo } from "react";
import {
  CatalogToolbar,
  EmptyState,
  ErrorState,
  LoadingRows,
  PageHeader,
} from "../../../components/dashboard-page";
import {
  openConfirmDeleteDialog,
  openResourceNameDialog,
} from "../../../components/resource-dialog";
import { useAuth } from "../../../features/auth/auth-atoms";
import { useMutationFeedback } from "../../../features/toast/use-mutation-feedback";

export const Route = createFileRoute("/_authenticated/kv/")({
  validateSearch: (search: Record<string, unknown>): { q?: string } =>
    typeof search.q === "string" && search.q ? { q: search.q } : {},
  component: KvPage,
});

type Namespace = { id: string; title: string };

function KvPage() {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const { q: search = "" } = Route.useSearch();
  const navigate = useNavigate();
  const feedback = useMutationFeedback();
  const enabled = client !== null && selectedInstanceId !== null;
  const namespaces = useQuery({
    queryKey: ["cloudflare-v4", "kv", selectedInstanceId, "namespaces"],
    queryFn: async ({ signal }) => {
      const result: Namespace[] = [];
      for await (const namespace of client!.kv.namespaces.list(
        { account_id: selectedInstanceId!, per_page: 1000, order: "title" },
        { signal },
      )) {
        result.push({ id: namespace.id, title: namespace.title });
      }
      return result;
    },
    enabled,
  });
  const visible = useMemo(() => {
    const needle = search.trim().toLowerCase();
    return (namespaces.data ?? []).filter((item) =>
      item.title.toLowerCase().includes(needle),
    );
  }, [namespaces.data, search]);
  function openCreateNamespaceDialog() {
    openResourceNameDialog({
      title: "Create a KV namespace",
      description:
        "Choose a name for your KV namespace. Once created, you can add key-value pairs to it.",
      label: "Namespace name",
      placeholder: "your_kv_namespace",
      submitLabel: "Create",
      submit: async (title) => {
        try {
          await client!.kv.namespaces.create({
            account_id: selectedInstanceId!,
            title,
          });
        } catch (error) {
          feedback.failure(error, "Unable to create the namespace.");
          throw error;
        }
        await namespaces.refetch();
        feedback.success("KV namespace created.");
      },
    });
  }

  function confirmDeleteNamespace(namespace: Namespace) {
    openConfirmDeleteDialog({
      name: namespace.title,
      confirm: async () => {
        try {
          await client!.kv.namespaces.delete(namespace.id, {
            account_id: selectedInstanceId!,
          });
        } catch (error) {
          feedback.failure(error, "Unable to delete the namespace.");
          throw error;
        }
        await namespaces.refetch();
        feedback.success("KV namespace deleted.");
      },
    });
  }

  return (
    <div className="min-w-0">
      <div className="mb-4">
        <PageHeader
          title="KV"
          description="Store application data and configuration in globally addressable namespaces."
          actions={
            <Button
              variant="primary"
              onClick={openCreateNamespaceDialog}
              disabled={!enabled}
            >
              <IconPlus size={16} /> Create namespace
            </Button>
          }
        />
      </div>
      <div className="pt-4 pb-3">
        <CatalogToolbar
          value={search}
          onChange={(value) =>
            void navigate({
              to: "/kv",
              search: value ? { q: value } : {},
              replace: true,
            })
          }
          onRefresh={() => void namespaces.refetch()}
          refreshing={namespaces.isFetching}
          placeholder="Search namespaces"
        />
      </div>
      {namespaces.isLoading ? (
        <LoadingRows />
      ) : namespaces.error ? (
        <ErrorState error={namespaces.error} />
      ) : visible.length === 0 ? (
        <EmptyState
          title={search ? "No matching namespaces" : "No KV namespaces"}
          description={
            search
              ? "Try a different search term."
              : "Create a namespace to store your first key-value pair."
          }
          action={
            !search ? (
              <Button variant="primary" onClick={openCreateNamespaceDialog}>
                Create namespace
              </Button>
            ) : undefined
          }
        />
      ) : (
        <LayerCard className="overflow-hidden p-0">
          <div className="overflow-x-auto">
            <Table className="min-w-0 table-fixed sm:min-w-lg">
              <Table.Header variant="compact">
                <Table.Row>
                  <Table.Head>Name</Table.Head>
                  <Table.Head>ID</Table.Head>
                  <Table.Head className="w-12">
                    <span className="sr-only">Actions</span>
                  </Table.Head>
                </Table.Row>
              </Table.Header>
              <Table.Body>
                {visible.map((namespace) => (
                  <Table.Row key={namespace.id}>
                    <Table.Cell className="min-w-0 overflow-hidden">
                      <Link
                        className="text-kumo-link block truncate underline"
                        to="/kv/$namespaceId"
                        params={{ namespaceId: namespace.id }}
                      >
                        {namespace.title}
                      </Link>
                    </Table.Cell>
                    <Table.Cell className="text-kumo-subtle min-w-0 overflow-hidden">
                      <InlineCopyText
                        labels={{
                          copyAction: `Copy ID for ${namespace.title}`,
                        }}
                      >
                        {namespace.id}
                      </InlineCopyText>
                    </Table.Cell>
                    <Table.Cell>
                      <DropdownMenu>
                        <DropdownMenu.Trigger>
                          <Button
                            variant="ghost"
                            shape="square"
                            aria-label={`Actions for ${namespace.title}`}
                          >
                            <IconDots size={20} strokeWidth={2.5} />
                          </Button>
                        </DropdownMenu.Trigger>
                        <DropdownMenu.Content>
                          <DropdownMenu.Item
                            onClick={() => {
                              void navigator.clipboard
                                .writeText(
                                  `[[kv_namespaces]]\nbinding = "MY_KV"\nid = "${namespace.id}"`,
                                )
                                .then(() =>
                                  feedback.success("KV binding copied."),
                                )
                                .catch((error: unknown) =>
                                  feedback.failure(
                                    error,
                                    "Unable to copy the KV binding.",
                                  ),
                                );
                            }}
                          >
                            Copy bindings
                          </DropdownMenu.Item>
                          <DropdownMenu.Separator />
                          <DropdownMenu.Item
                            variant="danger"
                            onClick={() => confirmDeleteNamespace(namespace)}
                          >
                            Delete
                          </DropdownMenu.Item>
                        </DropdownMenu.Content>
                      </DropdownMenu>
                    </Table.Cell>
                  </Table.Row>
                ))}
              </Table.Body>
            </Table>
          </div>
        </LayerCard>
      )}
    </div>
  );
}
