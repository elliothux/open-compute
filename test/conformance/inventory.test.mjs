import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { sourceIdentityExcluded } from "./checks/context.ts";
import { TYPE_ONLY_TARGET_SYMBOLS } from "./inventory-expand.ts";
import {
  assertEvidenceBinding,
  encodeInventory,
  generateInventoryTwice,
  parseMemberEvidence,
} from "./inventory.ts";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const COMMITTED = join(ROOT, "share/cloudflare-capabilities.json");
const CATALOG = join(ROOT, "test/conformance/catalog.json");

test("baseline digest excludes rules and prose but retains embedded runbooks and code", () => {
  for (const path of [
    "AGENTS.md",
    "apps/dashboard/AGENTS.md",
    "CLAUDE.md",
    ".agents/skills/example/SKILL.md",
    ".claude/settings.json",
    "docs/references/testing.md",
    "docs/references/p1-deviations.md",
    "docs/plan.md",
    "test/__pycache__/gate.pyc",
    "test/conformance/baseline.json",
  ]) {
    assert.equal(sourceIdentityExcluded(path), true, path);
  }
  for (const path of [
    "docs/references/runbooks/install-and-first-start.md",
    "crates/service/src/resources.rs",
    "test/conformance/checks/context.ts",
    "packages/runtime/workerd.lock.json",
    "Cargo.toml",
    ".github/workflows/ci.yml",
  ]) {
    assert.equal(sourceIdentityExcluded(path), false, path);
  }
});

