import { createFileRoute } from "@tanstack/react-router";
import { OfficialCatalog } from "../../../components/OfficialCatalog";

export const Route = createFileRoute("/_authenticated/kv/")({ component: KvPage });

function KvPage() {
  return <OfficialCatalog kind="KV namespaces" description="Namespaces returned by the official Workers KV API." load={async (client, accountID, signal) => {
    const page = await client.cloudflare.kv.namespaces.list({ account_id: accountID }, { signal });
    return page.result.map(namespace => ({ id: namespace.id, name: namespace.title, href: `/kv/${encodeURIComponent(namespace.id)}` }));
  }} create={(client, accountID, name) => client.cloudflare.kv.namespaces.create({ account_id: accountID, title: name })}
  rename={(client, accountID, row, name) => client.cloudflare.kv.namespaces.update(row.id, { account_id: accountID, title: name })}
  remove={(client, accountID, row) => client.cloudflare.kv.namespaces.delete(row.id, { account_id: accountID })} />;
}
