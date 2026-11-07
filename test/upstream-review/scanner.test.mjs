import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import test from "node:test";
import { classify } from "./scanner.ts";

// Classification fixtures: small synthetic OpenAPI snapshots, manifests, and
// SDK route closures prove the three-way classification rules offline.

const REVISION_A = "a".repeat(40);
const REVISION_B = "b".repeat(40);

function schemaDocument(paths, suffix = "") {
  return {
    openapi: "3.0.3",
    info: { version: "4.0.0" },
    paths: Object.fromEntries(
      paths.map((path) => [
        path,
        { get: { operationId: `op-${path}${suffix}` } },
      ]),
    ),
  };
}

function schemaBytes(suffix = "") {
  return Buffer.from(
    `${JSON.stringify(schemaDocument(["/accounts/{account_id}/queues"], suffix))}\n`,
  );
}

const manifest = {
  schemaVersion: 1,
  statusVocabulary: [
    "supported",
    "supported_with_deviation",
    "planned",
    "unsupported",
  ],
  operations: ["GET /accounts/{account_id}/queues"],
  deferredOperations: [],
  unsupportedOperations: [],
  sdkExcludedOperations: [],
};

function operationDigest(operationId) {
  return createHash("sha256")
    .update(`${JSON.stringify({ operationId })}\n`)
    .digest("hex");
}

function fixtureInput(overrides = {}) {
  const schemaBytesValue = schemaBytes();
  const candidateRoutes = new Map([
    [
      "GET /accounts/{}/queues",
      ["resources/queues/queues.mjs::BaseQueues::list"],
    ],
  ]);
  return {
    baseline: {
      openapiRevision: REVISION_A,
      openapiSha256: "1".repeat(64),
      cloudflareSdkVersion: "7.2.0",
      cfVersion: "1.0.0-beta.11",
    },
    candidateSchema: { revision: REVISION_A, bytes: schemaBytesValue },
    candidateCloudflare: {
      version: "7.2.0",
      npmShasum: "0".repeat(40),
      npmIntegrity: "sha512-",
    },
    candidateCf: {
      version: "1.0.0-beta.11",
      npmShasum: "0".repeat(40),
      npmIntegrity: "sha512-",
    },
    candidateCfEvidence: {
      packageSha256: "2".repeat(64),
      packageJsonSha256: "3".repeat(64),
      configSchemaSha256: "4".repeat(64),
      cliSha256: "5".repeat(64),
      missingConfigFields: [],
      missingBindings: [],
      missingCommands: [],
      unknownDifferences: [],
    },
    candidateSdkRoutes: candidateRoutes,
    manifest,
    sdkExcludedOperations: [],
    baselineMappedOperations: ["GET /accounts/{account_id}/queues"],
    baselineInventory: [
      {
        id: "GET /accounts/{account_id}/queues",
        operationSha256: operationDigest("op-/accounts/{account_id}/queues"),
      },
    ],
    selectedOperations: ["GET /accounts/{account_id}/queues"],
    ...overrides,
  };
}

test("identical upstream identities classify as unchanged", () => {
  const report = classify(fixtureInput());
  assert.equal(report.classification, "unchanged");
  assert.deepEqual(report.changedOperations, []);
  assert.deepEqual(report.removedOperations, []);
});

test("a schema move covered by a moved official SDK is ready", () => {
  const changed = schemaBytes("queues-moved");
  const report = classify(
    fixtureInput({
      candidateSchema: { revision: REVISION_B, bytes: changed },
      baselineInventory: [
        {
          id: "GET /accounts/{account_id}/queues",
          operationSha256: operationDigest("op-/accounts/{account_id}/queues"),
        },
      ],
      candidateCloudflare: {
        version: "7.3.0",
        npmShasum: "0".repeat(40),
        npmIntegrity: "sha512-",
      },
    }),
  );
  assert.equal(report.classification, "ready");
  assert.deepEqual(report.changedOperations, [
    "GET /accounts/{account_id}/queues",
  ]);
});

test("a schema move without a published SDK move stays blocked on the SDK", () => {
  const changed = schemaBytes("queues-moved");
  const report = classify(
    fixtureInput({
      candidateSchema: { revision: REVISION_B, bytes: changed },
      baselineInventory: [
        {
          id: "GET /accounts/{account_id}/queues",
          operationSha256: operationDigest("op-/accounts/{account_id}/queues"),
        },
      ],
    }),
  );
  assert.equal(report.classification, "blocked");
  assert.deepEqual(report.waitingOn, [
    "official cloudflare SDK stable release",
  ]);
});

test("a removed selected operation is breaking", () => {
  const report = classify(
    fixtureInput({
      candidateSchema: {
        revision: REVISION_B,
        bytes: Buffer.from('{"paths":{}}'),
      },
    }),
  );
  assert.equal(report.classification, "breaking");
  assert.deepEqual(report.removedOperations, [
    "GET /accounts/{account_id}/queues",
  ]);
});

