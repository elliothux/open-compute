import { Badge } from "@cloudflare/kumo/components/badge";
import { Button } from "@cloudflare/kumo/components/button";
import { Dialog } from "@cloudflare/kumo/components/dialog";
import { Input } from "@cloudflare/kumo/components/input";
import { Select } from "@cloudflare/kumo/components/select";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { CodeBlock } from "../../../components/code-block";
import {
  DefinitionList,
  EmptyState,
  ErrorState,
  LoadingRows,
  PageHeader,
  PageTabs,
  Panel,
  ResourceList,
  Section,
  StatGrid,
} from "../../../components/dashboard-page";
import { openConfirmDeleteDialog } from "../../../components/resource-dialog";
import { useAuth } from "../../../features/auth/auth-atoms";
import { useMutationFeedback } from "../../../features/toast/use-mutation-feedback";

type Tab = "overview" | "vectors" | "metadata";

export const Route = createFileRoute("/_authenticated/vectorize/$indexName")({
  validateSearch: (search: Record<string, unknown>): { tab?: Tab } =>
    search.tab === "vectors" || search.tab === "metadata"
      ? { tab: search.tab }
      : {},
  component: VectorizeDetailPage,
});

function VectorizeDetailPage() {
  const { indexName } = Route.useParams();
  const { tab: searchTab } = Route.useSearch();
  const tab = searchTab ?? "overview";
  const { client, instanceId: selectedInstanceId } = useAuth();
  const navigate = useNavigate();
  const feedback = useMutationFeedback();
  const queryClient = useQueryClient();
  const [queryVector, setQueryVector] = useState("");
  const [queryTopK, setQueryTopK] = useState("10");
  const [queryFilter, setQueryFilter] = useState("");
  const [returnValues, setReturnValues] = useState(false);
  const [matches, setMatches] = useState<Awaited<
    ReturnType<NonNullable<typeof client>["vectorize"]["indexes"]["query"]>
  > | null>(null);
  const [metadataOpen, setMetadataOpen] = useState(false);
  const [uploadOpen, setUploadOpen] = useState(false);
  const [uploadMode, setUploadMode] = useState<"insert" | "upsert">("insert");
  const [uploadFile, setUploadFile] = useState<File | null>(null);
  const [vectorId, setVectorId] = useState<string | null>(null);
  const [vectorDetail, setVectorDetail] = useState<unknown>(null);
  const [cursor, setCursor] = useState<string | undefined>();
  const [cursorHistory, setCursorHistory] = useState<(string | undefined)[]>(
    [],
  );
  const [propertyName, setPropertyName] = useState("");
  const [indexType, setIndexType] = useState<"string" | "number" | "boolean">(
    "string",
  );

  const details = useQuery({
    queryKey: ["vectorize", selectedInstanceId, indexName, "details"],
    queryFn: ({ signal }) =>
      Promise.all([
        client!.vectorize.indexes.get(
          indexName,
          { account_id: selectedInstanceId! },
          { signal },
        ),
        client!.vectorize.indexes.info(
          indexName,
          { account_id: selectedInstanceId! },
          { signal },
        ),
      ]),
    enabled: client !== null && selectedInstanceId !== null,
  });
  const vectors = useQuery({
    queryKey: ["vectorize", selectedInstanceId, indexName, "vectors", cursor],
    queryFn: ({ signal }) =>
      client!.vectorize.indexes.listVectors(
        indexName,
        {
          account_id: selectedInstanceId!,
          count: 100,
          ...(cursor ? { cursor } : {}),
        },
        { signal },
      ),
    enabled:
      client !== null && selectedInstanceId !== null && tab === "vectors",
  });
  const metadata = useQuery({
    queryKey: ["vectorize", selectedInstanceId, indexName, "metadata"],
    queryFn: ({ signal }) =>
      client!.vectorize.indexes.metadataIndex.list(
        indexName,
        { account_id: selectedInstanceId! },
        { signal },
      ),
    enabled:
      client !== null && selectedInstanceId !== null && tab === "metadata",
  });

  function confirmDeleteIndex() {
    openConfirmDeleteDialog({
      name: indexName,
      confirm: async () => {
        try {
          await client!.vectorize.indexes.delete(indexName, {
            account_id: selectedInstanceId!,
          });
        } catch (error) {
          feedback.failure(error, "Unable to delete the index.");
          throw error;
        }
        await queryClient.invalidateQueries({
          queryKey: ["vectorize", selectedInstanceId],
        });
        feedback.success("Vectorize index deleted.");
        await navigate({ to: "/vectorize" });
      },
    });
  }

  const runQuery = useMutation({
    mutationFn: () => {
      const vector = queryVector
        .split(/[\s,]+/)
        .filter(Boolean)
        .map(Number);
      if (
        vector.length === 0 ||
        vector.some((value) => !Number.isFinite(value))
      )
        throw new Error("Enter a comma-separated numeric vector.");
      if (
        details.data?.[1]?.dimensions &&
        vector.length !== details.data[1].dimensions
      )
        throw new Error(
          `This index requires ${details.data[1].dimensions} dimensions.`,
        );
      const topK = Number(queryTopK);
      if (!Number.isInteger(topK) || topK < 1 || topK > 50)
        throw new Error("Top K must be between 1 and 50.");
      let filter: unknown;
      if (queryFilter.trim()) {
        try {
          filter = JSON.parse(queryFilter);
        } catch {
          throw new Error("Metadata filter must be valid JSON.");
        }
      }
      return client!.vectorize.indexes.query(indexName, {
        account_id: selectedInstanceId!,
        vector,
        topK,
        returnMetadata: "all",
        returnValues,
        ...(filter === undefined ? {} : { filter }),
      });
    },
    onSuccess: setMatches,
    onError: (error) => feedback.failure(error, "Unable to query the index."),
  });
  const uploadVectors = useMutation({
    mutationFn: () => {
      if (!uploadFile || uploadFile.size === 0)
        throw new Error("Choose a non-empty NDJSON file.");
      if (uploadFile.size > 24 * 1024 * 1024)
        throw new Error("The NDJSON file must be 24 MiB or smaller.");
      return client!.vectorize.indexes[uploadMode](indexName, {
        account_id: selectedInstanceId!,
        body: uploadFile,
      });
    },
    onSuccess: async () => {
      setCursor(undefined);
      setCursorHistory([]);
      await queryClient.invalidateQueries({
        queryKey: ["vectorize", selectedInstanceId, indexName],
      });
      setUploadOpen(false);
      setUploadFile(null);
      feedback.success("Vector mutation queued.");
    },
    onError: (error) => feedback.failure(error, "Unable to upload vectors."),
  });
  const getVector = useMutation({
    mutationFn: (id: string) =>
      client!.vectorize.indexes.getByIDs(indexName, {
        account_id: selectedInstanceId!,
        ids: [id],
      }),
    onSuccess: setVectorDetail,
    onError: (error) => feedback.failure(error, "Unable to load the vector."),
  });
  function confirmDeleteVector() {
    openConfirmDeleteDialog({
      name: vectorId ?? "vector",
      confirm: async () => {
        try {
          await client!.vectorize.indexes.deleteByIDs(indexName, {
            account_id: selectedInstanceId!,
            ids: vectorId ? [vectorId] : [],
          });
        } catch (error) {
          feedback.failure(error, "Unable to delete the vector.");
          throw error;
        }
        await queryClient.invalidateQueries({
          queryKey: ["vectorize", selectedInstanceId, indexName],
        });
        setVectorId(null);
        setVectorDetail(null);
        feedback.success("Vector deletion queued.");
      },
    });
  }
  const createMetadata = useMutation({
    mutationFn: () =>
      client!.vectorize.indexes.metadataIndex.create(indexName, {
        account_id: selectedInstanceId!,
        propertyName: propertyName.trim(),
        indexType,
      }),
    onSuccess: async () => {
      await metadata.refetch();
      setMetadataOpen(false);
      setPropertyName("");
      feedback.success("Metadata index created.");
    },
    onError: (error) =>
      feedback.failure(error, "Unable to create the metadata index."),
  });
  const removeMetadata = useMutation({
    mutationFn: (property: string) =>
      client!.vectorize.indexes.metadataIndex.delete(indexName, {
        account_id: selectedInstanceId!,
        propertyName: property,
      }),
    onSuccess: async () => {
      await metadata.refetch();
      feedback.success("Metadata index deleted.");
    },
    onError: (error) =>
      feedback.failure(error, "Unable to delete the metadata index."),
  });

  const index = details.data?.[0];
  const info = details.data?.[1];
  const base = `/vectorize/${encodeURIComponent(indexName)}`;
  return (
    <div>
      <PageHeader
        title={indexName}
        description="Vectorize index"
        actions={
          <Button variant="destructive" onClick={confirmDeleteIndex}>
            Delete
          </Button>
        }
      />
      <PageTabs
        active={
          tab === "vectors"
            ? "Vectors and query"
            : tab === "metadata"
              ? "Metadata indexes"
              : "Overview"
        }
        items={[
          { label: "Overview", href: base },
          { label: "Vectors and query", href: `${base}?tab=vectors` },
          { label: "Metadata indexes", href: `${base}?tab=metadata` },
        ]}
      />
      {details.isLoading ? (
        <LoadingRows />
      ) : details.error ? (
        <ErrorState error={details.error} />
      ) : null}

      {tab === "overview" && details.data ? (
        <div className="grid gap-8">
          <StatGrid
            items={[
              { label: "Vectors", value: info?.vectorCount ?? 0 },
              {
                label: "Dimensions",
                value: info?.dimensions ?? index?.config?.dimensions ?? "—",
              },
              { label: "Distance metric", value: index?.config?.metric ?? "—" },
              {
                label: "Last processed",
                value: info?.processedUpToDatetime ?? "—",
              },
            ]}
          />
          <Section title="Index details">
            <Panel>
              <DefinitionList
                items={[
                  { label: "Name", value: index?.name ?? indexName },
                  { label: "Description", value: index?.description || "—" },
                  { label: "Created", value: index?.created_on ?? "—" },
                  { label: "Modified", value: index?.modified_on ?? "—" },
                  {
                    label: "Processed mutation",
                    value: info?.processedUpToMutation ?? "—",
                  },
                ]}
              />
            </Panel>
          </Section>
        </div>
      ) : null}

      {tab === "vectors" ? (
        <div className="grid gap-8">
          <Section
            title="Query index"
            description="Enter a vector with the same dimensions as this index."
          >
            <Panel className="grid gap-3">
              <Input
                label="Vector"
                placeholder="0.12, -0.45, 0.91"
                value={queryVector}
                onChange={(event) => setQueryVector(event.target.value)}
              />
              <div className="grid gap-3 sm:grid-cols-2">
                <Input
                  label="Top K"
                  type="number"
                  min={1}
                  max={50}
                  value={queryTopK}
                  onChange={(event) => setQueryTopK(event.target.value)}
                />
                <Input
                  label="Metadata filter (JSON)"
                  placeholder={'{"category":"docs"}'}
                  value={queryFilter}
                  onChange={(event) => setQueryFilter(event.target.value)}
                />
              </div>
              <label className="flex items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={returnValues}
                  onChange={(event) => setReturnValues(event.target.checked)}
                />
                Return vector values
              </label>
              <div>
                <Button
                  disabled={!queryVector.trim() || runQuery.isPending}
                  onClick={() => runQuery.mutate()}
                >
                  {runQuery.isPending ? "Querying…" : "Run query"}
                </Button>
              </div>
              {matches ? (
                <CodeBlock
                  className="bg-kumo-recessed max-h-80 overflow-auto rounded-lg p-3 text-xs"
                  code={JSON.stringify(matches, null, 2)}
                  language="json"
                />
              ) : null}
            </Panel>
          </Section>
          <Section
            title="Vectors"
            description={`${vectors.data?.totalCount ?? 0} vectors in this index.`}
          >
            <div>
              <Button onClick={() => setUploadOpen(true)}>
                Upload vectors
              </Button>
            </div>
            {vectors.isLoading ? (
              <LoadingRows />
            ) : vectors.error ? (
              <ErrorState error={vectors.error} />
            ) : !vectors.data?.vectors.length ? (
              <EmptyState
                title="No vectors"
                description="Upload an NDJSON file to insert or upsert vectors."
              />
            ) : (
              <ResourceList>
                {vectors.data.vectors.map((vector) => (
                  <Panel
                    key={vector.id}
                    className="flex items-center justify-between gap-3"
                  >
                    <span className="min-w-0 truncate font-medium">
                      {vector.id}
                    </span>
                    <Button
                      variant="secondary"
                      onClick={() => {
                        setVectorId(vector.id);
                        setVectorDetail(null);
                        getVector.mutate(vector.id);
                      }}
                    >
                      View
                    </Button>
                  </Panel>
                ))}
              </ResourceList>
            )}
            {cursorHistory.length || vectors.data?.isTruncated ? (
              <div className="flex gap-2">
                {cursorHistory.length ? (
                  <Button
                    variant="secondary"
                    onClick={() => {
                      const previous = cursorHistory.at(-1);
                      setCursorHistory((history) => history.slice(0, -1));
                      setCursor(previous);
                    }}
                  >
                    Previous page
                  </Button>
                ) : null}
                {vectors.data?.isTruncated && vectors.data.nextCursor ? (
                  <Button
                    variant="secondary"
                    onClick={() => {
                      setCursorHistory((history) => [...history, cursor]);
                      setCursor(vectors.data?.nextCursor ?? undefined);
                    }}
                  >
                    Next page
                  </Button>
                ) : null}
              </div>
            ) : null}
          </Section>
        </div>
      ) : null}

      {tab === "metadata" ? (
        <Section
          title="Metadata indexes"
          description="Enable filtered vector queries for selected metadata properties."
        >
          <div>
            <Button onClick={() => setMetadataOpen(true)}>
              Create metadata index
            </Button>
          </div>
          {metadata.isLoading ? (
            <LoadingRows />
          ) : metadata.error ? (
            <ErrorState error={metadata.error} />
          ) : !metadata.data?.metadataIndexes?.length ? (
            <EmptyState
              title="No metadata indexes"
              description="Create an index for a string, number or boolean metadata property."
            />
          ) : (
            <ResourceList>
              {metadata.data.metadataIndexes.map((item) => (
                <Panel
                  key={item.propertyName}
                  className="flex flex-wrap items-center gap-3"
                >
                  <span className="min-w-0 flex-1 font-medium">
                    {item.propertyName ?? "Unnamed property"}
                  </span>
                  <span>
                    <Badge variant="neutral">
                      {item.indexType ?? "unknown"}
                    </Badge>
                  </span>
                  <Button
                    variant="secondary"
                    disabled={removeMetadata.isPending}
                    onClick={() => {
                      if (item.propertyName)
                        removeMetadata.mutate(item.propertyName);
                    }}
                  >
                    Delete
                  </Button>
                </Panel>
              ))}
            </ResourceList>
          )}
        </Section>
      ) : null}

      <Dialog.Root open={uploadOpen} onOpenChange={setUploadOpen}>
        <Dialog className="px-6 py-5" size="lg">
          <form
            onSubmit={(event) => {
              event.preventDefault();
              if (uploadFile && !uploadVectors.isPending)
                uploadVectors.mutate();
            }}
          >
            <Dialog.Title>Upload vectors</Dialog.Title>
            <Dialog.Description>
              Choose an NDJSON file containing one vector per line. Upsert can
              update vectors with existing IDs.
            </Dialog.Description>
            <div className="mt-5 grid gap-4">
              <Select
                label="Operation"
                value={uploadMode}
                items={[
                  { label: "Insert", value: "insert" },
                  { label: "Upsert", value: "upsert" },
                ]}
                onValueChange={(value) =>
                  setUploadMode(value as typeof uploadMode)
                }
              />
              <label className="grid gap-1.5">
                <span className="font-medium">NDJSON file</span>
                <input
                  type="file"
                  accept=".ndjson,application/x-ndjson"
                  onChange={(event) =>
                    setUploadFile(event.target.files?.[0] ?? null)
                  }
                />
              </label>
              {uploadFile ? (
                <p className="text-kumo-subtle text-sm">
                  {uploadFile.name} · {uploadFile.size.toLocaleString()} bytes
                </p>
              ) : null}
            </div>
            {uploadVectors.error ? (
              <p className="text-kumo-danger mt-3 text-sm">
                {uploadVectors.error.message}
              </p>
            ) : null}
            <div className="mt-6 flex justify-end gap-2">
              <Button
                type="button"
                variant="secondary"
                onClick={() => setUploadOpen(false)}
              >
                Cancel
              </Button>
              <Button
                type="submit"
                disabled={!uploadFile || uploadVectors.isPending}
              >
                {uploadVectors.isPending ? "Uploading…" : "Upload"}
              </Button>
            </div>
          </form>
        </Dialog>
      </Dialog.Root>
      <Dialog.Root
        open={vectorId !== null}
        onOpenChange={(open) => {
          if (!open) setVectorId(null);
        }}
      >
        <Dialog className="px-6 py-5" size="lg">
          <Dialog.Title>{vectorId ?? "Vector"}</Dialog.Title>
          <Dialog.Description>Vector values and metadata</Dialog.Description>
          {getVector.isPending ? (
            <LoadingRows />
          ) : getVector.error ? (
            <ErrorState error={getVector.error} />
          ) : vectorDetail ? (
            <CodeBlock
              className="bg-kumo-recessed mt-5 max-h-96 overflow-auto rounded-lg p-3 text-xs"
              code={JSON.stringify(vectorDetail, null, 2)}
              language="json"
            />
          ) : null}
          <div className="mt-6 flex justify-end gap-2">
            <Button variant="secondary" onClick={() => setVectorId(null)}>
              Close
            </Button>
            <Button variant="destructive" onClick={confirmDeleteVector}>
              Delete
            </Button>
          </div>
        </Dialog>
      </Dialog.Root>
      <Dialog.Root open={metadataOpen} onOpenChange={setMetadataOpen}>
        <Dialog className="px-6 py-5" size="lg">
          <form
            onSubmit={(event) => {
              event.preventDefault();
              if (propertyName.trim() && !createMetadata.isPending)
                createMetadata.mutate();
            }}
          >
            <Dialog.Title>Create metadata index</Dialog.Title>
            <Dialog.Description>
              Select the property and value type used by filters.
            </Dialog.Description>
            <div className="mt-5 grid gap-4">
              <Input
                label="Property name"
                placeholder="category"
                value={propertyName}
                onChange={(event) => setPropertyName(event.target.value)}
                autoFocus
              />
              <Select
                label="Property type"
                value={indexType}
                items={[
                  { label: "String", value: "string" },
                  { label: "Number", value: "number" },
                  { label: "Boolean", value: "boolean" },
                ]}
                onValueChange={(value) =>
                  setIndexType(value as typeof indexType)
                }
              />
            </div>
            <div className="mt-6 flex justify-end gap-2">
              <Button
                type="button"
                variant="secondary"
                onClick={() => setMetadataOpen(false)}
              >
                Cancel
              </Button>
              <Button
                type="submit"
                disabled={!propertyName.trim() || createMetadata.isPending}
              >
                {createMetadata.isPending ? "Creating…" : "Create"}
              </Button>
            </div>
          </form>
        </Dialog>
      </Dialog.Root>
    </div>
  );
}