test("baseline digest hashes tracked working-tree inputs and ignores untracked local files", async (t) => {
  const root = mkdtempSync(join(tmpdir(), "oc-source-identity-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const context = join(root, "test/conformance/checks/context.ts");
  mkdirSync(dirname(context), { recursive: true });
  copyFileSync(join(ROOT, "test/conformance/checks/context.ts"), context);
  const excluded = [
    "AGENTS.md",
    "nested/AGENTS.md",
    ".agents/rules.md",
    ".claude/settings.json",
    "docs/references/testing.md",
    "docs/references/p1-deviations.md",
  ];
  const included = ["source.ts", "docs/references/runbooks/fixture.md"];
  for (const path of [...excluded, ...included]) {
    mkdirSync(dirname(join(root, path)), { recursive: true });
    writeFileSync(join(root, path), "original");
  }
  execFileSync("git", ["init", "-q"], { cwd: root });
  execFileSync("git", ["add", "-f", "."], { cwd: root });
  const { sourceIdentity } = await import(pathToFileURL(context).href);
  const original = sourceIdentity();
  for (const path of excluded) writeFileSync(join(root, path), "changed");
  for (const path of [
    "REVIEW.md",
    "local.ts",
    ".agents/local.md",
    ".claude/local.json",
  ])
    writeFileSync(join(root, path), "untracked");
  assert.equal(sourceIdentity(), original);
  for (const path of included) {
    writeFileSync(join(root, path), "changed");
    assert.notEqual(sourceIdentity(), original, path);
    writeFileSync(join(root, path), "original");
  }
  assert.equal(sourceIdentity(), original);
});

function hasCommit(repository, revision) {
  try {
    execFileSync("git", ["cat-file", "-e", `${revision}^{commit}`], {
      cwd: repository,
      stdio: "ignore",
      timeout: 10_000,
    });
    return true;
  } catch {
    return false;
  }
}

const PINNED = {
  workers_types_version: "5.20260930.2",
  ast_sha256:
    "c49a9c138ef23712f324432807840b7eec7cbcd696487ad9c42217015482be5b",
  named_declarations: 1217,
  target_declarations: 547,
  target_declarations_with_surface: 492,
  target_declarations_type_only: 55,
  inventoried_members: 2846,
  inventoried_symbols: 440,
};

const NEW_PIN_BLOCKED_MEMBERS = [
  "artifacts::ArtifactsCommitMetadata::author:property#0",
  "artifacts::ArtifactsCommitMetadata::authoredAt:property#0",
  "artifacts::ArtifactsCommitMetadata::committedAt:property#0",
  "artifacts::ArtifactsCommitMetadata::committer:property#0",
  "artifacts::ArtifactsCommitMetadata::hash:property#0",
  "artifacts::ArtifactsCommitMetadata::message:property#0",
  "artifacts::ArtifactsCommitMetadata::parents:property#0",
  "artifacts::ArtifactsCommitMetadata::treeHash:property#0",
  "artifacts::ArtifactsRepo::info:method#0",
  "artifacts::ArtifactsRepo::log:method#0",
  "artifacts::ArtifactsRepo::readBlob:method#0",
  "artifacts::ArtifactsRepo::readCommit:method#0",
  "artifacts::ArtifactsRepo::readFile:method#0",
  "artifacts::ArtifactsRepo::readTree:method#0",
  "artifacts::ArtifactsTreeEntry::hash:property#0",
  "artifacts::ArtifactsTreeEntry::mode:property#0",
  "artifacts::ArtifactsTreeEntry::name:property#0",
  "artifacts::ArtifactsTreeEntry::type:property#0",
  "workers::AnalyticsSQLBinding::query:method#0",
  "workers::AnalyticsSQLQuery::params:property#0",
  "workers::AnalyticsSQLQuery::query:property#0",
  "workers::AnalyticsSQLResult::data:property#0",
  "workers::AnalyticsSQLResult::rows:property#0",
  "workers::AnalyticsSQLResult::statistics:property#0",
  "workers::AnalyticsSQLStatistics::bytes_read:property#0",
  "workers::AnalyticsSQLStatistics::elapsed_ms:property#0",
  "workers::AnalyticsSQLStatistics::rows_read:property#0",
  "workers::CfProperties::originRangeRequests:property#0",
  "workers::JsonWebKey::priv:property#0",
  "workers::JsonWebKey::pub:property#0",
  "workers::JsonWebKeyWithKid::priv:property#0",
  "workers::JsonWebKeyWithKid::pub:property#0",
  "workers::LoopbackForExport::create:method#0",
  "workers::LoopbackForExport::createBatch:method#0",
  "workers::LoopbackForExport::deleteBatch:method#0",
  "workers::LoopbackForExport::get:method#0",
  "workers::MessageEventInit::source:property#0",
  "workers::RequestInitCfProperties::originRangeRequests:property#0",
  "workers::Span::recordException:method#0",
  "workers::Span::setStatus:method#0",
  "workers::Span::updateName:method#0",
  "workers::SubtleCryptoSignAlgorithm::context:property#0",
  "workers::Tracing::getActiveSpan:method#0",
  "workers::TracingSpanStatus::code:property#0",
  "workers::TracingSpanStatus::message:property#0",
  "workers::cloudflare:durable-objects::retryable:function#0",
  "workers::cloudflare:node.NodeStyleServer::address:method#0",
  "workers::cloudflare:node::connectHandler:function#0",
  "workers::cloudflare:node::handleAsNodeConnection:function#0",
  "workers::cloudflare:node::handleAsNodeRequest:function#0",
  "workflows::InstanceStatus::status:property#0",
  "workflows::WorkflowInstance::subscribe:method#0",
  "workflows::WorkflowInstanceEvent::attempt:property#0",
  "workflows::WorkflowInstanceEvent::attempt:property#1",
  "workflows::WorkflowInstanceEvent::attempt:property#2",
  "workflows::WorkflowInstanceEvent::attempt:property#3",
  "workflows::WorkflowInstanceEvent::attempt:property#4",
  "workflows::WorkflowInstanceEvent::attempt:property#5",
  "workflows::WorkflowInstanceEvent::config:property#0",
  "workflows::WorkflowInstanceEvent::config:property#1",
  "workflows::WorkflowInstanceEvent::durationMs:property#0",
  "workflows::WorkflowInstanceEvent::error:property#0",
  "workflows::WorkflowInstanceEvent::error:property#1",
  "workflows::WorkflowInstanceEvent::error:property#2",
  "workflows::WorkflowInstanceEvent::error:property#3",
  "workflows::WorkflowInstanceEvent::eventId:property#0",
  "workflows::WorkflowInstanceEvent::eventType:property#0",
  "workflows::WorkflowInstanceEvent::instanceId:property#0",
  "workflows::WorkflowInstanceEvent::output:property#0",
  "workflows::WorkflowInstanceEvent::output:property#1",
  "workflows::WorkflowInstanceEvent::params:property#0",
  "workflows::WorkflowInstanceEvent::retryDelayMs:property#0",
  "workflows::WorkflowInstanceEvent::retryDelayMs:property#1",
  "workflows::WorkflowInstanceEvent::stepName:property#0",
  "workflows::WorkflowInstanceEvent::stepName:property#1",
  "workflows::WorkflowInstanceEvent::stepName:property#10",
  "workflows::WorkflowInstanceEvent::stepName:property#11",
  "workflows::WorkflowInstanceEvent::stepName:property#12",
  "workflows::WorkflowInstanceEvent::stepName:property#13",
  "workflows::WorkflowInstanceEvent::stepName:property#14",
  "workflows::WorkflowInstanceEvent::stepName:property#15",
  "workflows::WorkflowInstanceEvent::stepName:property#16",
  "workflows::WorkflowInstanceEvent::stepName:property#2",
  "workflows::WorkflowInstanceEvent::stepName:property#3",
  "workflows::WorkflowInstanceEvent::stepName:property#4",
  "workflows::WorkflowInstanceEvent::stepName:property#5",
  "workflows::WorkflowInstanceEvent::stepName:property#6",
  "workflows::WorkflowInstanceEvent::stepName:property#7",
  "workflows::WorkflowInstanceEvent::stepName:property#8",
  "workflows::WorkflowInstanceEvent::stepName:property#9",
  "workflows::WorkflowInstanceEvent::timestamp:property#0",
  "workflows::WorkflowInstanceEvent::type:property#0",
  "workflows::WorkflowInstanceEvent::type:property#1",
  "workflows::WorkflowInstanceEvent::type:property#10",
  "workflows::WorkflowInstanceEvent::type:property#11",
  "workflows::WorkflowInstanceEvent::type:property#12",
  "workflows::WorkflowInstanceEvent::type:property#13",
  "workflows::WorkflowInstanceEvent::type:property#14",
  "workflows::WorkflowInstanceEvent::type:property#15",
  "workflows::WorkflowInstanceEvent::type:property#16",
  "workflows::WorkflowInstanceEvent::type:property#17",
  "workflows::WorkflowInstanceEvent::type:property#18",
  "workflows::WorkflowInstanceEvent::type:property#19",
  "workflows::WorkflowInstanceEvent::type:property#2",
  "workflows::WorkflowInstanceEvent::type:property#20",
  "workflows::WorkflowInstanceEvent::type:property#21",
  "workflows::WorkflowInstanceEvent::type:property#22",
  "workflows::WorkflowInstanceEvent::type:property#23",
  "workflows::WorkflowInstanceEvent::type:property#24",
  "workflows::WorkflowInstanceEvent::type:property#25",
  "workflows::WorkflowInstanceEvent::type:property#26",
  "workflows::WorkflowInstanceEvent::type:property#27",
  "workflows::WorkflowInstanceEvent::type:property#28",
  "workflows::WorkflowInstanceEvent::type:property#3",
  "workflows::WorkflowInstanceEvent::type:property#4",
  "workflows::WorkflowInstanceEvent::type:property#5",
  "workflows::WorkflowInstanceEvent::type:property#6",
  "workflows::WorkflowInstanceEvent::type:property#7",
  "workflows::WorkflowInstanceEvent::type:property#8",
  "workflows::WorkflowInstanceEvent::type:property#9",
  "workflows::WorkflowInstanceSubscribeOptions::cursor:property#0",
  "workflows::WorkflowInstanceSubscribeOptions::filter:property#0",
  "workflows::WorkflowInstanceSubscription::next:method#0",
];

const reportPromise = generateInventoryTwice();

function allMembers(inventory) {
  return Object.values(inventory.products).flatMap(
    (product) => product.members ?? [],
  );
}

function membersOf(inventory, symbol) {
  return allMembers(inventory).filter((member) => member.symbol === symbol);
}

function namesOf(inventory, symbol) {
  return new Set(membersOf(inventory, symbol).map((member) => member.member));
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
  assert.equal(
    evidence.get("kv::KVNamespaceListResult::cursor:property#0")?.status,
    "blocked",
  );
  assert.doesNotThrow(() =>
    assertEvidenceBinding(
      [
        "kv::KVNamespace::get:method#0",
        "kv::KVNamespaceListResult::cursor:property#0",
        "d1::D1Result::results:property#0",
      ],
      evidence,
    ),
  );
  assert.throws(
    () =>
      parseMemberEvidence([
        { id: "kv::KVNamespace::get:method#0", status: "blocked" },
        { id: "kv::KVNamespace::get:method#0", status: "blocked" },
      ]),
    /duplicate or empty memberEvidence id/,
  );
  assert.throws(
    () => parseMemberEvidence([{ id: "", status: "blocked" }]),
    /duplicate or empty memberEvidence id/,
  );
  assert.throws(
    () =>
      parseMemberEvidence([
        { id: "kv::KVNamespace::get:method#0", status: "maybe" },
      ]),
    /invalid memberEvidence status/,
  );
  assert.throws(
    () => assertEvidenceBinding(["kv::KVNamespace::get:method#0"], evidence),
    /stale memberEvidence id: kv::KVNamespaceListResult::cursor:property#0/,
  );
});

test("generation is deterministic across two runs and matches the committed inventory", async () => {
  const { inventory, encoded, coverage } = await reportPromise;
  assert.equal(Object.hasOwn(inventory.managementApi, "routeGroups"), false);
  for (const route of inventory.managementApi.routes)
    for (const field of ["group", "sdk", "docs", "cf"])
      assert.equal(Object.hasOwn(route, field), false, `${route.id}: ${field}`);
  const second = encodeInventory(inventory);
  assert.equal(encoded, second);
  const committed = await readFile(COMMITTED, "utf8");
  assert.equal(encoded, committed);
  assert.equal(TYPE_ONLY_TARGET_SYMBOLS.size, 0);
  assert.equal(coverage.named_declarations, PINNED.named_declarations);
  assert.equal(coverage.target_declarations, PINNED.target_declarations);
  assert.equal(
    coverage.target_declarations_with_surface,
    PINNED.target_declarations_with_surface,
  );
  assert.equal(
    coverage.target_declarations_type_only,
    PINNED.target_declarations_type_only,
  );
  assert.equal(
    coverage.target_declarations_with_surface +
      coverage.target_declarations_type_only,
    coverage.target_declarations,
  );
  const members = allMembers(inventory);
  assert.equal(
    inventory.source.workers_types_version,
    PINNED.workers_types_version,
  );
  assert.equal(inventory.source.ast_sha256, PINNED.ast_sha256);
  assert.equal(members.length, PINNED.inventoried_members);
  assert.equal(
    new Set(members.map((member) => member.symbol)).size,
    PINNED.inventoried_symbols,
  );
  assert.ok(
    members.every(
      (member) =>
        member.id &&
        member.symbol &&
        member.member &&
        member.kind &&
        member.signature_sha256,
    ),
  );
});

test("inventories KV union, R2 composite, D1 intersection, DurableObjectStub composition, and Workflow/Queue aliases", async () => {
  const { inventory } = await reportPromise;
  const kv = namesOf(inventory, "KVNamespaceListResult");
  for (const name of ["cacheStatus", "cursor", "list_complete", "keys"])
    assert.ok(kv.has(name), name);
  assert.equal(
    membersOf(inventory, "KVNamespaceListResult").filter(
      (member) => member.member === "cursor",
    ).length,
    1,
  );
  assert.equal(
    membersOf(inventory, "KVNamespaceListResult").filter(
      (member) => member.member === "list_complete",
    ).length,
    2,
  );

  const r2 = namesOf(inventory, "R2Objects");
  for (const name of ["truncated", "cursor", "objects", "delimitedPrefixes"])
    assert.ok(r2.has(name), name);
  assert.equal(
    membersOf(inventory, "R2Objects").filter(
      (member) => member.member === "cursor",
    ).length,
    1,
  );
  assert.equal(
    membersOf(inventory, "R2Objects").filter(
      (member) => member.member === "truncated",
    ).length,
    2,
  );

  const d1 = namesOf(inventory, "D1Result");
  for (const name of ["results", "success", "meta", "error"])
    assert.ok(d1.has(name), name);

  const stub = namesOf(inventory, "DurableObjectStub");
  for (const name of ["id", "name", "fetch", "connect"])
    assert.ok(stub.has(name), name);

  const delay = namesOf(
    inventory,
    "CloudflareWorkersModule.WorkflowDelayFunction",
  );
  assert.ok(delay.has("()"));
  const workflowEvent = namesOf(
    inventory,
    "CloudflareWorkersModule.WorkflowEvent",
  );
  for (const name of [
    "payload",
    "timestamp",
    "instanceId",
    "workflowName",
    "schedule",
  ])
    assert.ok(workflowEvent.has(name), name);
  const deleted = namesOf(inventory, "WorkflowBatchDeleteResult");
  for (const name of ["deleted", "errors"]) assert.ok(deleted.has(name), name);
  const instance = namesOf(inventory, "InstanceStatus");
  for (const name of ["status", "error", "output"])
    assert.ok(instance.has(name), name);

  const queueHandler = namesOf(inventory, "ExportedHandlerQueueHandler");
  assert.ok(queueHandler.has("()"));
  const batch = namesOf(inventory, "MessageBatch");
  for (const name of ["messages", "queue", "metadata", "retryAll", "ackAll"])
    assert.ok(batch.has(name), name);
});

test("pinned socket metadata distinguishes outbound peers from inbound connect authority", async () => {
  const workerdRoot = join(ROOT, "third_party/workerd");
  const { revision } = JSON.parse(
    await readFile(join(ROOT, "packages/runtime/workerd.lock.json"), "utf8"),
  );
  // Archive-based builds do not require an initialized source submodule, and
  // shallow CI submodules may not contain the pinned release revision itself.
  if (
    existsSync(join(workerdRoot, ".git")) &&
    hasCommit(workerdRoot, revision)
  ) {
    const sockets = execFileSync(
      "git",
      ["show", `${revision}:src/workerd/api/sockets.c++`],
      {
        cwd: workerdRoot,
        encoding: "utf8",
        timeout: 10_000,
      },
    );
    assert.match(
      sockets,
      /setupSocket\(js,[\s\S]{0,320}kj::mv\(addressStr\),\s*kj::none \/\* localAddress \*\//,
    );
    const globalScope = execFileSync(
      "git",
      ["show", `${revision}:src/workerd/api/global-scope.c++`],
      {
        cwd: workerdRoot,
        encoding: "utf8",
        timeout: 10_000,
      },
    );
    assert.match(
      globalScope,
      /setupSocket\(js,[\s\S]{0,320}kj::mv\(clientAddress\),[\s\n]*kj::mv\(host\)/,
    );
  }
  const serviceTransport = await readFile(
    join(ROOT, "packages/runtime/src/services/transport.ts"),
    "utf8",
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
  const byId = new Map(members.map((member) => [member.id, member]));
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
  const blocked = members
    .filter((member) => member.status === "blocked")
    .map((member) => member.id)
    .sort();
  assert.deepEqual([...owned].sort(), blocked);
  const browser = inventory.products.browser_rendering;
  assert.equal(browser.kind, "target");
  assert.equal(browser.status, "blocked");
  assert.equal(browser.members.length, 417);
  assert.ok(browser.members.every((member) => member.status === "blocked"));
  const containers = inventory.products.containers;
  assert.equal(containers.kind, "target");
  assert.equal(containers.status, "supported");
  assert.equal(containers.members.length, 65);
  for (const member of containers.members) {
    assert.equal(member.status, "supported", member.id);
    assert.deepEqual(
      member.compile_cases,
      ["ts::test/conformance/fixtures/containers-surface.ts"],
      member.id,
    );
    assert.deepEqual(
      member.runtime_cases,
      [
        "containers::ordinary_sdk_native_execution_code_deploy_rollout_and_restart_use_real_containers",
      ],
      member.id,
    );
  }
  assert.ok(
    containers.members.some(
      (member) =>
        member.id === "containers::DurableObjectState::container:property#0",
    ),
  );
  assert.equal(
    inventory.products.durable_objects.members.some(
      (member) =>
        member.symbol === "DurableObjectState" && member.member === "container",
    ),
    false,
  );
  assert.deepEqual(
    namesOf(inventory, "BrowserRun"),
    new Set([
      "acquire",
      "closeSession",
      "connectSession",
      "devtools",
      "fetch",
      "getLiveView",
      "getSession",
      "history",
      "launch",
      "limits",
      "listSessions",
      "quickAction",
    ]),
  );
  assert.equal(
    membersOf(inventory, "BrowserRun").filter(
      (member) => member.member === "quickAction",
    ).length,
    9,
  );
  assert.deepEqual(
    blocked,
    [
      ...browser.members.map((member) => member.id),
      ...NEW_PIN_BLOCKED_MEMBERS,
      "dynamic_workers::WorkerLoaderWorkerCode::allowExperimental:property#0",
      "dynamic_workers::WorkerLoaderWorkerCode::streamingTails:property#0",
    ].sort(),
  );
  assert.equal(
    inventory.products.dynamic_workers.status,
    "supported_with_deviation",
  );
  assert.equal(inventory.products.images.kind, "platform");
  assert.equal(inventory.products.images.status, "supported_with_deviation");
  assert.equal(inventory.products.static_assets.kind, "platform");
  assert.equal(inventory.products.ai.kind, "target");
  assert.equal(inventory.products.ai.members.length, 54);
  assert.equal(inventory.products.artifacts.kind, "target");
  assert.equal(inventory.products.artifacts.status, "blocked");
  assert.deepEqual(inventory.products.artifacts.deviations, [
    "OC-ARTIFACTS-001",
  ]);
  assert.equal(inventory.products.artifacts.members.length, 60);
  for (const id of [
    "ai::Ai::aiGatewayLogId:property#0",
    "ai::Ai::toMarkdown:method#0",
    "ai::AiSearchNamespace::create:method#0",
    "ai::AiSearchInstance::search:method#0",
    "ai::AiSearchItems::uploadAndPoll:method#0",
    "ai::AiSearchJobs::create:method#0",
    "ai::ToMarkdownService::supported:method#0",
  ])
    assert.equal(byId.get(id)?.status, "supported_with_deviation", id);
  assert.deepEqual(
    [...namesOf(inventory, "Ai")],
    ["aiGatewayLogId", "toMarkdown"],
  );
  assert.equal(membersOf(inventory, "Ai").length, 4);
  assert.equal(
    catalog.blockedGaps.some(
      (gap) => gap.id === "workers.raw-tcp-security-boundary",
    ),
    false,
  );
  const raw = catalog.memberEvidence.filter((item) =>
    item.compileCases?.includes(
      "ts::test/conformance/fixtures/raw-tcp-surface.ts",
    ),
  );
  assert.equal(raw.length, 27);
  assert.ok(raw.every((item) => item.status === "supported_with_deviation"));
  const rawIds = new Set(raw.map((item) => item.id));
  for (const id of [
    "workers::ExportedHandlerConnectHandler::():call#0",
    "workers::TlsOptions::expectedServerHostname:property#0",
    "durable_objects::DurableObject::connect:method#0",
    "durable_objects::CloudflareWorkersModule.DurableObject::connect:method#0",
  ])
    assert.ok(rawIds.has(id), id);

  const socketInfo = new Map(
    membersOf(inventory, "SocketInfo").map((member) => [member.member, member]),
  );
  assert.equal(
    socketInfo.get("localAddress")?.signature,
    "localAddress?: string;",
  );
  assert.equal(
    socketInfo.get("remoteAddress")?.signature,
    "remoteAddress?: string;",
  );
  assert.equal(
    socketInfo.get("localAddress")?.status,
    "supported_with_deviation",
  );
  assert.equal(
    socketInfo.get("remoteAddress")?.status,
    "supported_with_deviation",
  );
});
