import { Link } from "@tanstack/react-router";
import { DataTable, LoadingState, ErrorState, StatusBadge } from "./PageLayout";
import { Surface } from "@cloudflare/kumo/components/surface";
import { formatBytes, formatTimestamp } from "../lib/format";

function formatLabel(key: string): string {
  return key
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .replaceAll("_", " ")
    .replace(/^./, value => value.toUpperCase());
}

function formatScalar(key: string, value: unknown): string {
  if (value === null || value === undefined) return "—";
  if (typeof value === "boolean") return value ? "yes" : "no";
  if (typeof value === "object") return JSON.stringify(value);
  if (typeof value === "number" && /bytes$/i.test(key)) return formatBytes(value);
  if (typeof value === "number" && /at$/i.test(key)) return formatTimestamp(value);
  return String(value);
}

function ScalarGrid({ data }: { data: Record<string, unknown> }) {
  const entries = Object.entries(data).filter(([, value]) => value === null || typeof value !== "object");
  if (entries.length === 0) return null;
  return (
    <dl className="grid gap-3 sm:grid-cols-2">
      {entries.map(([key, value]) => (
        <div key={key}>
          <dt className="text-xs font-medium text-kumo-subtle">{formatLabel(key)}</dt>
          <dd className="mt-1 text-sm">{formatScalar(key, value)}</dd>
        </div>
      ))}
    </dl>
  );
}

function ArrayTable({ label, rows }: { label: string; rows: Record<string, unknown>[] }) {
  if (rows.length === 0) {
    return <p className="text-sm text-kumo-subtle">No {label.toLowerCase()} reported.</p>;
  }
  const columns = Object.keys(rows[0] ?? {}).slice(0, 6);
  return (
    <DataTable
      columns={columns.map(key => ({ key, label: formatLabel(key) }))}
      rows={rows.map(row =>
        Object.fromEntries(
          columns.map(key => [
            key,
            key === "state" && typeof row[key] === "string"
              ? <StatusBadge value={row[key]} />
              : formatScalar(key, row[key]),
          ]),
        ),
      )}
      emptyLabel={`No ${label.toLowerCase()} reported.`}
    />
  );
}

export function StructuredSummaryPanel({
  title,
  query,
  pick,
}: {
  title: string;
  query: {
    isLoading: boolean;
    error: Error | null;
    data: Record<string, unknown> | undefined;
  };
  pick?: (data: Record<string, unknown>) => {
    scalars?: Record<string, unknown>;
    tables?: Array<{ label: string; rows: Record<string, unknown>[] }>;
  };
}) {
  return (
    <Surface className="p-4">
      <div className="mb-3 text-sm font-medium">{title}</div>
      {query.isLoading ? (
        <LoadingState label={`Loading ${title.toLowerCase()}…`} />
      ) : query.error ? (
        <ErrorState message={`Unable to load ${title.toLowerCase()}.`} />
      ) : !query.data ? (
        <p className="text-sm text-kumo-subtle">No data.</p>
      ) : (
        <div className="space-y-4">
          {pick ? (
            (() => {
              const picked = pick(query.data);
              return (
                <>
                  {picked.scalars ? <ScalarGrid data={picked.scalars} /> : null}
                  {picked.tables?.map(table => (
                    <div key={table.label}>
                      <div className="mb-2 text-xs font-medium text-kumo-subtle">
                        {table.label}
                      </div>
                      <ArrayTable label={table.label} rows={table.rows} />
                    </div>
                  ))}
                </>
              );
            })()
          ) : (
            <ScalarGrid data={query.data} />
          )}
        </div>
      )}
    </Surface>
  );
}

export function SchedulerInspectPanel({
  title,
  query,
  mode,
}: {
  title: string;
  query: {
    isLoading: boolean;
    error: Error | null;
    data: Record<string, unknown> | undefined;
  };
  mode: "scheduler" | "queueConsumers" | "cronActivations";
}) {
  return (
    <StructuredSummaryPanel
      title={title}
      query={query}
      pick={data => {
        if (mode === "scheduler") {
          return {
            scalars: {
              paused: data.paused,
              inFlight: (data.global as Record<string, unknown> | undefined)?.inFlight,
              maxInFlight: (data.global as Record<string, unknown> | undefined)?.maxInFlight,
            },
            tables: [{
              label: "Pools",
              rows: Array.isArray(data.pools) ? data.pools as Record<string, unknown>[] : [],
            }],
          };
        }
        if (mode === "queueConsumers") {
          return {
            tables: [{
              label: "Queue consumers",
              rows: Array.isArray(data.queueConsumers) ? data.queueConsumers as Record<string, unknown>[] : [],
            }],
          };
        }
        return {
          tables: [{
            label: "Cron activations",
            rows: Array.isArray(data.cronActivations) ? data.cronActivations as Record<string, unknown>[] : [],
          }],
        };
      }}
    />
  );
}

export function ResourceCountCard({
  label,
  count,
  suffix,
  isLoading,
  error,
  to,
}: {
  label: string;
  count: number | null;
  suffix?: string;
  isLoading: boolean;
  error: boolean;
  to: string;
}) {
  return (
    <Surface className="p-4">
      <div className="text-sm text-kumo-subtle">{label}</div>
      {isLoading ? (
        <div className="mt-2 text-sm text-kumo-subtle">Loading…</div>
      ) : error ? (
        <div className="mt-2 text-sm text-kumo-danger">Unavailable</div>
      ) : (
        <Link to={to} className="mt-2 block text-2xl font-semibold text-kumo-link hover:underline">
          {count ?? 0}
          {suffix ?? ""}
        </Link>
      )}
    </Surface>
  );
}
