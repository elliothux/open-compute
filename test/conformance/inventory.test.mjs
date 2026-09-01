import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import {
  TYPE_ONLY_TARGET_SYMBOLS,
} from "./inventory-expand.ts";
import {
  assertEvidenceBinding,
  encodeInventory,
  generateInventoryTwice,
  parseMemberEvidence,
} from "./inventory.ts";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const COMMITTED = join(ROOT, "share/cloudflare-capabilities.json");
const CATALOG = join(ROOT, "test/conformance/catalog.json");

const PINNED = {
  workers_types_version: "5.20260830.1",
  ast_sha256: "da29f5ec1d9a81cc0094bd083ed3b28013573fcb2d4febd9fd62aecbfb53c6b3",
  named_declarations: 1165,
  target_declarations: 421,
  target_declarations_with_surface: 382,
  target_declarations_type_only: 39,
  inventoried_members: 2097,
  inventoried_symbols: 333,
};

const reportPromise = generateInventoryTwice();

function allMembers(inventory) {
  return Object.values(inventory.products).flatMap(product => product.members ?? []);
}

function membersOf(inventory, symbol) {
  return allMembers(inventory).filter(member => member.symbol === symbol);
}

function namesOf(inventory, symbol) {
  return new Set(membersOf(inventory, symbol).map(member => member.member));
}

test("memberEvidence parse and binding invariants use a synthetic fixture", () => {
  const fixture = [
    {
      id: "kv::KVNamespace::get:method#0",
      status: "supported",
      compileCases: ["kv.compile.get"],
      runtimeCases: ["kv.runtime.get"],
    },
    {
      id: "kv::KVNamespaceListResult::cursor:property#0",
      status: "blocked",
    },
  ];
  const evidence = parseMemberEvidence(fixture);
  assert.equal(evidence.size, 2);
  assert.deepEqual(evidence.get("kv::KVNamespace::get:method#0"), {
    id: "kv::KVNamespace::get:method#0",
    status: "supported",
    compile_cases: ["kv.compile.get"],
    runtime_cases: ["kv.runtime.get"],
    deviations: [],
  });
  assert.equal(evidence.get("kv::KVNamespaceListResult::cursor:property#0")?.status, "blocked");
  assert.doesNotThrow(() => assertEvidenceBinding([
    "kv::KVNamespace::get:method#0",
    "kv::KVNamespaceListResult::cursor:property#0",
    "d1::D1Result::results:property#0",
  ], evidence));
  assert.throws(
    () => parseMemberEvidence([{ id: "kv::KVNamespace::get:method#0", status: "blocked" }, { id: "kv::KVNamespace::get:method#0", status: "blocked" }]),
    /duplicate or empty memberEvidence id/,
  );
  assert.throws(
    () => parseMemberEvidence([{ id: "", status: "blocked" }]),
    /duplicate or empty memberEvidence id/,
  );
  assert.throws(
    () => parseMemberEvidence([{ id: "kv::KVNamespace::get:method#0", status: "maybe" }]),
    /invalid memberEvidence status/,
  );
  assert.throws(
    () => assertEvidenceBinding(["kv::KVNamespace::get:method#0"], evidence),
    /stale memberEvidence id: kv::KVNamespaceListResult::cursor:property#0/,
  );
});

test("generation is deterministic across two runs and matches the committed inventory", async () => {
  const { inventory, encoded, coverage } = await reportPromise;
  const second = encodeInventory(inventory);
  assert.equal(encoded, second);
  const committed = await readFile(COMMITTED, "utf8");
  assert.equal(encoded, committed);
  assert.equal(TYPE_ONLY_TARGET_SYMBOLS.size, 0);
  assert.equal(coverage.named_declarations, PINNED.named_declarations);
  assert.equal(coverage.target_declarations, PINNED.target_declarations);
  assert.equal(coverage.target_declarations_with_surface, PINNED.target_declarations_with_surface);
  assert.equal(coverage.target_declarations_type_only, PINNED.target_declarations_type_only);
  assert.equal(
    coverage.target_declarations_with_surface + coverage.target_declarations_type_only,
    coverage.target_declarations,
  );
  const members = allMembers(inventory);
  assert.equal(inventory.source.workers_types_version, PINNED.workers_types_version);
  assert.equal(inventory.source.ast_sha256, PINNED.ast_sha256);
  assert.equal(members.length, PINNED.inventoried_members);
  assert.equal(new Set(members.map(member => member.symbol)).size, PINNED.inventoried_symbols);
  assert.ok(members.every(member => member.id && member.symbol && member.member && member.kind && member.signature_sha256));
});

