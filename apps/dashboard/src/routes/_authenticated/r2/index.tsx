import { Button } from "@cloudflare/kumo/components/button";
import { IconCaretLeft, IconCaretRight, IconPlus } from "@tabler/icons-react";
import { useQuery } from "@tanstack/react-query";
import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import {
  CatalogToolbar,
  EmptyState,
  ErrorState,
  LoadingRows,
  PageHeader,
} from "../../../components/dashboard-page";
import { DataTable } from "../../../components/page-layout";
import { useAuth } from "../../../features/auth/auth-atoms";
import { formatBytes } from "../../../lib/format";

export const Route = createFileRoute("/_authenticated/r2/")({
  validateSearch: (
    search: Record<string, unknown>,
  ): { q?: string; cursor?: string } => ({
    ...(typeof search.q === "string" && search.q ? { q: search.q } : {}),
    ...(typeof search.cursor === "string" && search.cursor
      ? { cursor: search.cursor }
      : {}),
  }),
  component: R2Page,
});

function R2Page() {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const navigate = useNavigate();
  const { q: filter = "", cursor: startAfter } = Route.useSearch();
  const enabled = client !== null && selectedInstanceId !== null;
  const [previous, setPrevious] = useState<(string | undefined)[]>([]);

  const buckets = useQuery({
    queryKey: [
      "cloudflare-v4",
      "r2",
      selectedInstanceId,
      "buckets",
      filter,
      startAfter,
    ],
    queryFn: ({ signal }) =>
      client!.r2.buckets.list(
        {
          account_id: selectedInstanceId!,
          per_page: 11,
          ...(filter ? { name_contains: filter } : {}),
          ...(startAfter ? { start_after: startAfter } : {}),
        },
        { signal },
      ),
    enabled,
  });
  const rows = (buckets.data?.buckets ?? []).slice(0, 10);
  const hasNext = (buckets.data?.buckets?.length ?? 0) > 10;
  const usage = useQuery({
    queryKey: [
      "cloudflare-v4",
      "r2",
      selectedInstanceId,
      "bucket-usage",
      rows.map((bucket) => bucket.name),
    ],
    queryFn: async ({ signal }) =>
      Object.fromEntries(
        await Promise.all(
          rows
            .filter((bucket) => bucket.name)
            .map(async (bucket) => [
              bucket.name!,
              await client!.openCompute.r2.usage.get(
                selectedInstanceId!,
                bucket.name!,
                {
                  signal,
                },
              ),
            ]),
        ),
      ),
    enabled: enabled && rows.length > 0,
  });

  return (
    <>
      <PageHeader
        title="R2 object storage"
        description="Store files and unstructured objects."
        actions={
          <Button
            variant="primary"
            onClick={() => void navigate({ to: "/r2/new" })}
            disabled={!enabled}
          >
            <IconPlus size={16} /> Create bucket
          </Button>
        }
      />
      <div className="min-w-0">
        <CatalogToolbar
          value={filter}
          onChange={(value) => {
            setPrevious([]);
            void navigate({
              to: "/r2",
              search: value ? { q: value } : {},
              replace: true,
            });
          }}
          onRefresh={() => {
            void buckets.refetch();
            void usage.refetch();
          }}
          refreshing={buckets.isFetching || usage.isFetching}
          placeholder="Search buckets"
        />
        {buckets.isPending ? (
          <LoadingRows />
        ) : buckets.error ? (
          <ErrorState error={buckets.error} />
        ) : rows.length === 0 ? (
          <EmptyState
            title={filter ? "No matching buckets" : "No R2 buckets"}
            description={
              filter
                ? "Try a different search term."
                : "Create a bucket to upload your first object."
            }
            action={
              !filter ? (
                <Button
                  variant="primary"
                  onClick={() => void navigate({ to: "/r2/new" })}
                >
                  Create bucket
                </Button>
              ) : undefined
            }
          />
        ) : (
          <>
            {usage.error && (
              <p role="alert" className="text-kumo-danger mb-3 text-sm">
                Bucket usage is unavailable. Refresh to retry.
              </p>
            )}
            <DataTable
              columns={[
                { key: "bucket", label: "Bucket" },
                { key: "objects", label: "Objects" },
                { key: "size", label: "Size" },
              ]}
              rows={rows.flatMap((bucket) => {
                if (!bucket.name) return [];
                const current = usage.data?.[bucket.name];
                return [
                  {
                    bucket: (
                      <Link
                        className="text-kumo-link underline"
                        to="/r2/$bucketId"
                        params={{ bucketId: bucket.name }}
                        search={{ prefix: "" }}
                      >
                        {bucket.name}
                      </Link>
                    ),
                    objects: current
                      ? current.object_count.toLocaleString()
                      : "—",
                    size:
                      current?.size_bytes == null
                        ? "—"
                        : formatBytes(current.size_bytes),
                  },
                ];
              })}
            />
            <div className="mt-4 flex justify-end gap-1">
              <Button
                variant="secondary"
                shape="square"
                aria-label="Previous page"
                disabled={previous.length === 0 || buckets.isFetching}
                onClick={() => {
                  const cursor = previous.at(-1);
                  setPrevious((pages) => pages.slice(0, -1));
                  void navigate({
                    to: "/r2",
                    search: {
                      ...(filter ? { q: filter } : {}),
                      ...(cursor ? { cursor } : {}),
                    },
                  });
                }}
              >
                <IconCaretLeft size={16} />
              </Button>
              <Button
                variant="secondary"
                shape="square"
                aria-label="Next page"
                disabled={!hasNext || buckets.isFetching}
                onClick={() => {
                  const lastName = rows.at(-1)?.name;
                  if (!lastName) return;
                  setPrevious((pages) => [...pages, startAfter]);
                  void navigate({
                    to: "/r2",
                    search: {
                      ...(filter ? { q: filter } : {}),
                      cursor: lastName,
                    },
                  });
                }}
              >
                <IconCaretRight size={16} />
              </Button>
            </div>
          </>
        )}
      </div>
    </>
  );
}
