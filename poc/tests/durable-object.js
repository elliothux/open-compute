"use strict";

const fs = require("node:fs");
const assert = require("../harness/assertions");
const { acquireWorkerd } = require("../harness/runtime");
const { WorkerdProcess, liveCount } = require("../harness/process-supervisor");

const CASE_NAMES = [
  "D01-facet-fetch",
  "D02-facet-rpc",
  "D03-object-isolation",
  "D04-storage-isolation",
  "D05-transaction-rollback",
  "D-same-facet-stable",
  "D-class-isolation",
  "D-dostorage-isolation",
  "D-independent-js-state",
  "D-concurrency-no-lost-update",
  "D-identity-safe",
  "D-invalid-inputs",
  "D-sanitized-logs",
  "no-leaked-workerd-child",
];

const ACCOUNT_ID = "acct_fixture";
const WORKER_ID = "worker_do";
const DEPLOYMENT_A = "deploy_a";
const STORE_A = "store_alpha";
const STORE_B = "store_beta";
const OBJECT_1 = "obj_alpha";
const OBJECT_2 = "obj_beta";
const OBJECT_CONC = "obj_conc";
const CLASS_COUNTER = "Counter";
const CLASS_ALT = "AltCounter";

const SECRET_TOKENS = [
  "g0-master-key",
  "/var/g0-data",
  "/Users/g0/secret.js",
  "secret.sqlite",
  "supervisor-only",
];

class Reporter {
  constructor(required, options = {}) {
    this.required = required;
    this.results = [];
    this.quiet = Boolean(options.quiet);
  }

  async test(name, fn) {
    const started = Date.now();
    try {
      await fn();
      this.results.push({ name, status: "passed", ms: Date.now() - started });
      if (!this.quiet) console.log(`passed          ${name}  ${Date.now() - started}ms`);
    } catch (err) {
      this.results.push({
        name,
        status: "failed",
        ms: Date.now() - started,
        error: String(err && err.message ? err.message : err),
      });
      if (!this.quiet) {
        console.log(`failed          ${name}  ${Date.now() - started}ms`);
        console.log(`  ${err && err.stack ? err.stack : err}`);
      }
    }
  }

  notRun(name, reason) {
    this.results.push({ name, status: "not-run", reason: reason || "not executed" });
    if (!this.quiet) {
      console.log(`not-run         ${name}`);
      if (reason) console.log(`  ${reason}`);
    }
  }

  summary() {
    const seen = new Set(this.results.map((r) => r.name));
    for (const name of this.required) {
      if (!seen.has(name)) this.notRun(name, "never started");
    }
    const passed = this.results.filter((r) => r.status === "passed").length;
    const failed = this.results.filter((r) => r.status === "failed").length;
    const notRun = this.results.filter((r) => r.status === "not-run").length;
    const code = failed === 0 && notRun === 0 ? 0 : 1;
    if (!this.quiet) {
      console.log("");
      console.log(`results: ${passed} passed, ${failed} failed, ${notRun} not-run`);
      console.log(
        JSON.stringify({
          suite: "durable-object",
          gates: ["G0.5"],
          results: this.results,
          passed,
          failed,
          notRun,
        })
      );
    }
    return { passed, failed, notRun, code };
  }
}

function proveRequiredNotRunFailsClosed() {
  const probe = new Reporter(["required-not-run-probe"], { quiet: true });
  const summary = probe.summary();
  if (summary.failed !== 0 || summary.notRun !== 1 || summary.code === 0) {
    throw new Error(
      `required not-run must yield nonzero status (failed=${summary.failed} notRun=${summary.notRun} code=${summary.code})`
    );
  }
  return summary;
}

function parseAppLogs(text) {
  const entries = [];
  if (!text) return entries;
  for (const line of text.split("\n")) {
    if (!line.trim()) continue;
    try {
      const wrap = JSON.parse(line);
      const message = wrap && wrap.message;
      if (typeof message === "string" && message.startsWith("{")) {
        entries.push(JSON.parse(message));
      }
    } catch {
      /* ignore non-JSON */
    }
  }
  return entries;
}

