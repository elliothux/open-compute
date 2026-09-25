import { Badge } from "@cloudflare/kumo/components/badge";
import { Button } from "@cloudflare/kumo/components/button";
import { Input } from "@cloudflare/kumo/components/input";
import { LayerCard } from "@cloudflare/kumo/components/layer-card";
import { Popover } from "@cloudflare/kumo/components/popover";
import { Select } from "@cloudflare/kumo/components/select";
import { Table } from "@cloudflare/kumo/components/table";
import {
  IconFilter,
  IconRefresh,
  IconUpload,
  IconX,
} from "@tabler/icons-react";
import { useQuery } from "@tanstack/react-query";
import { Fragment, useDeferredValue, useState } from "react";
import { useAuth } from "../features/auth/auth-atoms";
import { AISearchItemDetail } from "./ai-search-item-detail";
import { AISearchUploadDialog } from "./ai-search-upload-dialog";
import { ErrorState, LoadingRows } from "./dashboard-page";
import { SearchInput } from "./search-input";

type MetadataField = {
  field_name: string;
  data_type: "text" | "number" | "boolean" | "datetime";
};
type MetadataCondition = { field: string; operator: string; value: string };
type AppliedFilters = { source: string; metadata: MetadataCondition[] };
type ItemStatus =
  "queued" | "running" | "completed" | "error" | "skipped" | "outdated";

const statuses: { value: ItemStatus | "all"; label: string }[] = [
  { value: "all", label: "All" },
  { value: "completed", label: "Indexed" },
  { value: "queued", label: "Queued" },
  { value: "running", label: "Processing" },
  { value: "outdated", label: "Outdated" },
  { value: "skipped", label: "Skipped" },
  { value: "error", label: "Error" },
];

function metadataQuery(
  conditions: MetadataCondition[],
  fields: MetadataField[],
) {
  if (!conditions.length) return undefined;
  const filter: Record<string, unknown> = {};
  for (const condition of conditions) {
    const field = fields.find((entry) => entry.field_name === condition.field);
    if (!field || !condition.value.trim()) continue;
    const value =
      field.data_type === "number"
        ? Number(condition.value)
        : field.data_type === "boolean"
          ? condition.value === "true"
          : field.data_type === "datetime"
            ? Date.parse(condition.value)
            : condition.value;
    if (typeof value === "number" && !Number.isFinite(value)) continue;
    filter[condition.field] =
      condition.operator === "$eq" ? value : { [condition.operator]: value };
  }
  return Object.keys(filter).length ? JSON.stringify(filter) : undefined;
}

function validConditions(
  conditions: MetadataCondition[],
  fields: MetadataField[],
) {
  return (
    new Set(conditions.map((condition) => condition.field)).size ===
      conditions.length &&
    conditions.every((condition) => {
      const field = fields.find(
        (entry) => entry.field_name === condition.field,
      );
      return (
        field &&
        condition.value.trim() &&
        (field.data_type !== "number" ||
          Number.isFinite(Number(condition.value))) &&
        (field.data_type !== "datetime" ||
          Number.isFinite(Date.parse(condition.value)))
      );
    })
  );
}

