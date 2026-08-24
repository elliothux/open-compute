"use strict";

const fs = require("node:fs");
const assert = require("../harness/assertions");
const { acquireWorkerd } = require("../harness/runtime");
const { WorkerdProcess, liveCount } = require("../harness/process-supervisor");

const CASE_NAMES = [
  "D06-process-restart",
  "D06-object-2-survives",
  "D06-supervisor-and-facet-recover",
  "D06-fresh-data-dir-empty",
  "D06-unwritable-data-dir-fail-closed",
  "D-crash-loop-seeded",
  "D-failAfterWrite-does-not-corrupt-other",
  "F6-transaction-before-commit",
  "F7-write-confirmed-response-failure",
  "F8-idle-sigkill",
  "F9-concurrent-sigkill",
  "F10-promote-without-abort",
  "F11-abort-before-get",
  "D07-code-promotion",
  "D08-rollback",
  "D09-explicit-delete",
  "no-leaked-workerd-child",
];

const ACCOUNT_ID = "acct_fixture";
const WORKER_ID = "worker_do";
const DEPLOYMENT_A = "deploy_a";
const DEPLOYMENT_B = "deploy_b";
const STORE = "store_alpha";
const CLASS_COUNTER = "Counter";
const OBJECT_1 = "obj_alpha";
const OBJECT_2 = "obj_beta";
const OBJECT_CRASH = "obj_crash";
const OBJECT_ERR = "obj_err";
const OBJECT_OK = "obj_ok";
const OBJECT_F6 = "obj_f6";
const OBJECT_F7 = "obj_f7";
const OBJECT_F9 = "obj_f9";
const OBJECT_LIFE = "obj_stable";
const SUPERVISOR_STAMP_KEY = "d06stamp";
const SUPERVISOR_STAMP_VALUE = "persist-v1";
const SEED = Number(process.env.G0_RECOVERY_SEED || 0x47300607) >>> 0;
const CRASH_CYCLES = 3;
const HOLD_MS = 4000;

class Reporter {
  constructor(required, options = {}) {
    this.required = required;
    this.results = [];
    this.quiet = Boolean(options.quiet);
  }

