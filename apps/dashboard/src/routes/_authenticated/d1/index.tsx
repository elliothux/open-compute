import { Button } from "@cloudflare/kumo/components/button";
import { LayerCard } from "@cloudflare/kumo/components/layer-card";
import { Table } from "@cloudflare/kumo/components/table";
import { IconPlus, IconTrash } from "@tabler/icons-react";
import { useQuery } from "@tanstack/react-query";
import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useMemo } from "react";
import { CloudflareProductIcon } from "../../../components/cloudflare-product-icons";
import {
  CatalogToolbar,
  EmptyState,
  ErrorState,
  LoadingRows,
  PageHeader,
} from "../../../components/dashboard-page";
import { openConfirmDeleteDialog } from "../../../components/resource-dialog";
import { useAuth } from "../../../features/auth/auth-atoms";
import { useMutationFeedback } from "../../../features/toast/use-mutation-feedback";

export const Route = createFileRoute("/_authenticated/d1/")({
  validateSearch: (search: Record<string, unknown>): { q?: string } =>
    typeof search.q === "string" && search.q ? { q: search.q } : {},
  component: D1Page,
});

type DatabaseRow = { uuid: string; name: string; created_at?: string };

function D1Page() {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const navigate = useNavigate();
  const { q: search = "" } = Route.useSearch();
  const feedback = useMutationFeedback();
  const enabled = client !== null && selectedInstanceId !== null;
  const databases = useQuery({
    queryKey: ["cloudflare-v4", "d1", selectedInstanceId, "databases"],
    queryFn: ({ signal }) =>
      client!.d1.database.list({ account_id: selectedInstanceId! }, { signal }),
    enabled,
  });
  const visible = useMemo(() => {
    const needle = search.trim().toLowerCase();
    return (databases.data?.result ?? [])
      .flatMap((item) =>
        item.uuid && item.name
          ? [{ ...item, uuid: item.uuid, name: item.name }]
          : [],
      )
      .filter((item) => item.name.toLowerCase().includes(needle));
  }, [databases.data, search]);
  function confirmDeleteDatabase(database: DatabaseRow) {
    openConfirmDeleteDialog({
      name: database.name,
      confirm: async () => {
        try {
          await client!.d1.database.delete(database.uuid, {
            account_id: selectedInstanceId!,
          });
        } catch (error) {
          feedback.failure(error, "Unable to delete the database.");
          throw error;
        }
        await databases.refetch();
        feedback.success("D1 database deleted.");
      },
    });
  }

  return (
    <div>
      <PageHeader
        title="D1 databases"
        description="Create serverless SQLite databases for relational application data."
        actions={
          <Button
            variant="primary"
            onClick={() => void navigate({ to: "/d1/new" })}
            disabled={!enabled}
          >
            <IconPlus size={16} /> Create database
          </Button>
        }
      />
      <CatalogToolbar
        value={search}
        onChange={(value) =>
          void navigate({
            to: "/d1",
            search: value ? { q: value } : {},
            replace: true,
          })
        }
        onRefresh={() => void databases.refetch()}
        refreshing={databases.isFetching}
        placeholder="Search databases"
      />
      {databases.isLoading ? (
        <LoadingRows />
      ) : databases.error ? (
        <ErrorState error={databases.error} />
      ) : visible.length === 0 ? (
        <EmptyState
          title={search ? "No matching databases" : "No D1 databases"}
          description={
            search
              ? "Try a different search term."
              : "Create a database to start storing relational data."
          }
          action={
            !search ? (
              <Button
                variant="primary"
                onClick={() => void navigate({ to: "/d1/new" })}
              >
                Create database
              </Button>
            ) : undefined
          }
        />
      ) : (
        <LayerCard className="overflow-hidden p-0">
          <div className="overflow-x-auto">
            <Table className="min-w-2xl">
              <Table.Header variant="compact">
                <Table.Row>
                  <Table.Head>Name</Table.Head>
                  <Table.Head>UUID</Table.Head>
                  <Table.Head>Created</Table.Head>
                  <Table.Head className="w-16">
                    <span className="sr-only">Actions</span>
                  </Table.Head>
                </Table.Row>
              </Table.Header>
              <Table.Body>
                {visible.map((database) => (
                  <Table.Row key={database.uuid}>
                    <Table.Cell>
                      <Link
                        className="inline-flex items-center gap-2 font-medium hover:underline"
                        to="/d1/$databaseId"
                        params={{ databaseId: database.uuid }}
                      >
                        <CloudflareProductIcon
                          product="D1"
                          size={16}
                          className="text-kumo-brand"
                        />
                        {database.name}
                      </Link>
                    </Table.Cell>
                    <Table.Cell className="text-kumo-subtle font-mono text-xs">
                      {database.uuid}
                    </Table.Cell>
                    <Table.Cell className="text-kumo-subtle">
                      {database.created_at
                        ? new Date(database.created_at).toLocaleString()
                        : "—"}
                    </Table.Cell>
                    <Table.Cell>
                      <Button
                        variant="ghost"
                        shape="square"
                        aria-label={`Delete ${database.name}`}
                        onClick={() => confirmDeleteDatabase(database)}
                      >
                        <IconTrash size={16} />
                      </Button>
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
