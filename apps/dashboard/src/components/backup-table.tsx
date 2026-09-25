import { Button } from "@cloudflare/kumo/components/button";
import { LayerCard } from "@cloudflare/kumo/components/layer-card";
import { Table } from "@cloudflare/kumo/components/table";
import { formatBytes } from "../lib/format";

type BackupRow = {
  id: string;
  state: string;
  size?: number;
  created_on: string;
};

export function BackupTable({
  backups,
  onRestore,
}: {
  backups: readonly BackupRow[];
  onRestore: (backupId: string) => void;
}) {
  return (
    <LayerCard className="overflow-hidden p-0">
      <div className="overflow-x-auto">
        <Table className="min-w-2xl">
          <Table.Header variant="compact">
            <Table.Row>
              <Table.Head>Backup</Table.Head>
              <Table.Head>State</Table.Head>
              <Table.Head>Size</Table.Head>
              <Table.Head>Created</Table.Head>
              <Table.Head className="w-24">
                <span className="sr-only">Actions</span>
              </Table.Head>
            </Table.Row>
          </Table.Header>
          <Table.Body>
            {backups.map((backup) => (
              <Table.Row key={backup.id}>
                <Table.Cell className="font-mono text-xs">
                  {backup.id}
                </Table.Cell>
                <Table.Cell>{backup.state}</Table.Cell>
                <Table.Cell>{formatBytes(backup.size)}</Table.Cell>
                <Table.Cell>
                  {new Date(backup.created_on).toLocaleString()}
                </Table.Cell>
                <Table.Cell>
                  <Button
                    variant="secondary"
                    onClick={() => onRestore(backup.id)}
                  >
                    Restore
                  </Button>
                </Table.Cell>
              </Table.Row>
            ))}
          </Table.Body>
        </Table>
      </div>
    </LayerCard>
  );
}