  async test(name, fn) {
    const started = Date.now();
    try {
      const extra = await fn();
      const row = { name, status: "passed", ms: Date.now() - started };
      if (extra && typeof extra === "object") {
        for (const [key, value] of Object.entries(extra)) {
          if (value !== undefined) row[key] = value;
        }
      }
      this.results.push(row);
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
          suite: "recovery",
          gates: ["G0.6", "G0.7"],
          seed: SEED,
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

function mulberry32(seed) {
  let a = seed >>> 0;
  return function next() {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function pidExists(pid) {
  if (pid == null) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (err) {
    if (err && err.code === "ESRCH") return false;
    throw err;
  }
}

function expectedFacetName(doStorageId, className, objectId) {
  return `v1/s/${doStorageId}/c/${className}/o/${objectId}`;
}

function target(overrides = {}) {
  return {
    accountId: ACCOUNT_ID,
    workerId: WORKER_ID,
    deploymentId: DEPLOYMENT_A,
    doStorageId: STORE,
    className: CLASS_COUNTER,
    objectId: OBJECT_1,
    ...overrides,
  };
}

function classifyInFlight({ clientCompleted, clientOk, before, after }) {
  if (!Number.isInteger(before) || !Number.isInteger(after)) return "result-unknown";
  if (clientCompleted && clientOk) {
    return after === before + 1 ? "applied" : "result-unknown";
  }
  if (after === before) return "not-applied";
  if (after === before + 1) return "applied";
  return "result-unknown";
}

function assertIntegerCounter(value, message) {
  assert.isTrue(Number.isInteger(value) && value >= 0, `${message}: non-negative integer`);
}

function watchRequest(promise) {
  const watched = Promise.resolve(promise).then(
    (res) => {
      watched.settled = true;
      return { completed: true, res, err: null };
    },
    (err) => {
      watched.settled = true;
      return { completed: true, res: null, err };
    }
  );
  watched.settled = false;
  return watched;
}

function isWatchedPending(watched) {
  return Boolean(watched) && watched.settled !== true;
}

async function settleWatched(watched, timeoutMs = 2000) {
  return Promise.race([
    watched,
    delay(timeoutMs).then(() => ({ completed: false, timeout: true, res: null, err: null })),
  ]);
}

class Session {
  constructor(proc) {
    this.proc = proc;
  }

  client() {
    return this.proc.client;
  }

  async doOp(body) {
    return this.client().doOp(body);
  }

  async increment(overrides = {}) {
    const res = await this.doOp({ op: "increment", ...target(overrides) });
    assert.okStatus(res, `increment ${JSON.stringify(overrides)}`);
    assert.equal(res.json.ok, true, "increment ok");
    return res;
  }

  async getValue(overrides = {}) {
    const res = await this.doOp({ op: "getValue", ...target(overrides) });
    assert.okStatus(res, `getValue ${JSON.stringify(overrides)}`);
    assert.equal(res.json.ok, true, "getValue ok");
    return res;
  }

  async abort(overrides = {}) {
    const res = await this.doOp({
      op: "abort",
      reason: "g0-code-restart",
      ...target(overrides),
    });
    assert.okStatus(res, `abort ${JSON.stringify(overrides)}`);
    assert.equal(res.json.ok, true, "abort ok");
    assert.equal(res.json.aborted, true, "aborted");
    return res;
  }

  async delete(overrides = {}) {
    const res = await this.doOp({ op: "delete", ...target(overrides) });
    assert.okStatus(res, `delete ${JSON.stringify(overrides)}`);
    assert.equal(res.json.ok, true, "delete ok");
    assert.equal(res.json.deleted, true, "deleted");
    return res;
  }

  async setFault(point, enabled) {
    const res = await this.client().fault({ target: "do", point, enabled: Boolean(enabled) });
    assert.okStatus(res, `fault ${point}=${enabled}`);
    return res;
  }

  async stampSupervisor() {
    const res = await this.doOp({
      op: "stampSupervisor",
      key: SUPERVISOR_STAMP_KEY,
      value: SUPERVISOR_STAMP_VALUE,
    });
    assert.okStatus(res, "stampSupervisor");
    assert.equal(res.json.ok, true, "stamp ok");
    assert.equal(res.json.value, SUPERVISOR_STAMP_VALUE, "stamp value");
    return res;
  }

  async probeSupervisor() {
    const res = await this.doOp({ op: "probeSupervisor" });
    assert.okStatus(res, "probeSupervisor");
    return res;
  }

  async stats() {
    const res = await this.doOp({ op: "stats" });
    assert.okStatus(res, "stats");
    return res;
  }

  async sigkillRestart() {
    const pid1 = this.proc.pid;
    assert.isTrue(this.proc.isAlive(), "pid1 alive before SIGKILL");
    const exit = await this.proc.kill("SIGKILL");
    assert.isTrue(exit != null, "SIGKILL exit observed");
    assert.equal(exit.signal, "SIGKILL", "exit signal is SIGKILL");
    assert.isFalse(this.proc.isAlive(), "killed child is not alive");
    assert.isFalse(pidExists(pid1), "killed pid is gone");
    await this.proc.start();
    const pid2 = this.proc.pid;
    assert.isTrue(pid2 != null && pid2 !== pid1, "PID 2 is a new pid");
    assert.isTrue(this.proc.isAlive(), "PID 2 alive");
    const health = await this.client().health();
    assert.okStatus(health, "health after restart");
    return { pid1, pid2, exit };
  }
}

async function run() {
  const reporter = new Reporter(CASE_NAMES);
  console.log("G0.6 crash/restart persistence / G0.7 facet lifecycle");
  console.log(`seed: ${SEED}`);

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
  const session = new Session(proc);
  const extras = [];
  const rng = mulberry32(SEED);
  let object1Nonce = null;
  let object1Facet = null;
  let object2Value = null;
  let lifeNonceA = null;
  let lifeFacet = null;
  let lifeNonceB = null;


  async function cleanupExtra(extraProc) {
    try {
      if (extraProc.isAlive()) await extraProc.kill("SIGKILL");
    } catch {
      /* ignore */
    }
    try {
      await extraProc.cleanupSuccess();
    } catch {
      /* ignore */
    }
  }

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

  try {
    await reporter.test("D06-process-restart", async () => {
      const pid1 = proc.pid;
      for (const expected of [1, 2, 3]) {
        const res = await session.increment();
        assert.equal(res.json.value, expected, `object-1 increment ${expected}`);
        assert.equal(res.json.codeVersion, "A", "codeVersion A");
        object1Facet = res.json.facetName;
        object1Nonce = res.json.jsNonce;
      }
      assert.equal(
        object1Facet,
        expectedFacetName(STORE, CLASS_COUNTER, OBJECT_1),
        "stable facet name"
      );
      const twoFirst = await session.increment({ objectId: OBJECT_2 });
      assert.equal(twoFirst.json.value, 1, "object-2 starts independent");
      notEqual(twoFirst.json.facetName, object1Facet, "object-2 has its own facet");
      const twoSecond = await session.increment({ objectId: OBJECT_2 });
      assert.equal(twoSecond.json.value, 2, "object-2 reaches 2");
      object2Value = 2;
      await session.stampSupervisor();
      const rpc = await session.getValue();
      assert.equal(rpc.json.value, 3, "RPC confirms 3 before kill");
      assert.equal(rpc.json.jsNonce, object1Nonce, "same JS instance before kill");
      const killed = await session.sigkillRestart();
      assert.equal(killed.pid1, pid1, "killed the original pid");
      const after = await session.getValue();
      assert.equal(after.json.value, 3, "RPC still 3 after PID replacement");
      assert.equal(after.json.facetName, object1Facet, "facet name unchanged across restart");
      assert.equal(after.json.codeVersion, "A", "codeVersion A after restart");
      notEqual(after.json.jsNonce, object1Nonce, "JS instance is new after restart");
      object1Nonce = after.json.jsNonce;
      const stats = await session.stats();
      const loaderKey = `${ACCOUNT_ID}/${WORKER_ID}/${DEPLOYMENT_A}`;
      assert.equal(stats.json.callbacks?.[loaderKey], 1, "cold-load callback after restart");
      const next = await session.increment();
      assert.equal(next.json.value, 4, "next increment is 4");
      assert.equal(next.json.jsNonce, object1Nonce, "same JS instance for next increment");
      return {
        classification: "applied",
        pid1: killed.pid1,
        pid2: killed.pid2,
        confirmedValue: 3,
        nextValue: 4,
      };
    });

    await reporter.test("D06-object-2-survives", async () => {
      const recovered2 = await session.getValue({ objectId: OBJECT_2 });
      assert.equal(recovered2.json.value, object2Value, "object-2 retains its value across the same restart");
      assert.equal(
        recovered2.json.facetName,
        expectedFacetName(STORE, CLASS_COUNTER, OBJECT_2),
        "object-2 facet name stable"
      );
      const recovered1 = await session.getValue();
      assert.equal(recovered1.json.value, 4, "object-1 still 4 on the recovered process");
      return { classification: "applied", object2Value: recovered2.json.value };
    });

    await reporter.test("D06-supervisor-and-facet-recover", async () => {
      const after = await session.probeSupervisor();
      assert.equal(after.json.ok, true, "supervisor probe ok after restart");
      assert.isTrue(
        (after.json.tables || []).includes("supervisor_meta"),
        "supervisor meta table recovered"
      );
      assert.isTrue(
        (after.json.tables || []).includes("supervisor_private"),
        "supervisor private table recovered"
      );
      assert.equal(after.json.privateValuePresent, true, "supervisor private SQL recovered");
      assert.equal(after.json.counterVisible, false, "facet counter still isolated");
      const recoveredStamp = (after.json.meta || []).some(
        (row) => row.k === SUPERVISOR_STAMP_KEY && row.v === SUPERVISOR_STAMP_VALUE
      );
      assert.isTrue(recoveredStamp, "supervisor stamp recovered");
      const facet = await session.getValue();
      assert.equal(facet.json.value, 4, "facet counter recovered");
      const facet2 = await session.getValue({ objectId: OBJECT_2 });
      assert.equal(facet2.json.value, object2Value, "object-2 recovered with supervisor");
      return { classification: "applied" };
    });

    await reporter.test("D06-fresh-data-dir-empty", async () => {
      const other = new WorkerdProcess({
        binPath: acquired.binPath,
        lock: acquired.lock,
      });
      extras.push(other);
      await other.start();
      const otherSession = new Session(other);
      const empty1 = await otherSession.getValue();
      assert.equal(empty1.json.value, 0, "fresh data dir object-1 is empty");
      const empty2 = await otherSession.getValue({ objectId: OBJECT_2 });
      assert.equal(empty2.json.value, 0, "fresh data dir object-2 is empty");
      const probe = await otherSession.probeSupervisor();
      const stamped = (probe.json.meta || []).some((row) => row.k === SUPERVISOR_STAMP_KEY);
      assert.isFalse(stamped, "fresh data dir has no recovered supervisor stamp");
      const original = await session.getValue();
      assert.equal(original.json.value, 4, "original data dir is unchanged");
      await other.stop("SIGTERM");
      await other.cleanupSuccess();
      return { classification: "not-applied" };
    });

    await reporter.test("D06-unwritable-data-dir-fail-closed", async () => {
      const blocked = new WorkerdProcess({
        binPath: acquired.binPath,
        lock: acquired.lock,
      });
      extras.push(blocked);
      blocked.prepareDirs();
      fs.chmodSync(blocked.dataDir, 0o500);
      try {
        const err = await assert.rejects(
          () => blocked.start(),
          /exited before listen|Permission denied/i,
          "unwritable data dir must fail closed"
        );
        assert.isFalse(blocked.isAlive(), "workerd must not remain running");
        assert.isTrue(blocked.exit != null, "unwritable dir must be an observed child exit");
        const logs = `${err.message}\n${blocked.readLogs()}`;
        assert.includes(logs, "Permission denied", "logs mention permission denied");
        assert.isFalse(blocked.client != null && blocked.isAlive(), "no in-memory serving child");
      } finally {
        try {
          fs.chmodSync(blocked.dataDir, 0o755);
        } catch {
          /* ignore */
        }
        if (blocked.isAlive()) await blocked.kill("SIGKILL");
        await blocked.cleanupSuccess();
      }
      const still = await session.getValue();
      assert.equal(still.json.value, 4, "writable original data dir still serves recovered value");
      return { classification: "runtime-unavailable" };
    });

    await reporter.test("D-crash-loop-seeded", async () => {
      await session.increment({ objectId: OBJECT_CRASH });
      const cycles = [];
      for (let i = 0; i < CRASH_CYCLES; i += 1) {
        const beforeRes = await session.getValue({ objectId: OBJECT_CRASH });
        const before = beforeRes.json.value;
        assertIntegerCounter(before, `cycle ${i} before`);
        const hold = rng() < 0.5 ? "before-write" : "after-write";
        const killDelay = 80 + Math.floor(rng() * 180);
        const inflight = watchRequest(
          session.doOp({
            op: "increment",
            hold,
            holdMs: HOLD_MS,
            ...target({ objectId: OBJECT_CRASH }),
          })
        );
        await delay(killDelay);
        await session.sigkillRestart();
        const settled = await settleWatched(inflight, 1500);
        const afterRes = await session.getValue({ objectId: OBJECT_CRASH });
        const after = afterRes.json.value;
        assertIntegerCounter(after, `cycle ${i} after`);
        assert.isTrue(after === before || after === before + 1, `cycle ${i} value is confirmed or next`);
        const classification = classifyInFlight({
          clientCompleted: Boolean(settled.completed && !settled.timeout && !settled.err),
          clientOk: Boolean(settled.res && settled.res.ok && settled.res.json && settled.res.json.ok),
          before,
          after,
        });
        const next = await session.increment({ objectId: OBJECT_CRASH });
        assert.equal(next.json.value, after + 1, `cycle ${i} database remains usable`);
        cycles.push({
          cycle: i,
          seed: SEED,
          hold,
          killDelayMs: killDelay,
          before,
          after,
          next: next.json.value,
          classification,
          clientCompleted: Boolean(settled.completed && !settled.timeout && !settled.err),
          clientOk: Boolean(settled.res && settled.res.ok && settled.res.json && settled.res.json.ok),
        });
      }
      return {
        seed: SEED,
        classification: "result-unknown",
        cycles,
        note: "in-flight crash is classified per cycle; not exactly-once",
      };
    });

    await reporter.test("D-failAfterWrite-does-not-corrupt-other", async () => {
      const ok1 = await session.increment({ objectId: OBJECT_OK });
      assert.equal(ok1.json.value, 1, "ok facet starts at 1");
      const errStart = await session.increment({ objectId: OBJECT_ERR });
      assert.equal(errStart.json.value, 1, "err facet starts at 1");
      const fail = await session.doOp({ op: "failAfterWrite", ...target({ objectId: OBJECT_ERR }) });
      assert.okStatus(fail, "failAfterWrite");
      assert.equal(fail.json.threw, true, "business error threw");
      assert.equal(fail.json.after, fail.json.before, "err facet rolled back");
      assert.equal(fail.json.classification, "not-applied", "business error not-applied");
      const errAfter = await session.getValue({ objectId: OBJECT_ERR });
      assert.equal(errAfter.json.value, 1, "err facet remains 1");
      const okAfter = await session.getValue({ objectId: OBJECT_OK });
      assert.equal(okAfter.json.value, 1, "ok facet unaffected by other facet error");
      await session.sigkillRestart();
      const errRecovered = await session.getValue({ objectId: OBJECT_ERR });
      const okRecovered = await session.getValue({ objectId: OBJECT_OK });
      assert.equal(errRecovered.json.value, 1, "err facet recovered");
      assert.equal(okRecovered.json.value, 1, "ok facet recovered independently");
      const okNext = await session.increment({ objectId: OBJECT_OK });
      assert.equal(okNext.json.value, 2, "ok facet still writable");
      return { classification: "not-applied" };
    });

    await reporter.test("F6-transaction-before-commit", async () => {
      const start = await session.increment({ objectId: OBJECT_F6 });
      assert.equal(start.json.value, 1, "F6 object starts at 1");
      const fail = await session.doOp({ op: "failAfterWrite", ...target({ objectId: OBJECT_F6 }) });
      assert.okStatus(fail, "F6 failAfterWrite");
      assert.equal(fail.json.classification, "not-applied", "throw before commit is not-applied");
      const afterFail = await session.getValue({ objectId: OBJECT_F6 });
      assert.equal(afterFail.json.value, 1, "value unchanged after business rollback");

      const before = afterFail.json.value;
      const inflight = watchRequest(
        session.doOp({
          op: "increment",
          hold: "before-commit",
          holdMs: HOLD_MS,
          ...target({ objectId: OBJECT_F6 }),
        })
      );
      await delay(120);
      await session.sigkillRestart();
      await settleWatched(inflight, 1500);
      const afterCrash = await session.getValue({ objectId: OBJECT_F6 });
      assert.equal(afterCrash.json.value, before, "crash before commit remains not-applied");
      const usable = await session.increment({ objectId: OBJECT_F6 });
      assert.equal(usable.json.value, before + 1, "F6 object remains usable");
      return {
        classification: "not-applied",
        fault: "F6",
        before,
        afterCrash: afterCrash.json.value,
      };
    });

    await reporter.test("F7-write-confirmed-response-failure", async () => {
      await session.setFault("F7", true);
      const before = await session.getValue({ objectId: OBJECT_F7 });
      assert.equal(before.json.value, 0, "F7 object starts empty");
      const failed = await session.doOp({ op: "increment", ...target({ objectId: OBJECT_F7 }) });
      assert.equal(failed.json?.ok, false, "F7 increment response failed");
      assert.equal(failed.json?.errorCode, "FAULT_INJECTED", "F7 classified as injected fault");
      await session.setFault("F7", false);
      const sameProcess = await session.getValue({ objectId: OBJECT_F7 });
      const observed = sameProcess.json.value;
      let classification = "result-unknown";
      if (observed === before.json.value + 1) classification = "applied";
      else if (observed === before.json.value) classification = "result-unknown";
      await session.sigkillRestart();
      const recovered = await session.getValue({ objectId: OBJECT_F7 });
      if (recovered.json.value === before.json.value + 1) classification = "applied";
      else if (recovered.json.value !== observed) classification = "result-unknown";
      assert.equal(
        recovered.json.value,
        observed,
        "post-restart value matches last API-observable value"
      );
      assert.isTrue(
        classification === "applied" || classification === "result-unknown",
        "F7 classified from API evidence only"
      );
      assert.equal(classification, "applied", "synced write before response failure is applied");
      const next = await session.increment({ objectId: OBJECT_F7 });
      assert.equal(next.json.value, recovered.json.value + 1, "F7 object remains usable");
      return {
        classification,
        fault: "F7",
        before: before.json.value,
        afterResponseFailure: observed,
        afterRestart: recovered.json.value,
      };
    });

    await reporter.test("F8-idle-sigkill", async () => {
      const idle = await session.getValue();
      const nonceBefore = idle.json.jsNonce;
      await delay(50);
      await session.sigkillRestart();
      const recovered = await session.getValue();
      assert.equal(recovered.json.value, idle.json.value, "idle SIGKILL preserves confirmed value");
      assert.equal(recovered.json.facetName, idle.json.facetName, "idle restart keeps facet name");
      notEqual(recovered.json.jsNonce, nonceBefore, "idle restart drops JS instance");
      object1Nonce = recovered.json.jsNonce;
      return { classification: "applied", fault: "F8", value: recovered.json.value };
    });

    await reporter.test("F9-concurrent-sigkill", async () => {
      const n = 8;
      const held = watchRequest(
        session.doOp({
          op: "increment",
          hold: "after-write",
          holdMs: HOLD_MS,
          ...target({ objectId: OBJECT_F9 }),
        })
      );
      const inflight = [
        held,
        ...Array.from({ length: n - 1 }, () =>
          watchRequest(session.doOp({ op: "increment", ...target({ objectId: OBJECT_F9 }) }))
        ),
      ];
      await delay(100);
      const pendingAtKill = inflight.filter(isWatchedPending).length;
      assert.isTrue(
        isWatchedPending(held),
        "F9 in-flight precondition: held increment must still be pending before SIGKILL"
      );
      assert.isTrue(
        pendingAtKill > 0,
        "F9 in-flight precondition: at least one issued request must remain pending before SIGKILL"
      );
      await session.sigkillRestart();
      await Promise.all(inflight.map((p) => settleWatched(p, 1500)));
      const recovered = await session.getValue({ objectId: OBJECT_F9 });
      const value = recovered.json.value;
      assertIntegerCounter(value, "F9 recovered counter");
      assert.isTrue(value <= n, "F9 recovered value cannot exceed issued increments");
      const next = await session.increment({ objectId: OBJECT_F9 });
      assert.equal(next.json.value, value + 1, "F9 recovered counter is usable");
      const fail = await session.doOp({ op: "failAfterWrite", ...target({ objectId: OBJECT_F9 }) });
      assert.equal(fail.json.after, fail.json.before, "F9 no corruption: rollback still works");
      const afterFail = await session.getValue({ objectId: OBJECT_F9 });
      assert.equal(afterFail.json.value, next.json.value, "F9 value stable after rollback");
      return {
        classification: value > 0 ? "applied" : "not-applied",
        fault: "F9",
        issued: n,
        pendingAtKill,
        recovered: value,
      };
    });

    await reporter.test("F10-promote-without-abort", async () => {
      for (const expected of [1, 2, 3]) {
        const res = await session.increment({ objectId: OBJECT_LIFE });
        assert.equal(res.json.value, expected, `lifecycle increment ${expected}`);
        lifeFacet = res.json.facetName;
        lifeNonceA = res.json.jsNonce;
      }
      assert.equal(
        lifeFacet,
        expectedFacetName(STORE, CLASS_COUNTER, OBJECT_LIFE),
        "lifecycle facet name"
      );
      await session.setFault("F10", true);
      const aborted = await session.doOp({
        op: "abort",
        reason: "g0-code-restart",
        ...target({ objectId: OBJECT_LIFE, deploymentId: DEPLOYMENT_B }),
      });
      assert.equal(aborted.json?.ok, false, "F10 abort did not complete");
      assert.equal(aborted.json?.errorCode, "FAULT_INJECTED", "F10 abort classified");
      await session.setFault("F10", false);
      const observed = await session.getValue({
        objectId: OBJECT_LIFE,
        deploymentId: DEPLOYMENT_B,
      });
      assert.equal(observed.json.value, 3, "storage value unchanged without abort");
      assert.equal(observed.json.facetName, lifeFacet, "facet name unchanged without abort");
      assert.equal(observed.json.codeVersion, "A", "old execution version still running");
      assert.equal(observed.json.jsNonce, lifeNonceA, "JS instance not restarted");
      assert.excludes(observed.json.facetName, DEPLOYMENT_B, "facet name has no deploymentId");
      return {
        classification: "applied",
        fault: "F10",
        window: {
          abortIssued: false,
          oldCodeVersion: "A",
          newExecutionTarget: "B",
          observedCodeVersion: observed.json.codeVersion,
          storageValue: observed.json.value,
          facetName: observed.json.facetName,
        },
      };
    });

    await reporter.test("F11-abort-before-get", async () => {
      const aborted = await session.abort({
        objectId: OBJECT_LIFE,
        deploymentId: DEPLOYMENT_B,
        reason: "g0-promote-b",
      });
      assert.equal(aborted.json.facetName, lifeFacet, "abort uses the same facet name");
      await session.setFault("F11", true);
      const blocked = await session.doOp({
        op: "getValue",
        ...target({ objectId: OBJECT_LIFE, deploymentId: DEPLOYMENT_B }),
      });
      assert.equal(blocked.json?.ok, false, "F11 next get did not complete");
      assert.equal(blocked.json?.errorCode, "FAULT_INJECTED", "F11 get classified");
      await session.setFault("F11", false);
      return {
        classification: "applied",
        fault: "F11",
        window: {
          abortIssued: true,
          nextGetIssued: false,
          oldCodeVersion: "A",
          newExecutionTarget: "B",
          observedCodeVersion: null,
          lastKnownStorageValue: 3,
          facetName: lifeFacet,
          note: "codeVersion after abort is not observed until the next get",
        },
      };
    });

    await reporter.test("D07-code-promotion", async () => {
      const promoted = await session.getValue({
        objectId: OBJECT_LIFE,
        deploymentId: DEPLOYMENT_B,
      });
      assert.equal(promoted.json.value, 3, "promotion preserves storage");
      assert.equal(promoted.json.codeVersion, "B", "new class is B");
      assert.equal(promoted.json.facetName, lifeFacet, "facet name/storage identity unchanged");
      notEqual(promoted.json.jsNonce, lifeNonceA, "new JS execution nonce after abort/get");
      assert.excludes(promoted.json.facetName, DEPLOYMENT_A, "facet name has no deployment A");
      assert.excludes(promoted.json.facetName, DEPLOYMENT_B, "facet name has no deployment B");
      lifeNonceB = promoted.json.jsNonce;
      return {
        classification: "applied",
        codeVersion: "B",
        value: 3,
        facetName: promoted.json.facetName,
        jsNonce: lifeNonceB,
      };
    });

    await reporter.test("D08-rollback", async () => {
      const aborted = await session.abort({
        objectId: OBJECT_LIFE,
        deploymentId: DEPLOYMENT_A,
        reason: "g0-rollback-a",
      });
      assert.equal(aborted.json.facetName, lifeFacet, "rollback abort keeps facet name");
      const rolled = await session.getValue({
        objectId: OBJECT_LIFE,
        deploymentId: DEPLOYMENT_A,
      });
      assert.equal(rolled.json.codeVersion, "A", "rollback loads class A");
      assert.equal(rolled.json.facetName, lifeFacet, "rollback keeps storage identity");
      assert.equal(rolled.json.value, 3, "rollback preserves SQLite value");
      notEqual(rolled.json.jsNonce, lifeNonceB, "rollback creates a new JS nonce");
      notEqual(rolled.json.jsNonce, lifeNonceA, "rollback nonce is not the original A instance");
      lifeNonceA = rolled.json.jsNonce;
      return {
        classification: "applied",
        codeVersion: "A",
        value: rolled.json.value,
        facetName: rolled.json.facetName,
      };
    });

    await reporter.test("D09-explicit-delete", async () => {
      const before = await session.getValue({ objectId: OBJECT_LIFE });
      assert.isTrue(before.json.value > 0, "delete starts from non-empty storage");
      const deleted = await session.delete({ objectId: OBJECT_LIFE });
      assert.equal(deleted.json.facetName, lifeFacet, "delete targets the same facet name");
      const after = await session.getValue({ objectId: OBJECT_LIFE });
      assert.equal(after.json.value, 0, "only delete resets storage to 0");
      assert.equal(after.json.facetName, lifeFacet, "recreated facet keeps the name");
      assert.equal(after.json.codeVersion, "A", "recreate uses current execution class");
      notEqual(after.json.jsNonce, before.json.jsNonce, "delete/get starts a new JS instance");
      const next = await session.increment({ objectId: OBJECT_LIFE });
      assert.equal(next.json.value, 1, "new storage starts at 1");
      return { classification: "applied", valueAfterDelete: 0 };
    });
  } catch (err) {
    proc.retainFailed(err);
    console.log(`failed          recovery-suite`);
    console.log(`  ${err && err.stack ? err.stack : err}`);
  } finally {
    for (const extra of extras) {
      await cleanupExtra(extra);
    }
    try {
      if (proc.isAlive()) await proc.stop("SIGTERM");
      if (reporter.results.some((row) => row.status === "failed")) {
        proc.retainFailed("recovery suite reported a failed case");
      } else {
        await proc.cleanupSuccess();
      }
    } catch (err) {
      try {
        proc.retainFailed(err);
        await proc.kill("SIGKILL");
      } catch {
        /* ignore */
      }
    }
  }

  await reporter.test("no-leaked-workerd-child", async () => {
    assert.equal(liveCount(), 0, "supervisor live child count");
  });

  const summary = reporter.summary();
  if (summary.failed === 0 && summary.notRun === 0) {
    console.log("G0.6: PASS");
    console.log("G0.7: PASS");
  } else {
    console.log("G0.6/G0.7: FAIL");
  }
  return summary.code;
}

function notEqual(actual, expected, message) {
  if (actual === expected) {
    throw new Error(`${message}: expected values to differ, both ${JSON.stringify(actual)}`);
  }
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
