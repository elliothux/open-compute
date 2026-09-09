import { Badge } from "@cloudflare/kumo/components/badge";
import { Empty } from "@cloudflare/kumo/components/empty";
import { LayerCard } from "@cloudflare/kumo/components/layer-card";
import { Surface } from "@cloudflare/kumo/components/surface";
import { Table } from "@cloudflare/kumo/components/table";
import { BookOpen, Tray } from "@phosphor-icons/react";
import { CopyableId } from "./copyable-id";

interface PageHeaderProps {
  title: string;
  description?: string;
  actions?: React.ReactNode;
  docsUrl?: string;
  resourceId?: string;
  resourceLabel?: string;
}

export function PageHeader({
  title,
  description,
  actions,
  docsUrl,
  resourceId,
  resourceLabel,
}: PageHeaderProps) {
  return (
    <div className="mb-6 flex flex-wrap items-start justify-between gap-4">
      <div>
        <h1 className="text-2xl font-semibold">{title}</h1>
        {description ? (
          <p className="text-kumo-subtle mt-1 max-w-3xl text-sm">
            {description}
          </p>
        ) : null}
        {resourceId ? (
          <div className="text-kumo-subtle mt-2 flex min-w-0 items-center gap-2 text-sm">
            <span>{resourceLabel ?? "Resource ID"}</span>
            <CopyableId
              value={resourceId}
              label={(resourceLabel ?? "resource ID").toLowerCase()}
            />
          </div>
        ) : null}
        {docsUrl ? (
          <a
            className="text-kumo-link mt-2 inline-flex items-center gap-1.5 text-sm hover:underline"
            href={docsUrl}
            target="_blank"
            rel="noreferrer"
          >
            <BookOpen size={16} />
            View documentation
          </a>
        ) : null}
      </div>
      {actions ? (
        <div className="flex items-center gap-2">{actions}</div>
      ) : null}
    </div>
  );
}

export function SectionHeader({
  title,
  description,
}: {
  title: string;
  description?: string;
}) {
  return (
    <div className="mb-4">
      <h2 className="text-lg font-semibold">{title}</h2>
      {description ? (
        <p className="text-kumo-subtle mt-1 text-sm">{description}</p>
      ) : null}
    </div>
  );
}

function EmptyState({
  title,
  description,
  action,
}: {
  title: string;
  description: string;
  action?: React.ReactNode;
}) {
  return (
    <LayerCard className="p-0">
      <Empty
        icon={<Tray size={36} />}
        title={title}
        description={description}
        contents={action}
        size="sm"
      />
    </LayerCard>
  );
}

export function LoadingState({ label = "Loading…" }: { label?: string }) {
  return (
    <Surface className="text-kumo-subtle p-8 text-center text-sm">
      {label}
    </Surface>
  );
}

export function ErrorState({ message }: { message: string }) {
  return (
    <Surface className="ring-kumo-danger/30 text-kumo-danger p-6 text-sm ring">
      {message}
    </Surface>
  );
}

interface DataTableProps {
  columns: Array<{ key: string; label: string; className?: string }>;
  rows: Array<Record<string, React.ReactNode>>;
  emptyLabel?: string;
  emptyAction?: React.ReactNode;
}

export function DataTable({
  columns,
  rows,
  emptyLabel = "No records found.",
  emptyAction,
}: DataTableProps) {
  if (rows.length === 0) {
    return (
      <EmptyState
        title="Nothing here yet"
        description={emptyLabel}
        action={emptyAction}
      />
    );
  }
  return (
    <LayerCard className="overflow-hidden p-0">
      <div className="overflow-x-auto">
        <Table className="min-w-full">
          <Table.Header variant="compact">
            <Table.Row>
              {columns.map((column) => (
                <Table.Head key={column.key} className={column.className}>
                  {column.label}
                </Table.Head>
              ))}
            </Table.Row>
          </Table.Header>
          <Table.Body>
            {rows.map((row, index) => (
              <Table.Row key={index}>
                {columns.map((column) => (
                  <Table.Cell key={column.key} className={column.className}>
                    {row[column.key]}
                  </Table.Cell>
                ))}
              </Table.Row>
            ))}
          </Table.Body>
        </Table>
      </div>
    </LayerCard>
  );
}

export function StatusBadge({ value }: { value: string }) {
  const normalized = value.toLowerCase();
  const tone =
    normalized.includes("ready") ||
    normalized.includes("active") ||
    normalized.includes("running") ||
    normalized === "live"
      ? "success"
      : normalized.includes("degrad") ||
          normalized.includes("pending") ||
          normalized.includes("paused")
        ? "warning"
        : normalized.includes("fail") ||
            normalized.includes("error") ||
            normalized.includes("stopped") ||
            normalized.includes("unavailable") ||
            normalized.includes("corrupt") ||
            normalized.includes("hard_limit") ||
            normalized.includes("denied")
          ? "error"
          : "neutral";
  return (
    <Badge variant={tone} appearance="dot">
      {value}
    </Badge>
  );
}