function readStdout(proc) {
  try {
    return fs.readFileSync(proc.stdoutPath, "utf8");
  } catch {
    return "";
  }
}

const tenantVisibleBodies = [];
let tenantScanPaths = [];

function assertNoSecrets(text, label) {
  const haystack = text == null ? "" : String(text);
  for (const token of SECRET_TOKENS) {
    assert.excludes(haystack, token, `${label} must not contain ${token}`);
  }
  assert.excludes(haystack, "/Users/", `${label} must not contain /Users/`);
}

function assertTenantVisibleSafe(text, label) {
  const haystack = text == null ? "" : String(text);
  assertNoSecrets(haystack, label);
  assert.excludes(haystack, ".sqlite", `${label} must not contain sqlite filename`);
  for (const p of tenantScanPaths) {
    assert.excludes(haystack, p, `${label} must not contain path`);
  }
}

function notEqual(actual, expected, message) {
  if (actual === expected) {
    throw new Error(`${message}: expected values to differ, both ${JSON.stringify(actual)}`);
  }
}

function target(overrides = {}) {
  return {
    accountId: ACCOUNT_ID,
    workerId: WORKER_ID,
    deploymentId: DEPLOYMENT_A,
    doStorageId: STORE_A,
    className: CLASS_COUNTER,
    objectId: OBJECT_1,
    ...overrides,
  };
}

function expectedFacetName(doStorageId, className, objectId) {
  return `v1/s/${doStorageId}/c/${className}/o/${objectId}`;
}

function assertFacetName(res, fields, message) {
  const expected = expectedFacetName(fields.doStorageId, fields.className, fields.objectId);
  assert.equal(res.json?.facetName, expected, `${message}: facetName`);
  assert.excludes(res.json.facetName, fields.deploymentId, `${message}: facetName has no deploymentId`);
  assert.excludes(res.json.facetName, ACCOUNT_ID, `${message}: facetName has no accountId`);
}

function assertTenantError(res, errorCode, message) {
  assert.equal(res.json?.ok, false, `${message}: ok`);
  assert.equal(res.json?.errorCode, errorCode, `${message}: errorCode`);
  assert.isTrue(res.json?.requestId != null && res.json.requestId !== "", `${message}: requestId`);
  const keys = Object.keys(res.json || {}).sort();
  assert.deepEqual(
    keys,
    ["deploymentId", "errorCode", "ok", "requestId"].sort(),
    `${message}: tenant error shape`
  );
  assertNoSecrets(res.text, `${message} body`);
}

async function scannedDoOp(client, body) {
  const res = await client.doOp(body);
  tenantVisibleBodies.push({ op: body.op || "unknown", text: res.text });
  assertTenantVisibleSafe(res.text, `do ${body.op || "unknown"} body`);
  return res;
}

async function doOp(client, op, overrides = {}) {
  return scannedDoOp(client, { op, ...target(overrides) });
}

async function increment(client, overrides = {}) {
  const res = await doOp(client, "increment", overrides);
  assert.okStatus(res, `increment ${JSON.stringify(overrides)}`);
  assert.equal(res.json.ok, true, "increment ok");
  return res;
}

async function getValue(client, overrides = {}) {
  const res = await doOp(client, "getValue", overrides);
  assert.okStatus(res, `getValue ${JSON.stringify(overrides)}`);
  assert.equal(res.json.ok, true, "getValue ok");
  return res;
}

