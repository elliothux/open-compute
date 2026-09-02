import { Link, useRouterState } from "@tanstack/react-router";

const LABELS: Record<string, string> = {
  "": "Overview",
  workers: "Workers",
  kv: "KV",
  d1: "D1",
  r2: "R2",
  "durable-objects": "Durable Objects",
  queues: "Queues",
  workflows: "Workflows",
  platform: "Platform",
};

function segmentLabel(segment: string): string {
  return LABELS[segment] ?? segment;
}

export function Breadcrumbs() {
  const pathname = useRouterState({ select: state => state.location.pathname });
  const segments = pathname.replace(/^\/+|\/+$/g, "").split("/").filter(Boolean);
  const crumbs = [{ label: "Overview", to: "/" }];
  let path = "";
  for (const segment of segments) {
    path += `/${segment}`;
    crumbs.push({ label: segmentLabel(segment), to: path });
  }

  return (
    <nav aria-label="Breadcrumb" className="mb-4 text-sm text-kumo-subtle">
      <ol className="flex flex-wrap items-center gap-2">
        {crumbs.map((crumb, index) => {
          const isLast = index === crumbs.length - 1;
          return (
            <li key={crumb.to} className="flex items-center gap-2">
              {index > 0 ? <span aria-hidden="true">/</span> : null}
              {isLast ? (
                <span className="font-medium text-kumo-default" aria-current="page">
                  {crumb.label}
                </span>
              ) : (
                <Link to={crumb.to} className="hover:text-kumo-default">
                  {crumb.label}
                </Link>
              )}
            </li>
          );
        })}
      </ol>
    </nav>
  );
}