test("inventories KV union, R2 composite, D1 intersection, DurableObjectStub composition, and Workflow/Queue aliases", async () => {
  const { inventory } = await reportPromise;
  const kv = namesOf(inventory, "KVNamespaceListResult");
  for (const name of ["cacheStatus", "cursor", "list_complete", "keys"]) assert.ok(kv.has(name), name);
  assert.equal(membersOf(inventory, "KVNamespaceListResult").filter(member => member.member === "cursor").length, 1);
  assert.equal(membersOf(inventory, "KVNamespaceListResult").filter(member => member.member === "list_complete").length, 2);

  const r2 = namesOf(inventory, "R2Objects");
  for (const name of ["truncated", "cursor", "objects", "delimitedPrefixes"]) assert.ok(r2.has(name), name);
  assert.equal(membersOf(inventory, "R2Objects").filter(member => member.member === "cursor").length, 1);
  assert.equal(membersOf(inventory, "R2Objects").filter(member => member.member === "truncated").length, 2);

  const d1 = namesOf(inventory, "D1Result");
  for (const name of ["results", "success", "meta", "error"]) assert.ok(d1.has(name), name);

  const stub = namesOf(inventory, "DurableObjectStub");
  for (const name of ["id", "name", "fetch", "connect"]) assert.ok(stub.has(name), name);

  const delay = namesOf(inventory, "CloudflareWorkersModule.WorkflowDelayFunction");
  assert.ok(delay.has("()"));
  const workflowEvent = namesOf(inventory, "CloudflareWorkersModule.WorkflowEvent");
  for (const name of ["payload", "timestamp", "instanceId", "workflowName", "schedule"]) assert.ok(workflowEvent.has(name), name);
  const deleted = namesOf(inventory, "WorkflowBatchDeleteResult");
  for (const name of ["deleted", "errors"]) assert.ok(deleted.has(name), name);
  const instance = namesOf(inventory, "InstanceStatus");
  for (const name of ["status", "error", "output"]) assert.ok(instance.has(name), name);

  const queueHandler = namesOf(inventory, "ExportedHandlerQueueHandler");
  assert.ok(queueHandler.has("()"));
  const batch = namesOf(inventory, "MessageBatch");
  for (const name of ["messages", "queue", "metadata", "retryAll", "ackAll"]) assert.ok(batch.has(name), name);
});

test("pinned socket metadata distinguishes outbound peers from inbound connect authority", async () => {
  const sockets = await readFile(join(ROOT, "references/workerd/src/workerd/api/sockets.c++"), "utf8");
  assert.match(
    sockets,
    /setupSocket\(js,[\s\S]{0,320}kj::mv\(addressStr\),\s*kj::none \/\* localAddress \*\//,
  );
  const globalScope = await readFile(join(ROOT, "references/workerd/src/workerd/api/global-scope.c++"), "utf8");
  assert.match(
    globalScope,
    /setupSocket\(js,[\s\S]{0,320}kj::none \/\* remoteAddress \*\/,[\s\n]*kj::mv\(host\)/,
  );
  const serviceTransport = await readFile(
    join(ROOT, "packages/runtime/src/services/transport.ts"), "utf8",
  );
  assert.match(serviceTransport, /inboundSocketTargetAddress\(socket\)/);
  for (const relative of [
    "packages/runtime/src/durable-objects/host.ts",
    "packages/runtime/src/durable-objects/router.ts",
  ]) {
    const source = await readFile(join(ROOT, relative), "utf8");
    assert.match(source, /inboundSocketAddress\(socket\)/, relative);
  }
});

test("target member evidence is complete and raw TCP coverage is exact", async () => {
  const { inventory } = await reportPromise;
  const catalog = JSON.parse(await readFile(CATALOG, "utf8"));
  const members = allMembers(inventory);
  const byId = new Map(members.map(member => [member.id, member]));
  const owned = new Set();
  for (const gap of catalog.blockedGaps) {
    assert.ok(Array.isArray(gap.memberIds) && gap.memberIds.length > 0, gap.id);
    for (const id of gap.memberIds) {
      assert.ok(byId.has(id), `${gap.id}: ${id}`);
      assert.equal(byId.get(id).status, "blocked", id);
      assert.equal(owned.has(id), false, `${id} has duplicate gap owners`);
      owned.add(id);
    }
  }
  const blocked = members.filter(member => member.status === "blocked").map(member => member.id).sort();
  assert.deepEqual([...owned].sort(), blocked);
  assert.deepEqual(blocked, []);
  assert.equal(inventory.products.images.kind, "platform");
  assert.equal(inventory.products.images.status, "supported_with_deviation");
  assert.equal(inventory.products.static_assets.kind, "platform");
  assert.equal(
    catalog.blockedGaps.some(gap => gap.id === "workers.raw-tcp-security-boundary"),
    false,
  );
  const raw = catalog.memberEvidence.filter(item =>
    item.compileCases?.includes("ts::test/conformance/fixtures/raw-tcp-surface.ts"));
  assert.equal(raw.length, 27);
  assert.ok(raw.every(item => item.status === "supported_with_deviation"));
  const rawIds = new Set(raw.map(item => item.id));
  for (const id of [
    "workers::ExportedHandlerConnectHandler::():call#0",
    "workers::TlsOptions::expectedServerHostname:property#0",
    "durable_objects::DurableObject::connect:method#0",
    "durable_objects::CloudflareWorkersModule.DurableObject::connect:method#0",
  ]) assert.ok(rawIds.has(id), id);

  const socketInfo = new Map(membersOf(inventory, "SocketInfo").map(member => [member.member, member]));
  assert.equal(socketInfo.get("localAddress")?.signature, "localAddress?: string;");
  assert.equal(socketInfo.get("remoteAddress")?.signature, "remoteAddress?: string;");
  assert.equal(socketInfo.get("localAddress")?.status, "supported_with_deviation");
  assert.equal(socketInfo.get("remoteAddress")?.status, "supported_with_deviation");
});
