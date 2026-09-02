import { useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { Button } from "@cloudflare/kumo/components/button";
import { CatalogToolbar } from "./CatalogToolbar";
import { ConfirmDeleteResourceDialog } from "./ConfirmDeleteResourceDialog";
import { CreateResourceDialog } from "./CreateResourceDialog";
import { DataTable, ErrorState, LoadingState, PageHeader } from "./PageLayout";
import { RenameResourceDialog } from "./RenameResourceDialog";
import { RowActionsMenu } from "./RowActionsMenu";
import { useAuth } from "../features/auth/AuthProvider";
import type { ManagementClient } from "../lib/cloudflare";

export interface CatalogRow {
  id: string;
  name: string;
  detail?: string;
  href?: string;
}

interface OfficialCatalogProps {
  kind: string;
  description: string;
  load: (client: ManagementClient, accountID: string, signal: AbortSignal) => Promise<readonly CatalogRow[]>;
  create?: (client: ManagementClient, accountID: string, name: string) => Promise<unknown>;
  rename?: (client: ManagementClient, accountID: string, row: CatalogRow, name: string) => Promise<unknown>;
  remove?: (client: ManagementClient, accountID: string, row: CatalogRow) => Promise<unknown>;
  primaryAction?: React.ReactNode;
}

/** Catalog CRUD backed directly by official Cloudflare SDK resources. */
export function OfficialCatalog({ kind, description, load, create, rename, remove, primaryAction }: OfficialCatalogProps) {
  const { client, accountId } = useAuth();
  const [createOpen, setCreateOpen] = useState(false);
  const [renameTarget, setRenameTarget] = useState<CatalogRow | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<CatalogRow | null>(null);
  const [mutationError, setMutationError] = useState<string | null>(null);
  const query = useQuery({
    queryKey: ["cloudflare-v4", kind, accountId],
    queryFn: ({ signal }) => load(client!, accountId!, signal),
    enabled: client !== null && accountId !== null,
  });
  const createMutation = useMutation({
    mutationFn: (name: string) => create!(client!, accountId!, name),
    onSuccess: async () => { setCreateOpen(false); setMutationError(null); await query.refetch(); },
    onError: error => setMutationError(error instanceof Error ? error.message : `Unable to create ${kind}.`),
  });
  const renameMutation = useMutation({
    mutationFn: (name: string) => rename!(client!, accountId!, renameTarget!, name),
    onSuccess: async () => { setRenameTarget(null); setMutationError(null); await query.refetch(); },
    onError: error => setMutationError(error instanceof Error ? error.message : `Unable to rename ${kind}.`),
  });
  const deleteMutation = useMutation({
    mutationFn: () => remove!(client!, accountId!, deleteTarget!),
    onSuccess: async () => { setDeleteTarget(null); setMutationError(null); await query.refetch(); },
    onError: error => setMutationError(error instanceof Error ? error.message : `Unable to delete ${kind}.`),
  });
  return (
    <div>
      <PageHeader title={kind} description={description} />
      <CatalogToolbar onRefresh={() => void query.refetch()} isRefreshing={query.isFetching} primaryAction={primaryAction ?? (create ? (
        <Button variant="primary" onClick={() => setCreateOpen(true)}>Create</Button>
      ) : undefined)} />
      {create ? <CreateResourceDialog title={`Create ${kind}`} description="Create this account-scoped resource through the Cloudflare v4 API." nameLabel="Name" namePlaceholder="resource-name" submitLabel="Create" open={createOpen} errorMessage={createOpen ? mutationError : null} isPending={createMutation.isPending} onClose={() => { setCreateOpen(false); setMutationError(null); }} onSubmit={name => createMutation.mutate(name)} /> : null}
      {rename ? <RenameResourceDialog title={`Rename ${kind}`} description="Update this resource through the Cloudflare v4 API." nameLabel="Name" currentName={renameTarget?.name ?? ""} open={renameTarget !== null} errorMessage={renameTarget ? mutationError : null} isPending={renameMutation.isPending} onClose={() => { setRenameTarget(null); setMutationError(null); }} onSubmit={name => renameMutation.mutate(name)} /> : null}
      {remove ? <ConfirmDeleteResourceDialog title={`Delete ${kind}`} description="This operation cannot be undone." resourceLabel="resource name" confirmValue={deleteTarget?.name ?? ""} open={deleteTarget !== null} errorMessage={deleteTarget ? mutationError : null} isPending={deleteMutation.isPending} onClose={() => { setDeleteTarget(null); setMutationError(null); }} onConfirm={() => deleteMutation.mutate()} /> : null}
      {query.isLoading ? <LoadingState /> : query.error ? (
        <ErrorState message={`Unable to load ${kind}.`} />
      ) : (
        <DataTable
          columns={[
            { key: "name", label: "Name" },
            { key: "id", label: "ID" },
            { key: "detail", label: "Details" },
            ...(rename || remove ? [{ key: "actions", label: "" }] : []),
          ]}
          rows={(query.data ?? []).map(row => ({
            name: row.href === undefined ? row.name : <a className="text-kumo-link hover:underline" href={row.href}>{row.name}</a>,
            id: <code className="[font-size:0.9em]">{row.id}</code>,
            detail: row.detail ?? "—",
            ...(rename || remove ? { actions: <RowActionsMenu label={row.name} actions={[
              ...(rename ? [{ id: "rename", label: "Rename", onSelect: () => setRenameTarget(row) }] : []),
              ...(remove ? [{ id: "delete", label: "Delete", variant: "danger" as const, onSelect: () => setDeleteTarget(row) }] : []),
            ]} /> } : {}),
          }))}
          emptyLabel={`No ${kind.toLowerCase()} were returned by the Cloudflare v4 API.`}
        />
      )}
    </div>
  );
}