test("a baseline-mapped operation lost from the candidate SDK is breaking", () => {
  const report = classify(
    fixtureInput({
      candidateSchema: { revision: REVISION_B, bytes: schemaBytes() },
      candidateCloudflare: {
        version: "7.3.0",
        npmShasum: "0".repeat(40),
        npmIntegrity: "sha512-",
      },
      candidateSdkRoutes: new Map(),
    }),
  );
  assert.equal(report.classification, "breaking");
  assert.match(
    report.reasons.join(" "),
    /no longer implements mapped operations/,
  );
});

test("a cf-only move with equivalent selected surface is ready", () => {
  const report = classify(
    fixtureInput({
      candidateCf: {
        version: "4.131.2",
        npmShasum: "0".repeat(40),
        npmIntegrity: "sha512-",
      },
    }),
  );
  assert.equal(report.classification, "ready");
  assert.match(report.reasons.join(" "), /preserves the selected/);
});

test("a Cf selected config or command removal is breaking", () => {
  const report = classify(
    fixtureInput({
      candidateCf: {
        version: "4.131.2",
        npmShasum: "0".repeat(40),
        npmIntegrity: "sha512-",
      },
      candidateCfEvidence: {
        ...fixtureInput().candidateCfEvidence,
        missingConfigFields: ["ai_search"],
        missingCommands: ["ai-search search"],
      },
    }),
  );
  assert.equal(report.classification, "breaking");
  assert.match(report.reasons.join(" "), /config field ai_search/);
  assert.match(report.reasons.join(" "), /command ai-search search/);
});

test("unknown Cf selected-surface drift is blocked", () => {
  const report = classify(
    fixtureInput({
      candidateCf: {
        version: "4.131.2",
        npmShasum: "0".repeat(40),
        npmIntegrity: "sha512-",
      },
      candidateCfEvidence: {
        ...fixtureInput().candidateCfEvidence,
        unknownDifferences: ["config schema has no RawConfig properties"],
      },
    }),
  );
  assert.equal(report.classification, "blocked");
  assert.deepEqual(report.waitingOn, ["Cf selected-surface review"]);
});

test("manifest-excluded operations do not force blocked or breaking", () => {
  const report = classify(
    fixtureInput({
      candidateCloudflare: {
        version: "7.3.0",
        npmShasum: "0".repeat(40),
        npmIntegrity: "sha512-",
      },
      candidateSdkRoutes: new Map(),
      sdkExcludedOperations: ["GET /accounts/{account_id}/queues"],
      baselineMappedOperations: [],
    }),
  );
  assert.equal(report.classification, "ready");
});

// The candidate renames `{account_id}` to `{account}`; the wire route is unchanged.
const RENAMED = "GET /accounts/{account}/queues";

function renamedSchema(paths = ["/accounts/{account}/queues"]) {
  return {
    revision: REVISION_B,
    bytes: Buffer.from(`${JSON.stringify(schemaDocument(paths))}\n`),
  };
}

const SDK_7_3 = {
  version: "7.3.0",
  npmShasum: "0".repeat(40),
  npmIntegrity: "sha512-",
};

test("a path-parameter rename covered by a moved official SDK is ready", () => {
  const report = classify(
    fixtureInput({
      candidateSchema: renamedSchema(),
      candidateCloudflare: SDK_7_3,
    }),
  );
  assert.equal(report.classification, "ready");
  assert.deepEqual(report.removedOperations, []);
  assert.deepEqual(report.renamedOperations, [
    { from: "GET /accounts/{account_id}/queues", to: RENAMED },
  ]);
  assert.deepEqual(report.changedOperations, [RENAMED]);
  assert.match(report.reasons.join(" "), /renamed path parameters/);
});

test("a path-parameter rename without a published SDK move stays blocked", () => {
  const report = classify(fixtureInput({ candidateSchema: renamedSchema() }));
  assert.equal(report.classification, "blocked");
  assert.deepEqual(report.removedOperations, []);
  assert.deepEqual(report.changedOperations, [RENAMED]);
  assert.deepEqual(report.waitingOn, [
    "official cloudflare SDK stable release",
  ]);
});

test("a renamed operation the candidate SDK no longer implements is breaking", () => {
  const report = classify(
    fixtureInput({
      candidateSchema: renamedSchema(),
      candidateCloudflare: SDK_7_3,
      candidateSdkRoutes: new Map(),
    }),
  );
  assert.equal(report.classification, "breaking");
  assert.deepEqual(report.renamedOperations, [
    { from: "GET /accounts/{account_id}/queues", to: RENAMED },
  ]);
  assert.match(
    report.reasons.join(" "),
    /no longer implements mapped operations: GET \/accounts\/\{account\}\/queues/,
  );
});

test("an ambiguous normalized match is a removal, not a rename", () => {
  const report = classify(
    fixtureInput({
      candidateSchema: renamedSchema([
        "/accounts/{account}/queues",
        "/accounts/{id}/queues",
      ]),
      candidateCloudflare: SDK_7_3,
    }),
  );
  assert.equal(report.classification, "breaking");
  assert.deepEqual(report.renamedOperations, []);
  assert.deepEqual(report.removedOperations, [
    "GET /accounts/{account_id}/queues",
  ]);
});
