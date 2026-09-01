import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

type JsonRecord = Record<string, unknown>;

interface Member {
  readonly id: string;
  readonly product: string;
  readonly symbol: string;
  readonly member: string;
  readonly status: "supported" | "supported_with_deviation" | "blocked";
}

interface Product {
  readonly status: "supported" | "supported_with_deviation" | "unsupported" | "blocked";
  readonly members: readonly Member[];
}

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const CATALOG = resolve(ROOT, "test/conformance/catalog.json");
const INVENTORY = resolve(ROOT, "share/cloudflare-capabilities.json");

const RAW_CONNECT_SYMBOLS = new Set([
  "CloudflareWorkersModule.DurableObject",
  "CloudflareWorkersModule.WorkerEntrypoint",
  "DurableObject",
  "DurableObjectStub",
  "ExportedHandler",
  "ExportedHandlerConnectHandler",
  "Fetcher",
  "LoopbackForExport",
  "LoopbackServiceStub",
  "Service",
]);

const DO_RPC_BLOCKED_IDS = new Set([
  "durable_objects::DurableObjectStub::():call#0",
  "workers::Rpc.Serializable::():call#0",
  "workers::Rpc.Serializable::():call#1",
]);

function record(value: unknown, label: string): JsonRecord {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  return value as JsonRecord;
}

function rawTcp(member: Member): boolean {
  return member.symbol === "cloudflare:sockets"
    || member.symbol === "Socket"
    || member.symbol === "SocketAddress"
    || member.symbol === "SocketInfo"
    || member.symbol === "SocketOptions"
    || (member.symbol === "TlsOptions" && member.member === "expectedServerHostname")
    || (member.member === "connect" && RAW_CONNECT_SYMBOLS.has(member.symbol))
    || (member.member === "()" && member.symbol === "ExportedHandlerConnectHandler");
}

function contractStatus(products: Record<string, Product>, contract: JsonRecord): Product["status"] {
  const names = [String(contract.product ?? "")];
  if (Array.isArray(contract.additionalProducts)) {
    for (const name of contract.additionalProducts) names.push(String(name));
  }
  const statuses = names.map(name => products[name]?.status ?? "blocked");
  if (statuses.includes("blocked")) return "blocked";
  if (statuses.includes("unsupported")) return "unsupported";
  if (statuses.includes("supported_with_deviation")) return "supported_with_deviation";
  return "supported";
}

function main(args: string[]): void {
  if (args.length !== 1 || args[0] !== "--apply") throw new Error("use --apply");
  const inventory = record(JSON.parse(readFileSync(INVENTORY, "utf8")), "inventory");
  const products = record(inventory.products, "inventory.products") as Record<string, Product>;
  const catalog = record(JSON.parse(readFileSync(CATALOG, "utf8")), "catalog");
  const members = Object.values(products).flatMap(product => product.members ?? []);
  const byId = new Map(members.map(member => [member.id, member]));

  // Evidence is review-owned. This updater never infers runtime support from a
  // product Gate or from the mere presence of an upstream declaration.
  if (!Array.isArray(catalog.memberEvidence)) throw new Error("catalog.memberEvidence must be an array");
  const evidence = catalog.memberEvidence.map((raw, index) => {
    const item = record(raw, `memberEvidence[${index}]`);
    const id = String(item.id ?? "");
    if (!byId.has(id)) throw new Error(`stale memberEvidence id: ${id}`);
    if (item.status !== "supported" && item.status !== "supported_with_deviation") {
      throw new Error(`${id}: blocked members belong only in blockedGaps`);
    }
    return item;
  }).sort((left, right) => String(left.id).localeCompare(String(right.id)));
  catalog.memberEvidence = evidence;

  const groups = new Map<string, string[]>();
  const add = (gap: string, id: string): void => {
    const ids = groups.get(gap) ?? [];
    ids.push(id);
    groups.set(gap, ids);
  };
  for (const member of members) {
    if (member.status !== "blocked") continue;
    if (rawTcp(member)) add("workers.raw-tcp-security-boundary", member.id);
    else if (DO_RPC_BLOCKED_IDS.has(member.id)) add("durable-objects.rpc-observable-boundary", member.id);
    else add(`${member.product}.unverified-stable-members`, member.id);
  }
  catalog.blockedGaps = [...groups]
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([id, memberIds]) => ({ id, memberIds: memberIds.sort() }));

  if (!Array.isArray(catalog.contracts)) throw new Error("catalog.contracts must be an array");
  for (const raw of catalog.contracts) {
    const contract = record(raw, "contract");
    contract.status = contractStatus(products, contract);
  }
  writeFileSync(CATALOG, `${JSON.stringify(catalog, null, 2)}\n`);
}

main(process.argv.slice(2));
