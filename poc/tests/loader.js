"use strict";

const fs = require("node:fs");
const assert = require("../harness/assertions");
const { acquireWorkerd } = require("../harness/runtime");
const { WorkerdProcess, liveCount } = require("../harness/process-supervisor");
const { KEYS } = require("../harness/fixture-loader");

const CASE_NAMES = [
  "L01-cold-load-a",
  "L02-warm-a",
  "L03-coexist-a-b",
  "L04-promote-a-to-b",
  "L05-rollback-b-to-a",
  "L06-invalid-bundle",
  "L07-cold-concurrency",
  "L08-outbound-denied",
  "L-restart-cold-load",
  "L-invariant-key-reuse",
  "D-default-entrypoint",
  "D-named-entrypoint",
  "D-unknown-entrypoint",
  "D-unknown-kind",
  "D-scheduled-unimplemented",
  "D-queue-unimplemented",
  "D-workflow-unimplemented",
  "D-request-body",
  "D-response-stream",
  "D-abort",
  "D-identity-forgery",
  "D-active-route-ignores-body-deployment",
  "D-host-generated-request-id",
  "D-sanitized-logs-and-errors",
  "no-leaked-workerd-child",
];

const BODY_TOKEN = "g0-body-token-xyz";
const FORGED_REQUEST_ID = "tenant-forged-request-id";

function envelope(overrides = {}) {
  return {
    kind: "fetch",
    accountId: "acct_fixture",
    workerId: "worker_fixture",
    deploymentId: "deploy_a",
    entrypoint: null,
    ...overrides,
  };
}

function logicalRoute(deploymentId) {
  return {
    accountId: "acct_fixture",
    workerId: "worker_fixture",
    deploymentId,
  };
}

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
          suite: "loader",
          gates: ["G0.2", "G0.3"],
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

async function stats(client) {
  const res = await client.stats();
  assert.okStatus(res, "loader stats");
  return res.json;
}

function callbacks(json, key) {
  return json.callbacks?.[key] ?? 0;
}

async function expectTenantError(res, errorCode, message) {
  assert.equal(res.json?.ok, false, `${message}: ok`);
  assert.equal(res.json?.errorCode, errorCode, `${message}: errorCode`);
  assert.isTrue(res.json?.requestId != null && res.json.requestId !== "", `${message}: requestId`);
  const keys = Object.keys(res.json || {}).sort();
  assert.deepEqual(
    keys,
    ["deploymentId", "errorCode", "ok", "requestId"].sort(),
    `${message}: tenant error shape`
  );
}

async function withWorkerd(acquired, fn, options = {}) {
  const proc = new WorkerdProcess({
    binPath: acquired.binPath,
    lock: acquired.lock,
    ...options,
  });
  try {
    await proc.start();
    const result = await fn(proc);
    const exit = await proc.stop(options.stopSignal || "SIGTERM");
    await proc.cleanupSuccess();
    return { proc, result, exit };
  } catch (err) {
    const retained = proc.retainFailed(err);
    err.message = `${err.message} (retained ${retained})`;
    try {
      await proc.kill("SIGKILL");
    } catch {
      /* ignore */
    }
    throw err;
  }
}