export function AISearchItems({
  namespaceName,
  instanceId,
  source,
  sourceType,
  metadataFields,
}: {
  namespaceName: string;
  instanceId: string;
  source?: string | null | undefined;
  sourceType?: "r2" | "web-crawler" | null | undefined;
  metadataFields: MetadataField[];
}) {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const [uploadOpen, setUploadOpen] = useState(false);
  const [selectedItemId, setSelectedItemId] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const deferredSearch = useDeferredValue(search.trim());
  const [status, setStatus] = useState<ItemStatus | "all">("all");
  const [page, setPage] = useState(1);
  const [filterOpen, setFilterOpen] = useState(false);
  const [applied, setApplied] = useState<AppliedFilters>({
    source: "",
    metadata: [],
  });
  const [draft, setDraft] = useState<AppliedFilters>(applied);
  const sourceOptions = [
    { value: "builtin", label: "Built-in storage" },
    ...(sourceType && source
      ? [{ value: `${sourceType}:${source}`, label: source }]
      : []),
  ];
  const filter = metadataQuery(applied.metadata, metadataFields);
  const appliedFilterCount =
    Number(Boolean(applied.source)) + applied.metadata.length;
  const hasSearchOrFilters =
    Boolean(deferredSearch) || status !== "all" || appliedFilterCount > 0;
  const clearAllFilters = () => {
    setSearch("");
    setStatus("all");
    setApplied({ source: "", metadata: [] });
    setPage(1);
  };
  const items = useQuery({
    queryKey: [
      "ai-search",
      selectedInstanceId,
      namespaceName,
      instanceId,
      "items",
      page,
      deferredSearch,
      status,
      applied,
    ],
    queryFn: ({ signal }) =>
      client!.aiSearch.namespaces.instances.items.list(
        instanceId,
        {
          account_id: selectedInstanceId!,
          name: namespaceName,
          page,
          per_page: 20,
          sort_by: "modified_at",
          ...(deferredSearch ? { search: deferredSearch } : {}),
          ...(status === "all" ? {} : { status }),
          ...(applied.source ? { source: applied.source } : {}),
          ...(filter ? { metadata_filter: filter } : {}),
        },
        { signal },
      ),
    enabled: client !== null && selectedInstanceId !== null,
  });
  return (
    <div className="grid gap-6">
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="primary" onClick={() => setUploadOpen(true)}>
          <IconUpload size={16} />
          Upload file
        </Button>
        <div className="flex min-w-48 flex-1">
          <SearchInput
            aria-label="Search items"
            className="min-w-0 flex-1 rounded-r-none"
            placeholder="Search items"
            value={search}
            onChange={(event) => {
              setSearch(event.target.value);
              setPage(1);
            }}
          />
          <Button
            className="rounded-l-none"
            shape="square"
            variant="secondary"
            aria-label="Refresh items"
            onClick={() => items.refetch()}
          >
            <IconRefresh
              size={16}
              className={items.isFetching ? "animate-spin" : ""}
            />
          </Button>
        </div>
        <Select
          aria-label="Item status"
          className="w-36"
          value={status}
          renderValue={(value) =>
            statuses.find((entry) => entry.value === value)?.label ?? value
          }
          onValueChange={(value) => {
            setStatus((value ?? "all") as ItemStatus | "all");
            setPage(1);
          }}
        >
          {statuses.map((entry) => (
            <Select.Option key={entry.value} value={entry.value}>
              {entry.label}
            </Select.Option>
          ))}
        </Select>
        <Popover
          open={filterOpen}
          onOpenChange={(open) => {
            setFilterOpen(open);
            if (open)
              setDraft({
                source: applied.source,
                metadata: [...applied.metadata],
              });
          }}
        >
          <Popover.Trigger render={<Button variant="secondary" />}>
            <IconFilter size={16} /> Filters
            {appliedFilterCount > 0 ? (
              <span className="bg-kumo-recessed rounded px-1.5 text-xs">
                {appliedFilterCount}
              </span>
            ) : null}
          </Popover.Trigger>
          <Popover.Content align="end" className="p-4">
            <Popover.Title>Filters</Popover.Title>
            <div className="mt-4 grid gap-4">
              <Select
                label="Source"
                value={draft.source || "all"}
                renderValue={(value) =>
                  value === "all"
                    ? "All sources"
                    : (sourceOptions.find((entry) => entry.value === value)
                        ?.label ?? value)
                }
                onValueChange={(value) =>
                  setDraft({
                    ...draft,
                    source: value === "all" ? "" : (value ?? ""),
                  })
                }
              >
                <Select.Option value="all">All sources</Select.Option>
                {sourceOptions.map((option) => (
                  <Select.Option key={option.value} value={option.value}>
                    {option.label}
                  </Select.Option>
                ))}
              </Select>
              <div className="grid gap-2">
                <span className="text-kumo-subtle text-sm">Metadata</span>
                {draft.metadata.map((condition, index) => {
                  const field = metadataFields.find(
                    (entry) => entry.field_name === condition.field,
                  );
                  const operators =
                    field?.data_type === "number" ||
                    field?.data_type === "datetime"
                      ? [
                          { value: "$eq", label: "Equals" },
                          { value: "$gt", label: "Greater than" },
                          { value: "$gte", label: "Greater than or equal" },
                          { value: "$lt", label: "Less than" },
                          { value: "$lte", label: "Less than or equal" },
                        ]
                      : [{ value: "$eq", label: "Equals" }];
                  const update = (change: Partial<MetadataCondition>) =>
                    setDraft({
                      ...draft,
                      metadata: draft.metadata.map((entry, at) =>
                        at === index ? { ...entry, ...change } : entry,
                      ),
                    });
                  return (
                    <div
                      className="grid grid-cols-4 items-end gap-2"
                      key={index}
                    >
                      <Select
                        label="Field"
                        value={condition.field}
                        renderValue={(value) =>
                          metadataFields.find(
                            (entry) => entry.field_name === value,
                          )
                            ? `${value} (${metadataFields.find((entry) => entry.field_name === value)?.data_type})`
                            : value
                        }
                        onValueChange={(value) =>
                          update({
                            field: value ?? "",
                            operator: "$eq",
                            value: "",
                          })
                        }
                      >
                        {metadataFields.map((entry) => (
                          <Select.Option
                            key={entry.field_name}
                            value={entry.field_name}
                          >
                            {entry.field_name} ({entry.data_type})
                          </Select.Option>
                        ))}
                      </Select>
                      <Select
                        label="Operator"
                        value={condition.operator}
                        renderValue={(value) =>
                          operators.find((entry) => entry.value === value)
                            ?.label ?? value
                        }
                        onValueChange={(value) =>
                          update({ operator: value ?? "$eq" })
                        }
                      >
                        {operators.map((entry) => (
                          <Select.Option key={entry.value} value={entry.value}>
                            {entry.label}
                          </Select.Option>
                        ))}
                      </Select>
                      {field?.data_type === "boolean" ? (
                        <Select
                          label="Value"
                          value={condition.value || "false"}
                          renderValue={(value) =>
                            value === "true" ? "True" : "False"
                          }
                          onValueChange={(value) =>
                            update({ value: value ?? "false" })
                          }
                        >
                          <Select.Option value="false">False</Select.Option>
                          <Select.Option value="true">True</Select.Option>
                        </Select>
                      ) : (
                        <Input
                          label="Value"
                          type={
                            field?.data_type === "number"
                              ? "number"
                              : field?.data_type === "datetime"
                                ? "datetime-local"
                                : "text"
                          }
                          value={condition.value}
                          onChange={(event) =>
                            update({ value: event.target.value })
                          }
                        />
                      )}
                      <Button
                        shape="square"
                        variant="secondary"
                        aria-label="Remove filter"
                        onClick={() =>
                          setDraft({
                            ...draft,
                            metadata: draft.metadata.filter(
                              (_, at) => at !== index,
                            ),
                          })
                        }
                      >
                        <IconX size={16} />
                      </Button>
                    </div>
                  );
                })}
                <div>
                  <Button
                    variant="secondary"
                    disabled={
                      metadataFields.length === 0 ||
                      draft.metadata.length >= metadataFields.length
                    }
                    onClick={() => {
                      const next = metadataFields.find(
                        (field) =>
                          !draft.metadata.some(
                            (condition) => condition.field === field.field_name,
                          ),
                      );
                      if (next)
                        setDraft({
                          ...draft,
                          metadata: [
                            ...draft.metadata,
                            {
                              field: next.field_name,
                              operator: "$eq",
                              value:
                                next.data_type === "boolean" ? "false" : "",
                            },
                          ],
                        });
                    }}
                  >
                    Add filter
                  </Button>
                </div>
                {metadataFields.length === 0 ? (
                  <p className="text-kumo-subtle text-sm">
                    No filterable metadata fields are configured.
                  </p>
                ) : null}
              </div>
            </div>
            <div className="border-kumo-line mt-4 flex items-center justify-between border-t pt-4">
              <Button
                variant="ghost"
                onClick={() => setDraft({ source: "", metadata: [] })}
              >
                Clear all
              </Button>
              <div className="flex gap-2">
                <Button
                  variant="secondary"
                  onClick={() => setFilterOpen(false)}
                >
                  Cancel
                </Button>
                <Button
                  variant="primary"
                  disabled={!validConditions(draft.metadata, metadataFields)}
                  onClick={() => {
                    setApplied(draft);
                    setPage(1);
                    setFilterOpen(false);
                  }}
                >
                  Apply
                </Button>
              </div>
            </div>
          </Popover.Content>
        </Popover>
      </div>
      {items.isLoading ? (
        <LoadingRows />
      ) : items.error ? (
        <ErrorState error={items.error} />
      ) : !items.data?.result.length ? (
        <LayerCard
          className={`flex flex-col items-center justify-center gap-2 px-4 py-5 text-center ${hasSearchOrFilters ? "min-h-64" : "min-h-52"}`}
        >
          <h2 className="text-xl font-semibold">
            {hasSearchOrFilters ? "No matching items" : "No items found"}
          </h2>
          <p className="text-kumo-subtle text-sm">
            {hasSearchOrFilters
              ? "Try adjusting your search or filters to find what you're looking for."
              : "Items will appear here after the source is indexed."}
          </p>
          {hasSearchOrFilters ? (
            <Button variant="secondary" onClick={clearAllFilters}>
              Clear all filters
            </Button>
          ) : null}
        </LayerCard>
      ) : (
        <div className="ring-kumo-line overflow-x-auto rounded-lg ring">
          <Table className="min-w-4xl">
            <Table.Header variant="compact">
              <Table.Row>
                {[
                  "Public ID",
                  "Status",
                  "Key",
                  "Chunks",
                  "File size",
                  "Source",
                  "Last seen",
                ].map((heading) => (
                  <Table.Head key={heading}>{heading}</Table.Head>
                ))}
              </Table.Row>
            </Table.Header>
            <Table.Body>
              {items.data.result.map((item) => (
                <Fragment key={item.id}>
                  <Table.Row
                    className="hover:bg-kumo-recessed cursor-pointer"
                    onClick={() =>
                      setSelectedItemId(
                        selectedItemId === item.id ? null : item.id,
                      )
                    }
                    onKeyDown={(event) => {
                      if (event.key === "Enter" || event.key === " ") {
                        event.preventDefault();
                        setSelectedItemId(
                          selectedItemId === item.id ? null : item.id,
                        );
                      }
                    }}
                    role="button"
                    tabIndex={0}
                    aria-expanded={selectedItemId === item.id}
                  >
                    <Table.Cell
                      className="max-w-56 truncate font-mono text-xs"
                      title={item.id}
                    >
                      {item.id}
                    </Table.Cell>
                    <Table.Cell>
                      <Badge
                        appearance="dot"
                        variant={
                          item.status === "completed"
                            ? "success"
                            : item.status === "error"
                              ? "error"
                              : "warning"
                        }
                      >
                        {statuses.find((entry) => entry.value === item.status)
                          ?.label ?? item.status}
                      </Badge>
                    </Table.Cell>
                    <Table.Cell className="font-medium">{item.key}</Table.Cell>
                    <Table.Cell>{item.chunks_count ?? "—"}</Table.Cell>
                    <Table.Cell>
                      {item.file_size === null
                        ? "—"
                        : item.file_size < 1024
                          ? `${item.file_size} B`
                          : `${(item.file_size / 1024).toFixed(1)} KB`}
                    </Table.Cell>
                    <Table.Cell>
                      {item.source_id === "builtin"
                        ? "Uploaded"
                        : (item.source_id ?? "—")}
                    </Table.Cell>
                    <Table.Cell>
                      {new Date(item.last_seen_at).toLocaleString()}
                    </Table.Cell>
                  </Table.Row>
                  {selectedItemId === item.id ? (
                    <Table.Row>
                      <Table.Cell colSpan={7} className="p-0">
                        <AISearchItemDetail
                          itemId={item.id}
                          namespaceName={namespaceName}
                          instanceId={instanceId}
                          onDeleted={() => setSelectedItemId(null)}
                        />
                      </Table.Cell>
                    </Table.Row>
                  ) : null}
                </Fragment>
              ))}
            </Table.Body>
          </Table>
        </div>
      )}
      {page > 1 || (items.data?.result.length ?? 0) === 20 ? (
        <div className="flex items-center justify-end gap-3 text-sm">
          <Button
            variant="secondary"
            disabled={page === 1}
            onClick={() => setPage(page - 1)}
          >
            Previous
          </Button>
          <span>Page {page}</span>
          <Button
            variant="secondary"
            disabled={(items.data?.result.length ?? 0) < 20}
            onClick={() => setPage(page + 1)}
          >
            Next
          </Button>
        </div>
      ) : null}
      <AISearchUploadDialog
        open={uploadOpen}
        onOpenChange={setUploadOpen}
        namespaceName={namespaceName}
        instanceId={instanceId}
      />
    </div>
  );
}
