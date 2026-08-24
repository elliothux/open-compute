"use strict";

const fs = require("node:fs");
const assert = require("../harness/assertions");
const { acquireWorkerd } = require("../harness/runtime");
const { WorkerdProcess, liveCount } = require("../harness/process-supervisor");
const { KEYS } = require("../harness/fixture-loader");

const CASE_NAMES = [
  "B-cold-warm-scope",
  "B01-resource-isolation",
  "B02-forged-scope",
  "B03-safe-error",
  "B-path-url-as-data",
  "B-capability-surface",
  "B-structured-clone",
  "B-fault-f4-not-applied",
  "B-fault-f5-applied",
  "B-fault-isolation",
  "B-unbound-worker-a-unaffected",
  "B-sanitized-logs",
  "no-leaked-workerd-child",
];

const SECRET_TOKENS = [
  "g0-master-key",
  "/var/g0-data",
  "/Users/g0/secret.js",
  "secret.sqlite",
  "internal adapter failure",
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
          suite: "binding",
          gates: ["G0.4"],
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

function kvEnvelope(deploymentId, pathname, body, extra = {}) {
  return {
    kind: "fetch",
    accountId: "acct_fixture",
    workerId: "worker_kv",
    deploymentId,
    entrypoint: null,
    url: `https://g0.invalid${pathname}`,
    method: "POST",
    body: body ?? {},
    ...extra,
  };
}

function workerAEnvelope() {
  return {
    kind: "fetch",
    accountId: "acct_fixture",
    workerId: "worker_fixture",
    deploymentId: "deploy_a",
    entrypoint: null,
  };
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

function assertNoSecrets(text, label) {
  const haystack = text == null ? "" : String(text);
  for (const token of SECRET_TOKENS) {
    assert.excludes(haystack, token, `${label} must not contain ${token}`);
  }
  assert.excludes(haystack, "/Users/", `${label} must not contain /Users/`);
}

async function kvCall(client, deploymentId, pathname, body, headers) {
  const res = await client.dispatch(kvEnvelope(deploymentId, pathname, body), headers);
  return res;
}

async function run() {
  const reporter = new Reporter(CASE_NAMES);
  console.log("G0.4 binding-scoped host adapter");

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
  const classifications = {};

  try {
    await reporter.test("B-cold-warm-scope", async () => {
      const cold = await kvCall(client, "deploy_a", "/get", { key: "shared" });
      assert.okStatus(cold, "cold A get");
      assert.equal(cold.json.ok, true, "cold A ok");
      assert.equal(cold.json.value, "A", "cold A shared");
      assert.equal(cold.headers.get("x-g0-loader-outcome"), "cold", "cold outcome");
      const warm = await kvCall(client, "deploy_a", "/get", { key: "shared" });
      assert.okStatus(warm, "warm A get");
      assert.equal(warm.json.ok, true, "warm A ok");
      assert.equal(warm.json.value, "A", "warm A shared");
      assert.equal(warm.headers.get("x-g0-loader-outcome"), "warm", "warm outcome");
      const st = await client.stats();
      assert.okStatus(st, "loader stats");
      assert.equal(st.json.callbacks?.[KEYS.KV_A], 1, "binding A callback once");
      assert.equal(st.json.lastOutcome[KEYS.KV_A], "warm", "binding A warm stats");
    });

    await reporter.test("B01-resource-isolation", async () => {
      const a = await kvCall(client, "deploy_a", "/get", { key: "shared" });
      assert.okStatus(a, "B01 A get");
      assert.equal(a.json.value, "A", "binding A reads A");
      const b = await kvCall(client, "deploy_b", "/get", { key: "shared" });
      assert.okStatus(b, "B01 B get");
      assert.equal(b.json.value, "B", "binding B reads B");
      const putA = await kvCall(client, "deploy_a", "/put", { key: "only-a", value: "from-a" });
      assert.okStatus(putA, "put only-a");
      assert.equal(putA.json.ok, true, "put only-a ok");
      const putB = await kvCall(client, "deploy_b", "/put", { key: "only-b", value: "from-b" });
      assert.okStatus(putB, "put only-b");
      assert.equal(putB.json.ok, true, "put only-b ok");
      const aSeesB = await kvCall(client, "deploy_a", "/get", { key: "only-b" });
      assert.equal(aSeesB.json.value, null, "A cannot read B's key");
      const bSeesA = await kvCall(client, "deploy_b", "/get", { key: "only-a" });
      assert.equal(bSeesA.json.value, null, "B cannot read A's key");
      const aOwn = await kvCall(client, "deploy_a", "/get", { key: "only-a" });
      assert.equal(aOwn.json.value, "from-a", "A reads its own put");
      const bOwn = await kvCall(client, "deploy_b", "/get", { key: "only-b" });
      assert.equal(bOwn.json.value, "from-b", "B reads its own put");
      const aShared = await kvCall(client, "deploy_a", "/get", { key: "shared" });
      const bShared = await kvCall(client, "deploy_b", "/get", { key: "shared" });
      assert.equal(aShared.json.value, "A", "shared A unchanged");
      assert.equal(bShared.json.value, "B", "shared B unchanged");
    });

    await reporter.test("B02-forged-scope", async () => {
      const forged = await kvCall(
        client,
        "deploy_a",
        "/get",
        { key: "shared", resourceId: "kv_fixture_b" },
        { "x-resource-id": "kv_fixture_b" }
      );
      assert.okStatus(forged, "forged body resourceId");
      assert.equal(forged.json.value, "A", "body resourceId cannot switch A to B");

      const viaHeaders = await client.dispatch(
        kvEnvelope("deploy_a", "/forge", { key: "shared" }, {
          headers: { "x-resource-id": "kv_fixture_b" },
        })
      );
      assert.okStatus(viaHeaders, "forge");
      assert.equal(viaHeaders.json.ok, true, "forge ok");
      const byVia = Object.fromEntries((viaHeaders.json.attempts || []).map((row) => [row.via, row]));
      assert.equal(byVia["second-arg"]?.value, "A", "extra get arg cannot switch scope");
      assert.equal(byVia["third-arg-put-ignored"]?.value, "from-a", "extra put arg stays on A");
      assert.equal(viaHeaders.json.envIdentity?.deploymentId, "deploy_a", "identity stays A");
      assert.isFalse(
        Object.prototype.hasOwnProperty.call(viaHeaders.json.envIdentity || {}, "resourceId"),
        "env identity has no resourceId selector"
      );
      const stillA = await kvCall(client, "deploy_a", "/get", { key: "shared" });
      assert.equal(stillA.json.value, "A", "A unchanged after forge");
      const stillB = await kvCall(client, "deploy_b", "/get", { key: "shared" });
      assert.equal(stillB.json.value, "B", "B unchanged after forge");
      const bDoesNotSeeClaim = await kvCall(client, "deploy_b", "/get", { key: "forge-claim" });
      assert.equal(bDoesNotSeeClaim.json.value, null, "forged put did not land in B");
    });

    await reporter.test("B03-safe-error", async () => {
      const res = await kvCall(client, "deploy_a", "/error", {});
      assert.equal(res.json?.ok, false, "internal error is a failure");
      assert.isFalse(res.json?.unexpected === true, "must not succeed");
      assert.equal(res.json?.message, "BINDING_INTERNAL", "stable tenant-safe message");
      assertNoSecrets(res.text, "tenant error body");
      assertNoSecrets(res.json?.stack, "tenant error stack");
      assertNoSecrets(res.json?.message, "tenant error message");
      assert.excludes(res.text, fixturesDir, "no fixture path");
      assert.excludes(res.text, proc.dataDir, "no data dir");
      assert.isFalse(/at BindingBackend/.test(res.text), "no host frame");
    });

    await reporter.test("B-path-url-as-data", async () => {
      const res = await kvCall(client, "deploy_a", "/forge", { key: "shared" });
      assert.okStatus(res, "path/url forge");
      const byVia = Object.fromEntries((res.json.attempts || []).map((row) => [row.via, row]));
      assert.equal(byVia["key-path"]?.value, null, "absolute path is ordinary missing key");
      assert.equal(byVia["internal-url-key"]?.value, null, "internal URL is ordinary missing key");
      assert.equal(byVia["other-resource-id-key"]?.value, null, "other resource id is ordinary key");
      assert.equal(
        byVia["path-url-as-data"]?.value,
        "http://127.0.0.1/internal",
        "path-like key stores ordinary string value"
      );
      const asData = await kvCall(client, "deploy_a", "/get", { key: "/etc/passwd" });
      assert.equal(asData.json.value, "http://127.0.0.1/internal", "path key still scoped to A");
      const bPath = await kvCall(client, "deploy_b", "/get", { key: "/etc/passwd" });
      assert.equal(bPath.json.value, null, "path key did not select B or host files");
    });

    await reporter.test("B-capability-surface", async () => {
      const probe = await kvCall(client, "deploy_a", "/probe", {});
      assert.okStatus(probe, "probe");
      assert.deepEqual(probe.json.envKeys, ["G0_IDENTITY", "KV"].sort(), "env only identity + KV");
      assert.isFalse(probe.json.hasBackend, "no BINDING_BACKEND");
      assert.isFalse(probe.json.hasBindingHost, "no BINDING_HOST");
      assert.isFalse(probe.json.hasLoader, "no LOADER");
      assert.isFalse(probe.json.hasFixtures, "no FIXTURES");
      assert.isFalse(probe.json.hasEcho, "no ECHO");
      const calls = probe.json.calls || {};
      for (const name of [
        "list",
        "admin",
        "openFile",
        "listResources",
        "stats",
        "setFault",
        "selectResource",
        "dump",
        "connect",
      ]) {
        assert.equal(calls[name]?.ok, false, `${name} is not an available capability`);
        assertNoSecrets(JSON.stringify(calls[name]), `${name} error`);
        const dumped = JSON.stringify(calls[name]);
        assert.excludes(dumped, "kv_fixture_b", `${name} must not enumerate resources`);
        assert.excludes(dumped, "shared", `${name} must not dump store data`);
      }
      assert.equal(probe.json.methodTypes?.get, "function", "get is the facade");
      assert.equal(probe.json.methodTypes?.put, "function", "put is the facade");
      for (const key of probe.json.kvKeys || []) {
        assert.isFalse(
          ["list", "admin", "openFile", "listResources", "stats", "setFault"].includes(key),
          `kv must not enumerate ${key}`
        );
      }
      assert.deepEqual(probe.json.identityKeys, ["accountId", "deploymentId", "workerId"].sort(), "identity keys");
      assert.isFalse(
        Object.prototype.hasOwnProperty.call(probe.json.identity || {}, "resourceId"),
        "identity has no resourceId"
      );

      const fetchKv = await kvCall(client, "deploy_a", "/fetch-kv", {});
      assert.equal(fetchKv.json?.ok, false, "generic fetch denied");
      assert.match(String(fetchKv.json?.message || ""), /BINDING_DENIED/, "fetch denied code");
      assertNoSecrets(fetchKv.text, "fetch-kv body");
      assert.excludes(fetchKv.text, "example.com", "must not fetch arbitrary URL");
    });

    await reporter.test("B-structured-clone", async () => {
      const res = await kvCall(client, "deploy_a", "/clone", {});
      assert.okStatus(res, "clone");
      const byName = Object.fromEntries((res.json.results || []).map((row) => [row.name, row]));
      assert.equal(byName.string?.ok, true, "string put supported");
      assert.equal(byName.string?.output?.value, "clone-ok", "string round-trip");
      assert.equal(byName["empty-string"]?.ok, true, "empty string supported");
      assert.equal(byName["empty-string"]?.output?.value, "", "empty string round-trip");
      for (const name of [
        "number",
        "boolean",
        "null",
        "object",
        "array",
        "undefined",
        "function",
        "symbol",
        "bigint",
        "date",
        "map",
        "bytes",
      ]) {
        assert.equal(byName[name]?.ok, false, `${name} must fail at facade/JSRPC boundary`);
        assertNoSecrets(JSON.stringify(byName[name]), `${name} error`);
        assert.isTrue(byName[name]?.error?.message != null, `${name} has a stable error`);
      }
      assert.equal(res.json.priorAfter, "prior", "unsupported values did not corrupt prior data");
      assert.equal(res.json.tryAfter, "", "last supported write is empty string");
      const after = await kvCall(client, "deploy_a", "/get", { key: "clone-prior" });
      assert.equal(after.json.value, "prior", "clone-prior intact");
      const bClone = await kvCall(client, "deploy_b", "/get", { key: "clone-prior" });
      assert.equal(bClone.json.value, null, "clone writes stayed on A");
    });

    await reporter.test("B-fault-f4-not-applied", async () => {
      const enable = await client.fault({
        target: "binding",
        point: "F4",
        enabled: true,
        resourceId: "kv_fixture_a",
      });
      assert.okStatus(enable, "enable F4");
      const put = await kvCall(client, "deploy_a", "/put", { key: "f4-key", value: "should-not-apply" });
      assert.equal(put.json?.ok, false, "F4 put fails");
      assert.equal(put.json?.message, "BINDING_INTERNAL", "F4 is tenant-safe");
      const disable = await client.fault({ target: "binding", point: "F4", enabled: false });
      assert.okStatus(disable, "disable F4");
      const got = await kvCall(client, "deploy_a", "/get", { key: "f4-key" });
      assert.equal(got.json.value, null, "F4 write not-applied");
      classifications["B-fault-f4-not-applied"] = "not-applied";
    });

    await reporter.test("B-fault-f5-applied", async () => {
      const enable = await client.fault({
        target: "binding",
        point: "F5",
        enabled: true,
        resourceId: "kv_fixture_a",
      });
      assert.okStatus(enable, "enable F5");
      const put = await kvCall(client, "deploy_a", "/put", { key: "f5-key", value: "written" });
      assert.equal(put.json?.ok, false, "F5 put fails after write");
      assert.equal(put.json?.message, "BINDING_INTERNAL", "F5 is tenant-safe");
      const disable = await client.fault({ target: "binding", point: "F5", enabled: false });
      assert.okStatus(disable, "disable F5");
      const got = await kvCall(client, "deploy_a", "/get", { key: "f5-key" });
      assert.equal(got.json.value, "written", "F5 write applied");
      classifications["B-fault-f5-applied"] = "applied";
    });

    await reporter.test("B-fault-isolation", async () => {
      const enable = await client.fault({
        target: "binding",
        point: "F4",
        enabled: true,
        resourceId: "kv_fixture_a",
      });
      assert.okStatus(enable, "enable F4 for isolation");
      const failA = await kvCall(client, "deploy_a", "/put", { key: "iso-a", value: "nope" });
      assert.equal(failA.json?.ok, false, "A put fails under F4");
      const b = await kvCall(client, "deploy_b", "/get", { key: "shared" });
      assert.okStatus(b, "B get during A fault");
      assert.equal(b.json.value, "B", "B still reads B");
      const unbound = await client.dispatch(workerAEnvelope());
      assert.okStatus(unbound, "unbound Worker A during A fault");
      assert.equal(unbound.json.deployment, "A", "Worker A still A");
      const disable = await client.fault({ target: "binding", point: "F4", enabled: false });
      assert.okStatus(disable, "disable F4 after isolation");
      const aShared = await kvCall(client, "deploy_a", "/get", { key: "shared" });
      assert.equal(aShared.json.value, "A", "A scope intact after fault");
    });

    await reporter.test("B-unbound-worker-a-unaffected", async () => {
      const res = await client.dispatch(workerAEnvelope());
      assert.okStatus(res, "unbound Worker A");
      assert.equal(res.json.deployment, "A", "deployment A");
      assert.equal(res.json.module, "mod-a", "module A");
      assert.equal(res.json.identity.deploymentId, "deploy_a", "env identity");
    });

    await reporter.test("B-sanitized-logs", async () => {
      const app = parseAppLogs(readStdout(proc));
      assert.isTrue(app.length > 0, "structured app logs present");
      const bindingLogs = app.filter((e) => e.bindingType === "FixtureKV");
      assert.isTrue(bindingLogs.length > 0, "binding logs present");
      for (const entry of bindingLogs) {
        assert.isTrue("resourceIdHash" in entry, "resourceIdHash present");
        assert.isTrue("bindingType" in entry, "bindingType present");
        assert.isTrue("outcome" in entry, "outcome present");
      }
      const appText = JSON.stringify(app);
      assertNoSecrets(appText, "app logs");
      assert.excludes(appText, fixturesDir, "logs have no fixture path");
      assert.excludes(appText, proc.dataDir, "logs have no data dir");
      assert.excludes(appText, "kv_fixture_a", "logs hash resource ids");
      assert.excludes(appText, "from-a", "logs omit tenant values");
      assert.excludes(appText, "clone-ok", "logs omit tenant bodies");
    });
  } catch (err) {
    proc.retainFailed(err);
    console.log(`failed          binding-suite`);
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
    console.log("G0.4: PASS");
  } else {
    console.log("G0.4: FAIL");
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