async function waitForWorkerAbortEvents(client, baseline, timeoutMs = 2000) {
  const deadline = Date.now() + timeoutMs;
  let last = baseline;
  let lastRes = null;
  while (Date.now() < deadline) {
    lastRes = await client.dispatch(envelope({ url: "https://g0.invalid/abort-status" }));
    assert.okStatus(lastRes, "abort-status");
    assert.equal(lastRes.json?.deployment, "A", "abort-status is Worker A");
    last = lastRes.json?.abortEvents;
    if (typeof last === "number" && last > baseline) return last;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(
    `loaded Worker A never observed request.signal abort (abortEvents ${baseline} -> ${last})`
  );
}

async function run() {
  const reporter = new Reporter(CASE_NAMES);
  console.log("G0.2 workerLoader immutable deployment / G0.3 dispatch envelope");

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

  try {
    await reporter.test("L01-cold-load-a", async () => {
      const res = await client.dispatch(envelope());
      assert.okStatus(res, "cold A");
      assert.equal(res.json.deployment, "A", "deployment A");
      assert.equal(res.json.module, "mod-a", "module A");
      assert.equal(res.headers.get("x-g0-loader-outcome"), "cold", "cold outcome header");
      const st = await stats(client);
      assert.equal(callbacks(st, KEYS.A), 1, "callback invoked once for cold A");
      assert.equal(st.lastOutcome[KEYS.A], "cold", "stats cold");
    });

    await reporter.test("L02-warm-a", async () => {
      const res = await client.dispatch(envelope());
      assert.okStatus(res, "warm A");
      assert.equal(res.json.deployment, "A", "deployment A");
      assert.equal(res.json.module, "mod-a", "module A");
      assert.equal(res.headers.get("x-g0-loader-outcome"), "warm", "warm outcome header");
      const st = await stats(client);
      assert.equal(callbacks(st, KEYS.A), 1, "warm A does not invoke another callback");
      assert.equal(st.lastOutcome[KEYS.A], "warm", "stats warm");
    });

    await reporter.test("D-default-entrypoint", async () => {
      const res = await client.dispatch(envelope({ entrypoint: null }));
      assert.okStatus(res, "default entrypoint");
      assert.equal(res.json.entrypoint, "default", "default export");
      assert.equal(res.json.identity.deploymentId, "deploy_a", "env identity");
    });

    await reporter.test("D-named-entrypoint", async () => {
      const res = await client.dispatch(envelope({ entrypoint: "extra" }));
      assert.okStatus(res, "named extra");
      assert.equal(res.json.entrypoint, "extra", "named export");
      assert.equal(res.json.deployment, "A", "still A");
      assert.equal(res.json.identity.deploymentId, "deploy_a", "env identity");
    });

    await reporter.test("D-unknown-entrypoint", async () => {
      const res = await client.dispatch(envelope({ entrypoint: "nope" }));
      assert.equal(res.status, 404, "unknown entrypoint status");
      await expectTenantError(res, "ENTRYPOINT_NOT_FOUND", "unknown entrypoint");
      const still = await client.dispatch(envelope());
      assert.okStatus(still, "A after unknown entrypoint");
      assert.equal(still.json.deployment, "A", "A isolated");
    });

    await reporter.test("D-request-body", async () => {
      const res = await client.dispatch(
        envelope({
          url: "https://g0.invalid/body",
          method: "POST",
          body: BODY_TOKEN,
        })
      );
      assert.okStatus(res, "body dispatch");
      assert.equal(res.json.body, BODY_TOKEN, "inner body");
      assert.equal(res.json.deployment, "A", "body still A");
    });

    await reporter.test("D-response-stream", async () => {
      const res = await client.dispatch(envelope({ url: "https://g0.invalid/stream" }));
      assert.okStatus(res, "stream dispatch");
      assert.equal(res.text, "chunk-a-1chunk-a-2", "streamed chunks");
      assert.includes(res.headers.get("content-type") || "", "text/plain", "stream content-type");
    });

    await reporter.test("D-abort", async () => {
      const before = await client.dispatch(envelope({ url: "https://g0.invalid/abort-status" }));
      assert.okStatus(before, "abort-status before hang");
      assert.equal(before.json.deployment, "A", "abort-status is Worker A");
      const baseline = before.json.abortEvents;
      assert.equal(typeof baseline, "number", "abortEvents is a number");

      const ac = new AbortController();
      const started = Date.now();
      const pending = fetch(`${proc.baseUrl}/g0/dispatch`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(envelope({ url: "https://g0.invalid/hang" })),
        signal: ac.signal,
      });
      await new Promise((resolve) => setTimeout(resolve, 150));
      ac.abort();
      let aborted = false;
      try {
        const res = await pending;
        const text = await res.text();
        assert.isFalse(
          res.ok && text.includes('"hang":"timeout"'),
          "hang must not complete as a timeout after abort"
        );
        aborted = /aborted/i.test(text) || res.status >= 400;
      } catch (err) {
        const text = String(err && err.name ? err.name : err);
        assert.match(text, /AbortError|aborted/i, "client abort rejects");
        aborted = true;
      }
      assert.isTrue(aborted, "abort was observed");
      assert.isTrue(Date.now() - started < 5000, "abort did not wait out the hang timer");
      assert.isTrue(proc.isAlive(), "workerd alive after abort");
      const observed = await waitForWorkerAbortEvents(client, baseline);
      assert.isTrue(observed > baseline, "worker-side abort counter increased");
      const still = await client.dispatch(envelope());
      assert.okStatus(still, "A after abort");
      assert.equal(still.json.deployment, "A", "A after abort");
    });

    await reporter.test("D-identity-forgery", async () => {
      const res = await client.dispatch(
        envelope({ url: "https://g0.invalid/headers" }),
        {
          "x-account-id": "forged-acct",
          "x-deployment-id": "deploy_b",
          "x-worker-id": "forged-worker",
        }
      );
      assert.okStatus(res, "forged headers still dispatch");
      assert.equal(res.json.deployment, "A", "envelope identity wins");
      assert.equal(res.json.envDeployment, "deploy_a", "env identity not forged");
      assert.equal(res.json.accountHeader, null, "tenant account header stripped");
      assert.equal(res.json.deploymentHeader, null, "tenant deployment header stripped");

      const viaBodyHeaders = await client.dispatch(
        envelope({
          url: "https://g0.invalid/headers",
          headers: {
            "x-account-id": "forged-acct",
            "x-deployment-id": "deploy_b",
          },
        })
      );
      assert.okStatus(viaBodyHeaders, "forged body headers");
      assert.equal(viaBodyHeaders.json.deployment, "A", "body headers cannot switch deployment");
      assert.equal(viaBodyHeaders.json.envDeployment, "deploy_a", "env identity");
    });

    await reporter.test("D-host-generated-request-id", async () => {
      const res = await client.dispatch(envelope(), { "x-g0-request-id": FORGED_REQUEST_ID });
      assert.okStatus(res, "dispatch with forged request id");
      const used = res.headers.get("x-g0-request-id");
      assert.isTrue(used != null && used !== "", "host request id present");
      assert.isFalse(used === FORGED_REQUEST_ID, "tenant request id is not used");
    });

    await reporter.test("D-unknown-kind", async () => {
      const before = await stats(client);
      const res = await client.dispatch(envelope({ kind: "not-a-kind" }));
      assert.equal(res.status, 400, "unknown kind status");
      await expectTenantError(res, "DISPATCH_KIND_UNKNOWN", "unknown kind");
      const after = await stats(client);
      assert.equal(callbacks(after, KEYS.A), callbacks(before, KEYS.A), "unknown kind does not load");
    });

    await reporter.test("D-scheduled-unimplemented", async () => {
      const res = await client.dispatch(envelope({ kind: "scheduled" }));
      assert.equal(res.status, 400, "scheduled status");
      await expectTenantError(res, "DISPATCH_KIND_UNSUPPORTED", "scheduled");
    });

    await reporter.test("D-queue-unimplemented", async () => {
      const res = await client.dispatch(envelope({ kind: "queue" }));
      await expectTenantError(res, "DISPATCH_KIND_UNSUPPORTED", "queue");
    });

    await reporter.test("D-workflow-unimplemented", async () => {
      const res = await client.dispatch(envelope({ kind: "workflow" }));
      await expectTenantError(res, "DISPATCH_KIND_UNSUPPORTED", "workflow");
    });

    await reporter.test("L03-coexist-a-b", async () => {
      const b = await client.dispatch(envelope({ deploymentId: "deploy_b" }));
      assert.okStatus(b, "cold B");
      assert.equal(b.json.deployment, "B", "deployment B");
      assert.equal(b.json.module, "mod-b", "module B");
      assert.equal(b.headers.get("x-g0-loader-outcome"), "cold", "B cold");
      const a = await client.dispatch(envelope());
      assert.okStatus(a, "A after B");
      assert.equal(a.json.deployment, "A", "A still A");
      assert.equal(a.json.module, "mod-a", "A module unchanged");
      const st = await stats(client);
      assert.equal(callbacks(st, KEYS.A), 1, "A callback still 1");
      assert.equal(callbacks(st, KEYS.B), 1, "B callback 1");
    });

    await reporter.test("L04-promote-a-to-b", async () => {
      const setA = await client.route(logicalRoute("deploy_a"));
      assert.okStatus(setA, "route A");
      const activeA = await client.active(envelope({ deploymentId: "ignored" }));
      assert.okStatus(activeA, "active A");
      assert.equal(activeA.json.deployment, "A", "active A");
      const setB = await client.route(logicalRoute("deploy_b"));
      assert.okStatus(setB, "promote B");
      const activeB = await client.active(envelope({ deploymentId: "deploy_a" }));
      assert.okStatus(activeB, "active B after promote");
      assert.equal(activeB.json.deployment, "B", "new requests execute B");
      assert.equal(activeB.json.module, "mod-b", "B module");
      const explicitA = await client.dispatch(envelope());
      assert.equal(explicitA.json.deployment, "A", "A key remains callable");
      const st = await stats(client);
      assert.equal(callbacks(st, KEYS.B), 1, "promote does not reload B");
    });

    await reporter.test("L05-rollback-b-to-a", async () => {
      const setA = await client.route(logicalRoute("deploy_a"));
      assert.okStatus(setA, "rollback A");
      const activeA = await client.active(envelope({ deploymentId: "deploy_b" }));
      assert.okStatus(activeA, "active A after rollback");
      assert.equal(activeA.json.deployment, "A", "rollback executes A");
      assert.equal(activeA.json.module, "mod-a", "A module without retransmit");
      const st = await stats(client);
      assert.equal(callbacks(st, KEYS.A), 1, "rollback does not reload A");
    });

    await reporter.test("D-active-route-ignores-body-deployment", async () => {
      const res = await client.active({
        kind: "fetch",
        accountId: "acct_fixture",
        workerId: "worker_fixture",
        deploymentId: "deploy_b",
      });
      assert.okStatus(res, "active with forged deploymentId");
      assert.equal(res.json.deployment, "A", "route wins over body deploymentId");
    });

    await reporter.test("L06-invalid-bundle", async () => {
      const bad = await client.dispatch(envelope({ deploymentId: "deploy_bad_syntax" }));
      await expectTenantError(bad, "BUNDLE_INVALID", "bad-syntax");
      const missing = await client.dispatch(envelope({ deploymentId: "deploy_missing_module" }));
      await expectTenantError(missing, "BUNDLE_INVALID", "missing-module");
      const thrown = await client.dispatch(envelope({ deploymentId: "deploy_throw_startup" }));
      assert.equal(thrown.json?.ok, false, "throw-startup fails");
      assert.equal(thrown.json?.errorCode, "LOADER_ERROR", "throw-startup isolated error");
      await expectTenantError(thrown, "LOADER_ERROR", "throw-startup");
      const a = await client.dispatch(envelope());
      const b = await client.dispatch(envelope({ deploymentId: "deploy_b" }));
      assert.okStatus(a, "A after invalid");
      assert.okStatus(b, "B after invalid");
      assert.equal(a.json.deployment, "A", "A unpoisoned");
      assert.equal(b.json.deployment, "B", "B unpoisoned");
      assert.equal(a.json.module, "mod-a", "A module unpoisoned");
      assert.equal(b.json.module, "mod-b", "B module unpoisoned");
    });

    await reporter.test("L08-outbound-denied", async () => {
      const res = await client.dispatch({
        kind: "fetch",
        accountId: "acct_fixture",
        workerId: "worker_out",
        deploymentId: "deploy_a",
      });
      assert.okStatus(res, "outbound fixture responded");
      assert.equal(res.json.outbound, "denied", "globalOutbound null denies fetch");
      assert.isFalse(res.json.outbound === "unexpected-success", "network success is No-Go");
      assert.isTrue(res.json.status == null, "no HTTP success status from the network");
    });

    await reporter.test("L-invariant-key-reuse", async () => {
      const res = await client.invariant({ key: KEYS.A, alternateRoot: "worker-b" });
      assert.equal(res.status, 409, "invariant status");
      assert.equal(res.json.ok, false, "invariant ok");
      assert.equal(res.json.errorCode, "PLATFORM_INVARIANT_VIOLATION", "errorCode");
      assert.equal(
        res.json.classification,
        "platform-invariant-violation",
        "classification"
      );
      const a = await client.dispatch(envelope());
      assert.okStatus(a, "A after rejected remap");
      assert.equal(a.json.deployment, "A", "original A bytes still execute");
      assert.equal(a.json.module, "mod-a", "A module unchanged");
    });

    await reporter.test("D-sanitized-logs-and-errors", async () => {
      const app = parseAppLogs(readStdout(proc));
      assert.isTrue(app.length > 0, "structured app logs present");
      const dispatchLogs = app.filter((e) => e.dispatchKind === "fetch" && e.deploymentId === "deploy_a");
      assert.isTrue(dispatchLogs.length > 0, "fetch logs for A");
      for (const entry of dispatchLogs) {
        for (const field of [
          "timestamp",
          "requestId",
          "deploymentId",
          "loaderKeyHash",
          "loaderOutcome",
          "dispatchKind",
          "entrypoint",
          "durationMs",
          "outcome",
        ]) {
          assert.isTrue(field in entry, `log field ${field}`);
        }
        assert.isTrue("errorCode" in entry, "log field errorCode");
        assert.isTrue("workerdPid" in entry, "log field workerdPid");
      }
      const appText = JSON.stringify(app);
      for (const token of [
        BODY_TOKEN,
        "export default",
        "export const value",
        fixturesDir,
        proc.dataDir,
        "/Users/",
        "g0-master-key",
        FORGED_REQUEST_ID,
      ]) {
        assert.excludes(appText, token, `app logs must not contain ${token}`);
      }
      const unknown = await client.dispatch(envelope({ entrypoint: "nope" }));
      await expectTenantError(unknown, "ENTRYPOINT_NOT_FOUND", "sanitized unknown entrypoint");
      assert.excludes(unknown.text, "Ensure the worker exports", "no workerd hint");
      assert.excludes(unknown.text, fixturesDir, "no fixture path in tenant error");
      const keys = Object.keys(unknown.json);
      assert.isFalse(keys.includes("stack"), "no stack field");
      assert.isFalse(keys.includes("message"), "no raw message field");
    });

    await reporter.test("L-restart-cold-load", async () => {
      const firstPid = proc.pid;
      const { pid } = await proc.restart("SIGKILL");
      assert.isTrue(pid != null && pid !== firstPid, "new pid");
      const res = await proc.client.dispatch(envelope());
      assert.okStatus(res, "A after restart");
      assert.equal(res.json.deployment, "A", "same fixture");
      assert.equal(res.json.module, "mod-a", "same module");
      assert.equal(res.headers.get("x-g0-loader-outcome"), "cold", "cold after restart");
      const st = await stats(proc.client);
      assert.equal(callbacks(st, KEYS.A), 1, "callback runs again after restart");
    });
  } catch (err) {
    proc.retainFailed(err);
    console.log(`failed          loader-suite`);
    console.log(`  ${err && err.stack ? err.stack : err}`);
  } finally {
    try {
      if (proc.isAlive()) await proc.stop("SIGTERM");
      if (!proc.exit || proc.exit.code === 0 || proc.exit.signal) {
        /* keep failed dirs only when retainFailed was used */
      }
      await proc.cleanupSuccess();
    } catch {
      try {
        await proc.kill("SIGKILL");
      } catch {
        /* ignore */
      }
    }
  }

  await reporter.test("L07-cold-concurrency", async () => {
    await withWorkerd(acquired, async (coldProc) => {
      const payloads = [
        envelope(),
        envelope(),
        envelope({ url: "https://g0.invalid/body", method: "POST", body: "c1" }),
        envelope({ url: "https://g0.invalid/body", method: "POST", body: "c2" }),
      ];
      const results = await Promise.all(payloads.map((body) => coldProc.client.dispatch(body)));
      for (const res of results) {
        assert.okStatus(res, "concurrent A");
        assert.equal(res.json.deployment, "A", "concurrent identity A");
        assert.equal(res.json.module, "mod-a", "concurrent module A");
      }
      const st = await stats(coldProc.client);
      assert.equal(callbacks(st, KEYS.A), 1, "one callback for concurrent cold key");
      const outcomes = results.map((res) => res.headers.get("x-g0-loader-outcome"));
      assert.isTrue(outcomes.includes("cold"), "at least one cold");
      assert.isTrue(
        outcomes.every((o) => o === "cold" || o === "warm"),
        "only cold/warm outcomes"
      );
    });
  });

  await reporter.test("no-leaked-workerd-child", async () => {
    assert.equal(liveCount(), 0, "supervisor live child count");
  });

  const summary = reporter.summary();
  if (summary.failed === 0 && summary.notRun === 0) {
    console.log("G0.2/G0.3: PASS");
  } else {
    console.log("G0.2/G0.3: FAIL");
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
