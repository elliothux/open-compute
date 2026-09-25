import { Badge } from "@cloudflare/kumo/components/badge";
import { LayerCard } from "@cloudflare/kumo/components/layer-card";
import { Table } from "@cloudflare/kumo/components/table";
import { EmptyState } from "./dashboard-page";

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
