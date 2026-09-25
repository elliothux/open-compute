import { Button } from "@cloudflare/kumo/components/button";
import { IconActivity } from "@tabler/icons-react";
import { useQuery } from "@tanstack/react-query";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { CloudflareProductIcon } from "../../../components/cloudflare-product-icons";
import {
  CatalogToolbar,
  ErrorState,
  LoadingRows,
  PageHeader,
  Panel,
  Section,
  StatGrid,
} from "../../../components/dashboard-page";
import { useAuth } from "../../../features/auth/auth-atoms";
import { normalizeRfc3339 } from "../../../lib/date-time";

export const Route = createFileRoute("/_authenticated/observability/")({
  component: ObservabilityPage,
});

function sourceText(value: unknown) {
  if (typeof value === "string") return value;
  return JSON.stringify(value) ?? "null";
}

function ObservabilityPage() {
  const { client, instanceId: selectedInstanceId } = useAuth();
  const enabled = client !== null && selectedInstanceId !== null;
  const [hours, setHours] = useState(1);
  const [search, setSearch] = useState("");
  const [rangeEnd, setRangeEnd] = useState(() => Date.now());
  const from = rangeEnd - hours * 60 * 60 * 1_000;

  const usage = useQuery({
    queryKey: [
      "cloudflare-v4",
      "observability",
      "usage",
      selectedInstanceId,
      hours,
      rangeEnd,
    ],
    queryFn: ({ signal }) =>
      client!.openCompute.workers.observability.usage(selectedInstanceId!, {
        signal,
        query: { from, to: rangeEnd },
      }),
    enabled,
    refetchInterval: 30_000,
  });
  const logs = useQuery({
    queryKey: [
      "cloudflare-v4",
      "observability",
      "events",
      selectedInstanceId,
      hours,
      rangeEnd,
    ],
    queryFn: ({ signal }) =>
      client!.workers.observability.telemetry.query(
        {
          account_id: selectedInstanceId!,
          queryId: "dashboard-account-observability",
          timeframe: { from, to: rangeEnd },
          view: "events",
          limit: 100,
          parameters: { datasets: ["cloudflare-workers"], filters: [] },
        },
        { signal },
      ),
    enabled,
    refetchInterval: 10_000,
  });
  const events = useMemo(() => {
    const query = search.trim().toLowerCase();
    return (logs.data?.events?.events ?? []).filter((event) => {
      const service = event.$workers?.scriptName ?? "";
      return `${service} ${event.$metadata.level ?? event.$metadata.type} ${sourceText(event.source)}`
        .toLowerCase()
        .includes(query);
    });
  }, [logs.data, search]);
  const services = new Set(
    (usage.data?.breakdown ?? []).map((item) => item.service),
  ).size;
  const peak = Math.max(
    1,
    ...(usage.data?.breakdown ?? []).map((item) => item.count),
  );

  return (
    <>
      <PageHeader
        title="Observability"
        description="Search persisted Workers Logs across this account."
      />
      <CatalogToolbar
        value={search}
        onChange={setSearch}
        onRefresh={() => {
          void usage.refetch();
          void logs.refetch();
          setRangeEnd(Date.now());
        }}
        refreshing={usage.isFetching || logs.isFetching}
        placeholder="Search service, level, or message"
      />
      <div className="grid gap-6">
        <div className="flex flex-wrap gap-2" aria-label="Time range">
          {[1, 6, 24].map((value) => (
            <Button
              key={value}
              variant={hours === value ? "primary" : "secondary"}
              onClick={() => {
                setHours(value);
                setRangeEnd(Date.now());
              }}
            >
              Last {value}h
            </Button>
          ))}
        </div>
        {usage.isLoading ? (
          <LoadingRows count={3} />
        ) : usage.error ? (
          <ErrorState error={usage.error} />
        ) : (
          <>
            <StatGrid
              items={[
                {
                  label: "Events",
                  value: usage.data?.events.toLocaleString() ?? "0",
                },
                { label: "Services", value: services },
                {
                  label: "Errors shown",
                  value: events.filter(
                    (event) => event.$metadata.level === "error",
                  ).length,
                },
                { label: "Time range", value: `${hours}h` },
              ]}
            />
            <Section
              title="Events"
              description="Persisted event volume by service and UTC day."
            >
              <Panel className="grid gap-4">
                <div
                  className="flex h-40 items-end gap-2 overflow-x-auto"
                  aria-label="Event volume"
                >
                  {(usage.data?.breakdown ?? []).length ? (
                    usage.data!.breakdown.map((item, index) => (
                      <Link
                        key={`${item.bin}-${item.service}-${index}`}
                        to="/workers/$workerId"
                        params={{ workerId: item.service }}
                        className="group flex h-full min-w-12 flex-1 flex-col justify-end gap-2"
                        title={`${item.service}: ${item.count} events`}
                      >
                        <span
                          className="bg-kumo-brand group-hover:bg-kumo-brand-hover min-h-1 rounded-t"
                          style={{
                            height: `${Math.max(4, (item.count / peak) * 100)}%`,
                          }}
                        />
                        <span className="text-kumo-subtle truncate text-center text-xs">
                          {item.service}
                        </span>
                      </Link>
                    ))
                  ) : (
                    <div className="text-kumo-subtle m-auto">
                      No events in this time range.
                    </div>
                  )}
                </div>
              </Panel>
            </Section>
          </>
        )}
        <Section
          title="Workers Logs"
          description="The latest 100 persisted events."
        >
          {logs.isLoading ? (
            <LoadingRows count={5} />
          ) : logs.error ? (
            <ErrorState error={logs.error} />
          ) : (
            <Panel className="overflow-hidden p-0">
              {events.length ? (
                <div className="divide-kumo-line divide-y overflow-x-auto">
                  {events.map((event, index) => {
                    const service =
                      event.$workers?.scriptName ?? "Unknown Worker";
                    return (
                      <div
                        key={
                          event.$metadata.id ?? `${event.timestamp}-${index}`
                        }
                        className="grid min-w-2xl grid-cols-4 gap-3 px-4 py-3"
                      >
                        <span className="text-kumo-subtle">
                          {normalizeRfc3339(event.timestamp) ?? "—"}
                        </span>
                        <span className="text-kumo-link">
                          {event.$metadata.level ?? event.$metadata.type}
                        </span>
                        <Link
                          className="flex min-w-0 items-center gap-2 hover:underline"
                          to="/workers/$workerId"
                          params={{ workerId: service }}
                        >
                          <CloudflareProductIcon
                            product="Workers"
                            size={16}
                            className="text-kumo-brand shrink-0"
                          />
                          <span className="truncate">{service}</span>
                        </Link>
                        <span className="break-all">
                          {sourceText(event.source)}
                        </span>
                      </div>
                    );
                  })}
                </div>
              ) : (
                <div className="text-kumo-subtle flex min-h-40 flex-col items-center justify-center gap-2 px-4 py-8 text-center">
                  <IconActivity size={28} />
                  <span>
                    {search
                      ? "No events match this search."
                      : "No events in this time range."}
                  </span>
                </div>
              )}
            </Panel>
          )}
        </Section>
      </div>
    </>
  );
}
