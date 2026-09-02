import { createFileRoute } from "@tanstack/react-router";
import { OfficialCatalog } from "../../../components/OfficialCatalog";

export const Route = createFileRoute("/_authenticated/durable-objects/")({ component: DurableObjectsPage });

function DurableObjectsPage() {
  return <OfficialCatalog kind="Durable Objects" description="Read-only namespace inventory from the open-compute extension." load={async (client, accountID, signal) => {
    const namespaces = await client.openCompute.durableObjects.list(accountID, { signal });
    return namespaces.map(namespace => ({ id: namespace.id, name: namespace.class_name, detail: namespace.script_name, href: `/durable-objects/${encodeURIComponent(namespace.id)}` }));
  }} />;
}
