import { createFileRoute } from "@tanstack/react-router";
import { OfficialCatalog } from "../../../components/OfficialCatalog";

export const Route = createFileRoute("/_authenticated/r2/")({ component: R2Page });

function R2Page() {
  return <OfficialCatalog kind="R2 buckets" description="Buckets returned by the official R2 API." load={async (client, accountID, signal) => {
    const result = await client.cloudflare.r2.buckets.list({ account_id: accountID }, { signal });
    return (result.buckets ?? []).map(bucket => ({
      id: bucket.name ?? "unknown",
      name: bucket.name ?? "Unnamed bucket",
      ...(bucket.creation_date === undefined ? {} : { detail: bucket.creation_date }),
      href: `/r2/${encodeURIComponent(bucket.name ?? "unknown")}`,
    }));
  }} create={(client, accountID, name) => client.cloudflare.r2.buckets.create({ account_id: accountID, name })}
  remove={(client, accountID, row) => client.cloudflare.r2.buckets.delete(row.id, { account_id: accountID })} />;
}
