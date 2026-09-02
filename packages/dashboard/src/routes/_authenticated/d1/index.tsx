import { createFileRoute } from "@tanstack/react-router";
import { OfficialCatalog } from "../../../components/OfficialCatalog";

export const Route = createFileRoute("/_authenticated/d1/")({ component: D1Page });

function D1Page() {
  return <OfficialCatalog kind="D1 databases" description="Databases returned by the official D1 API." load={async (client, accountID, signal) => {
    const page = await client.cloudflare.d1.database.list({ account_id: accountID }, { signal });
    return page.result.map(database => ({
      id: database.uuid ?? "unknown",
      name: database.name ?? "Unnamed database",
      ...(database.created_at === undefined ? {} : { detail: database.created_at }),
      href: `/d1/${encodeURIComponent(database.uuid ?? "unknown")}`,
    }));
  }} create={(client, accountID, name) => client.cloudflare.d1.database.create({ account_id: accountID, name })}
  remove={(client, accountID, row) => client.cloudflare.d1.database.delete(row.id, { account_id: accountID })} />;
}
