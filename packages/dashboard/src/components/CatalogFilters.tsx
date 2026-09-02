import { Select } from "@cloudflare/kumo/components/select";
import { z } from "zod";

export const catalogSearchSchema = z.object({
  q: z.string().optional(),
  status: z.enum(["all", "creating", "ready", "deleting"]).optional(),
  sort: z.enum(["name", "createdAt", "updatedAt"]).optional(),
  direction: z.enum(["asc", "desc"]).optional(),
});

export type CatalogStatus = "all" | "creating" | "ready" | "deleting";
export type CatalogSort = "name" | "createdAt" | "updatedAt";
export type CatalogDirection = "asc" | "desc";

const statusLabels: Record<CatalogStatus, string> = {
  all: "All statuses",
  ready: "Ready",
  creating: "Creating",
  deleting: "Deleting",
};

const sortLabels: Record<`${CatalogSort}:${CatalogDirection}`, string> = {
  "updatedAt:desc": "Recently updated",
  "updatedAt:asc": "Least recently updated",
  "createdAt:desc": "Recently created",
  "createdAt:asc": "Oldest created",
  "name:asc": "Name A–Z",
  "name:desc": "Name Z–A",
};

export function CatalogFilters({
  status,
  sort,
  direction,
  onStatusChange,
  onSortChange,
}: {
  status: CatalogStatus;
  sort: CatalogSort;
  direction: CatalogDirection;
  onStatusChange: (value: CatalogStatus) => void;
  onSortChange: (sort: CatalogSort, direction: CatalogDirection) => void;
}) {
  return (
    <>
      <Select
        aria-label="Resource status"
        value={status}
        renderValue={value => statusLabels[value] ?? value}
        onValueChange={value => {
          if (value) onStatusChange(value);
        }}
      >
        <Select.Option value="all">All statuses</Select.Option>
        <Select.Option value="ready">Ready</Select.Option>
        <Select.Option value="creating">Creating</Select.Option>
        <Select.Option value="deleting">Deleting</Select.Option>
      </Select>
      <Select
        aria-label="Sort catalog"
        value={`${sort}:${direction}`}
        renderValue={value => sortLabels[value as `${CatalogSort}:${CatalogDirection}`] ?? value}
        onValueChange={value => {
          if (!value) return;
          const [nextSort, nextDirection] = value.split(":") as [CatalogSort, CatalogDirection];
          onSortChange(nextSort, nextDirection);
        }}
      >
        <Select.Option value="updatedAt:desc">Recently updated</Select.Option>
        <Select.Option value="createdAt:desc">Recently created</Select.Option>
        <Select.Option value="name:asc">Name A–Z</Select.Option>
        <Select.Option value="name:desc">Name Z–A</Select.Option>
      </Select>
    </>
  );
}
