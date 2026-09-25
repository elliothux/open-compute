import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Input } from "@cloudflare/kumo/components/input";
import { Select } from "@cloudflare/kumo/components/select";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
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
import { useAuth } from "../../../features/auth/auth-atoms";
import { useMutationFeedback } from "../../../features/toast/use-mutation-feedback";

export const Route = createFileRoute("/_authenticated/vectorize/")({
  validateSearch: (search: Record<string, unknown>): { q?: string } =>
    typeof search.q === "string" && search.q ? { q: search.q } : {},
  component: VectorizePage,
});

type Metric = "cosine" | "euclidean" | "dot-product";

function VectorizePage() {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const { q: filter = "" } = Route.useSearch();
  const navigate = useNavigate();
  const feedback = useMutationFeedback();
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [dimensions, setDimensions] = useState("768");
  const [metric, setMetric] = useState<Metric>("cosine");

  const indexes = useQuery({
    queryKey: ["vectorize", selectedInstanceId],
    queryFn: async ({ signal }) => {
      const page = await client!.vectorize.indexes.list(
        { account_id: selectedInstanceId! },
        { signal },
      );
      return page.result;
    },
    enabled: client !== null && selectedInstanceId !== null,
  });

  const createIndex = useMutation({
    mutationFn: () =>
      client!.vectorize.indexes.create({
        account_id: selectedInstanceId!,
        name: name.trim(),
        config: { dimensions: Number(dimensions), metric },
      }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({
        queryKey: ["vectorize", selectedInstanceId],
      });
      setOpen(false);
      setName("");
      feedback.success("Vectorize index created.");
    },
    onError: (error) => feedback.failure(error, "Unable to create the index."),
  });

  const needle = filter.trim().toLowerCase();
  const filtered = (indexes.data ?? []).filter((index) =>
    `${index.name ?? ""} ${index.description ?? ""}`
      .toLowerCase()
      .includes(needle),
  );
  const dimensionCount = Number(dimensions);
  const valid =
    name.trim().length > 0 &&
    Number.isInteger(dimensionCount) &&
    dimensionCount > 0 &&
    dimensionCount <= 1536;

  return (
    <div>
      <PageHeader
        title="Vectorize"
        description="Create and manage vector indexes for semantic search and retrieval."
        actions={<Button onClick={() => setOpen(true)}>Create index</Button>}
      />
      <CatalogToolbar
        value={filter}
        onChange={(value) =>
          void navigate({
            to: "/vectorize",
            search: value ? { q: value } : {},
            replace: true,
          })
        }
        onRefresh={() => void indexes.refetch()}
        refreshing={indexes.isFetching}
        placeholder="Search indexes"
      />
      {indexes.isLoading ? (
        <LoadingRows />
      ) : indexes.error ? (
        <ErrorState error={indexes.error} />
      ) : filtered.length === 0 ? (
        <EmptyState
          title={
            needle ? "No matching indexes" : "Create your first Vectorize index"
          }
          description={
            needle
              ? "Try a different search term."
              : "Choose dimensions and a distance metric, then insert vector data."
          }
          action={
            !needle ? (
              <Button onClick={() => setOpen(true)}>Create index</Button>
            ) : undefined
          }
        />
      ) : (
        <ResourceList>
          {filtered.map((index) => {
            const indexName = index.name ?? "unnamed-index";
            return (
              <ResourceRow
                key={indexName}
                href={`/vectorize/${encodeURIComponent(indexName)}`}
                icon={<CloudflareProductIcon product="Vectorize" size={20} />}
                title={indexName}
                description={
                  index.description || `Created ${index.created_on ?? "—"}`
                }
                meta={
                  index.config
                    ? `${index.config.dimensions} dimensions`
                    : undefined
                }
                footer={
                  index.config
                    ? `${index.config.metric} distance`
                    : "Configuration unavailable"
                }
              />
            );
          })}
        </ResourceList>
      )}

      <Dialog.Root open={open} onOpenChange={setOpen}>
        <Dialog className="px-6 py-5" size="lg">
          <form
            onSubmit={(event) => {
              event.preventDefault();
              if (valid && !createIndex.isPending) createIndex.mutate();
            }}
          >
            <Dialog.Title>Create Vectorize index</Dialog.Title>
            <Dialog.Description>
              Index dimensions and distance metric cannot be changed after
              creation.
            </Dialog.Description>
            <div className="mt-5 grid gap-4">
              <Input
                label="Index name"
                placeholder="semantic-search"
                value={name}
                onChange={(event) => setName(event.target.value)}
                autoFocus
              />
              <Input
                label="Dimensions"
                type="number"
                min={1}
                max={1536}
                value={dimensions}
                onChange={(event) => setDimensions(event.target.value)}
              />
              <Select
                label="Distance metric"
                value={metric}
                items={[
                  { label: "Cosine", value: "cosine" },
                  { label: "Euclidean", value: "euclidean" },
                  { label: "Dot product", value: "dot-product" },
                ]}
                onValueChange={(value) => setMetric(value as Metric)}
              />
            </div>
            {createIndex.error ? (
              <p className="text-kumo-danger mt-3 text-sm">
                {createIndex.error.message}
              </p>
            ) : null}
            <div className="mt-6 flex justify-end gap-2">
              <Button
                type="button"
                variant="secondary"
                onClick={() => setOpen(false)}
                disabled={createIndex.isPending}
              >
                Cancel
              </Button>
              <Button type="submit" disabled={!valid || createIndex.isPending}>
                {createIndex.isPending ? "Creating…" : "Create index"}
              </Button>
            </div>
          </form>
        </Dialog>
      </Dialog.Root>
    </div>
  );
}