async function run() {
  const reporter = new Reporter(CASE_NAMES);
  console.log("G0.5 native Durable Object facets");

  try {
    const probe = proveRequiredNotRunFailsClosed();
    console.log(
      `self-check      required not-run yields code=${probe.code} (notRun=${probe.notRun})`
    );
  } catch (err) {
    console.log(`failed          not-run-self-check`);
    console.log(`  ${err && err.stack ? err.stack : err}`);
    return 1;
  }

  let acquired;
  try {
    acquired = await acquireWorkerd();
  } catch (err) {
    console.log(`failed          acquire-pinned-workerd`);
    console.log(`  ${err && err.stack ? err.stack : err}`);
    for (const name of CASE_NAMES) reporter.notRun(name, "workerd acquire failed");
    return reporter.summary().code || 1;
  }

  const proc = new WorkerdProcess({
    binPath: acquired.binPath,
    lock: acquired.lock,
  });

  try {
    await proc.start();
  } catch (err) {
    proc.retainFailed(err);
    console.log(`failed          start-workerd`);
    console.log(`  ${err && err.stack ? err.stack : err}`);
    for (const name of CASE_NAMES) reporter.notRun(name, "workerd start failed");
    try {
      await proc.kill("SIGKILL");
    } catch {
      /* ignore */
    }
    return reporter.summary().code || 1;
  }

  const client = proc.client;
  const fixturesDir = proc.fixturesDir;
  tenantScanPaths = [fixturesDir, proc.dataDir].filter(Boolean);
  tenantVisibleBodies.length = 0;
  const classifications = {};
  let d01Values = [];
  let d01FacetName = null;
  let object1Nonce = null;

  try {
    await reporter.test("D01-facet-fetch", async () => {
      d01Values = [];
      for (const expected of [1, 2, 3]) {
        const res = await increment(client);
        assert.equal(res.json.value, expected, `fetch increment ${expected}`);
        assert.equal(res.json.codeVersion, "A", "codeVersion A");
        assertFacetName(res, target(), `increment ${expected}`);
        d01Values.push(res.json.value);
        d01FacetName = res.json.facetName;
        object1Nonce = res.json.jsNonce;
      }
      assert.deepEqual(d01Values, [1, 2, 3], "monotonic 1,2,3");
      assert.isTrue(object1Nonce != null && object1Nonce !== "", "jsNonce present");
    });

    await reporter.test("D02-facet-rpc", async () => {
      const res = await getValue(client);
      assert.equal(res.json.value, 3, "RPC getValue is 3");
      assert.equal(res.json.codeVersion, "A", "RPC codeVersion A");
      assert.equal(res.json.facetName, d01FacetName, "RPC hits the same facet");
      assert.equal(res.json.jsNonce, object1Nonce, "RPC same JS instance");
      assertFacetName(res, target(), "getValue");
    });

    await reporter.test("D03-object-isolation", async () => {
      const empty = await getValue(client, { objectId: OBJECT_2 });
      assert.equal(empty.json.value, 0, "object-2 starts at 0");
      assert.equal(empty.json.codeVersion, "A", "object-2 codeVersion A");
      notEqual(empty.json.facetName, d01FacetName, "object-2 has its own facet");
      assertFacetName(empty, target({ objectId: OBJECT_2 }), "object-2");
      const still = await getValue(client);
      assert.equal(still.json.value, 3, "object-1 remains 3");
      const inc2 = await increment(client, { objectId: OBJECT_2 });
      assert.equal(inc2.json.value, 1, "object-2 increment is independent");
      const still1 = await getValue(client);
      assert.equal(still1.json.value, 3, "object-1 unchanged by object-2");
    });

    await reporter.test("D04-storage-isolation", async () => {
      const supervisor = await scannedDoOp(client, { op: "probeSupervisor" });
      assert.okStatus(supervisor, "probeSupervisor");
      assert.equal(supervisor.json.ok, true, "probeSupervisor ok");
      assert.isTrue(
        (supervisor.json.tables || []).includes("supervisor_private"),
        "supervisor sees its private table"
      );
      assert.isTrue(
        (supervisor.json.tables || []).includes("supervisor_meta"),
        "supervisor sees its meta table"
      );
      assert.equal(supervisor.json.privateValuePresent, true, "supervisor private SQL exists");
      assert.isTrue(
        supervisor.json.secret === undefined,
        "probeSupervisor omits private value field"
      );
      assert.equal(supervisor.json.counterVisible, false, "supervisor cannot see facet counter");
      assert.excludes(JSON.stringify(supervisor.json.tables), "counter", "supervisor tables omit counter");
      assertTenantVisibleSafe(supervisor.text, "probeSupervisor");
      assert.excludes(supervisor.text, "supervisor-only", "probeSupervisor omits private value");

      const facet = await doOp(client, "probeFacet");
      assert.okStatus(facet, "probeFacet");
      assert.equal(facet.json.ok, true, "probeFacet ok");
      assert.isTrue((facet.json.tables || []).includes("counter"), "facet sees counter");
      assert.isFalse(
        (facet.json.tables || []).includes("supervisor_private"),
        "facet cannot see supervisor_private"
      );
      assert.isFalse(
        (facet.json.tables || []).includes("supervisor_meta"),
        "facet cannot see supervisor_meta"
      );
      assert.equal(facet.json.supervisorSecret?.visible, false, "supervisor secret not visible");
      assert.equal(facet.json.supervisorSecret?.error, "not-visible", "secret probe is not-visible");
      assertTenantVisibleSafe(facet.text, "probeFacet");
      assert.excludes(facet.text, "supervisor-only", "probeFacet omits private value");
    });

    await reporter.test("D05-transaction-rollback", async () => {
      const before = await getValue(client);
      const fail = await doOp(client, "failAfterWrite");
      assert.okStatus(fail, "failAfterWrite");
      assert.equal(fail.json.ok, true, "failAfterWrite ok");
      assert.equal(fail.json.threw, true, "transaction threw");
      assert.equal(fail.json.before, before.json.value, "before matches current value");
      assert.equal(fail.json.after, before.json.value, "after rolled back");
      assert.equal(fail.json.classification, "not-applied", "classified not-applied");
      classifications["D05-transaction-rollback"] = "not-applied";
      const after = await getValue(client);
      assert.equal(after.json.value, before.json.value, "RPC value unchanged after rollback");
    });

    await reporter.test("D-same-facet-stable", async () => {
      const first = await increment(client);
      const second = await increment(client);
      assert.equal(first.json.facetName, d01FacetName, "same facet after earlier ops");
      assert.equal(second.json.facetName, first.json.facetName, "repeated get is the same facet");
      assert.equal(second.json.value, first.json.value + 1, "count continues on the same facet");
      assert.equal(first.json.jsNonce, object1Nonce, "JS nonce stable");
      assert.equal(second.json.jsNonce, object1Nonce, "JS nonce still stable");
      assert.equal(second.json.value, 5, "object-1 continued to 5");
    });

    await reporter.test("D-class-isolation", async () => {
      const alt = await increment(client, { className: CLASS_ALT });
      assert.equal(alt.json.value, 1, "AltCounter starts at 1");
      assert.equal(alt.json.codeVersion, "alt", "alt codeVersion");
      assertFacetName(alt, target({ className: CLASS_ALT }), "AltCounter");
      notEqual(alt.json.facetName, d01FacetName, "different class is a different facet");
      const original = await getValue(client);
      assert.equal(original.json.value, 5, "Counter object-1 unchanged by AltCounter");
      assert.equal(original.json.codeVersion, "A", "original still A");
      const altAgain = await getValue(client, { className: CLASS_ALT });
      assert.equal(altAgain.json.value, 1, "AltCounter stays at 1");
    });

    await reporter.test("D-dostorage-isolation", async () => {
      const other = await increment(client, { doStorageId: STORE_B });
      assert.equal(other.json.value, 1, "other doStorageId starts at 1");
      assertFacetName(other, target({ doStorageId: STORE_B }), "store_beta");
      notEqual(other.json.facetName, d01FacetName, "storage namespace is a different facet");
      const original = await getValue(client);
      assert.equal(original.json.value, 5, "original storage unchanged");
      const otherRead = await getValue(client, { doStorageId: STORE_B });
      assert.equal(otherRead.json.value, 1, "other storage stays at 1");
    });

    await reporter.test("D-independent-js-state", async () => {
      const one = await getValue(client);
      const two = await increment(client, { objectId: OBJECT_2 });
      const oneAfter = await getValue(client);
      notEqual(one.json.jsNonce, two.json.jsNonce, "different objects have different JS nonce");
      assert.equal(oneAfter.json.jsNonce, one.json.jsNonce, "object-1 nonce unchanged");
      assert.equal(oneAfter.json.jsTicks, one.json.jsTicks, "object-1 JS ticks unchanged by object-2");
      assert.isTrue(two.json.jsTicks >= 1, "object-2 has its own JS ticks");

      const [left, right] = await Promise.all([
        increment(client, { objectId: "obj_p1" }),
        increment(client, { objectId: "obj_p2" }),
      ]);
      assert.equal(left.json.value, 1, "obj_p1 independent start");
      assert.equal(right.json.value, 1, "obj_p2 independent start");
      notEqual(left.json.facetName, right.json.facetName, "parallel objects use distinct facets");
      notEqual(left.json.jsNonce, right.json.jsNonce, "parallel objects do not share JS state");
    });

    await reporter.test("D-concurrency-no-lost-update", async () => {
      const n = 12;
      const results = await Promise.all(
        Array.from({ length: n }, () => increment(client, { objectId: OBJECT_CONC }))
      );
      const values = results.map((res) => res.json.value).sort((a, b) => a - b);
      assert.deepEqual(
        values,
        Array.from({ length: n }, (_, i) => i + 1),
        "concurrent increments are unique and monotonic"
      );
      for (const res of results) {
        assert.equal(res.json.facetName, expectedFacetName(STORE_A, CLASS_COUNTER, OBJECT_CONC), "same facet");
        assert.equal(res.json.codeVersion, "A", "concurrent codeVersion A");
      }
      const final = await getValue(client, { objectId: OBJECT_CONC });
      assert.equal(final.json.value, n, "final count equals successful increments");
      const uniqueNonces = new Set(results.map((res) => res.json.jsNonce));
      assert.equal(uniqueNonces.size, 1, "one object keeps one JS instance");
    });

    await reporter.test("D-identity-safe", async () => {
      const res = await doOp(client, "getIdentity");
      assert.okStatus(res, "getIdentity");
      const identity = res.json.identity || {};
      assert.equal(identity.codeVersion, "A", "identity codeVersion");
      assert.equal(identity.id, OBJECT_1, "ctx.id string is the objectId");
      assert.isTrue(identity.name === null || identity.name === OBJECT_1, "ctx.id.name is safe");
      const dumped = JSON.stringify(res.json);
      assertNoSecrets(dumped, "identity");
      assert.excludes(dumped, fixturesDir, "identity has no fixture path");
      assert.excludes(dumped, proc.dataDir, "identity has no data dir");
      assert.excludes(dumped, ".sqlite", "identity has no sqlite filename");
      assert.excludes(dumped, "supervisor_private", "identity has no supervisor table");
      assert.excludes(dumped.toLowerCase(), "stack", "identity has no stack");
    });

    await reporter.test("D-invalid-inputs", async () => {
      const before = await getValue(client);
      const cases = [
        { label: "missing class", body: { className: "" }, code: "IDENTIFIER_INVALID" },
        { label: "missing object", body: { objectId: "" }, code: "IDENTIFIER_INVALID" },
        { label: "missing storage", body: { doStorageId: "" }, code: "IDENTIFIER_INVALID" },
        {
          label: "overlong class",
          body: { className: "C".repeat(65) },
          code: "IDENTIFIER_INVALID",
        },
        {
          label: "path class",
          body: { className: "../Counter" },
          code: "IDENTIFIER_INVALID",
        },
        {
          label: "slash object",
          body: { objectId: "obj/alpha" },
          code: "IDENTIFIER_INVALID",
        },
        {
          label: "space storage",
          body: { doStorageId: "store alpha" },
          code: "IDENTIFIER_INVALID",
        },
        {
          label: "malformed unicode",
          body: { objectId: "obj_\uD800" },
          code: "IDENTIFIER_INVALID",
        },
        {
          label: "unpaired surrogate class",
          body: { className: "Counter\uDC00" },
          code: "IDENTIFIER_INVALID",
        },
        {
          label: "unknown class",
          body: { className: "MissingClass" },
          code: "CLASS_NOT_FOUND",
        },
        {
          label: "unknown deployment",
          body: { deploymentId: "deploy_missing" },
          code: "DEPLOYMENT_NOT_FOUND",
        },
      ];
      for (const row of cases) {
        const res = await doOp(client, "increment", row.body);
        assertTenantError(res, row.code, row.label);
      }
      const after = await increment(client);
      assert.equal(after.json.value, before.json.value + 1, "valid facet unaffected by invalid inputs");
      assert.equal(after.json.facetName, d01FacetName, "valid facet identity unchanged");
    });

    await reporter.test("D-sanitized-logs", async () => {
      const app = parseAppLogs(readStdout(proc));
      assert.isTrue(app.length > 0, "structured app logs present");
      const doLogs = app.filter(
        (e) => e.dispatchKind === "do-fetch" || e.dispatchKind === "do-rpc" || e.dispatchKind === "do"
      );
      assert.isTrue(doLogs.length > 0, "DO logs present");
      const okLogs = doLogs.filter((e) => e.outcome === "ok" && e.className === CLASS_COUNTER);
      assert.isTrue(okLogs.length > 0, "successful Counter logs present");
      for (const entry of okLogs) {
        assert.isTrue("deploymentId" in entry, "deploymentId present");
        assert.isTrue("doStorageIdHash" in entry, "doStorageIdHash present");
        assert.isTrue(typeof entry.doStorageIdHash === "string", "doStorageIdHash string");
        assert.equal(entry.className, CLASS_COUNTER, "className present");
        assert.isTrue(typeof entry.objectIdHash === "string", "objectIdHash string");
        assert.isTrue(typeof entry.durationMs === "number", "durationMs present");
        assert.equal(entry.outcome, "ok", "outcome ok");
      }
      const errLogs = doLogs.filter((e) => e.outcome === "error");
      assert.isTrue(errLogs.length > 0, "error logs present");
      for (const entry of errLogs) {
        assert.isTrue(typeof entry.errorCode === "string" && entry.errorCode.length > 0, "errorCode");
      }
      const appText = JSON.stringify(app);
      assertNoSecrets(appText, "app logs");
      assert.excludes(appText, fixturesDir, "logs have no fixture path");
      assert.excludes(appText, proc.dataDir, "logs have no data dir");
      assert.excludes(appText, STORE_A, "logs hash doStorageId");
      assert.excludes(appText, OBJECT_1, "logs hash objectId");
      assert.excludes(appText, OBJECT_CONC, "logs hash concurrent objectId");
      assert.excludes(appText, ".sqlite", "logs omit sqlite filenames");
      assert.excludes(appText, "g0-fail-after-write", "logs omit tenant/fixture bodies");
      assert.isTrue(tenantVisibleBodies.length > 0, "tenant-visible DO responses recorded");
      for (const body of tenantVisibleBodies) {
        assertTenantVisibleSafe(body.text, `recorded ${body.op} body`);
      }
    });
  } catch (err) {
    proc.retainFailed(err);
    console.log(`failed          durable-object-suite`);
    console.log(`  ${err && err.stack ? err.stack : err}`);
  } finally {
    try {
      if (proc.isAlive()) await proc.stop("SIGTERM");
      await proc.cleanupSuccess();
    } catch {
      try {
        await proc.kill("SIGKILL");
      } catch {
        /* ignore */
      }
    }
  }

  await reporter.test("no-leaked-workerd-child", async () => {
    assert.equal(liveCount(), 0, "supervisor live child count");
  });

  for (const row of reporter.results) {
    if (classifications[row.name]) row.classification = classifications[row.name];
  }
  const summary = reporter.summary();
  if (summary.failed === 0 && summary.notRun === 0) {
    console.log("G0.5: PASS");
  } else {
    console.log("G0.5: FAIL");
  }
  return summary.code;
}

module.exports = { run };

if (require.main === module) {
  run()
    .then((code) => process.exit(code))
    .catch((err) => {
      console.error(err);
      process.exit(1);
    });
}
