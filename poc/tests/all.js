"use strict";

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawn } = require("node:child_process");

const G0_BIN = path.resolve(__dirname, "..", "g0");
const REPO_ROOT = path.resolve(__dirname, "..", "..");
const RESULTS_REL = "docs/implemented/g0-results.md";
const LOCK_REL = "poc/workerd.lock";
const REPORT_WRITE_FAIL = "failed to write docs/implemented/g0-results.md";

const SUITES = ["bootstrap", "loader", "binding", "durable-object", "recovery"];
const ROUND_SEEDS = [1194329607, 1194329608, 1194329609];
const ALLOWED_FAIL = { suite: "loader", name: "D-abort" };
const HARD_REPORT_IDS = [
  "L01",
  "L02",
  "L03",
  "L04",
  "L05",
  "L06",
  "L07",
  "L08",
  "B01",
  "B02",
  "B03",
  "D01",
  "D02",
  "D03",
  "D04",
  "D05",
  "D06",
  "D07",
  "D08",
  "D09",
  "R01",
];
const HARD_LABELS = {
  L01: "cold load A",
  L02: "warm A",
  L03: "coexist A/B",
  L04: "promote A to B",
  L05: "rollback B to A",
  L06: "invalid bundle",
  L07: "cold concurrency",
  L08: "outbound denied",
  B01: "resource isolation",
  B02: "forged scope",
  B03: "safe error",
  D01: "facet fetch",
  D02: "facet RPC",
  D03: "object isolation",
  D04: "storage isolation",
  D05: "transaction rollback",
  D06: "process restart",
  D07: "code promotion",
  D08: "rollback",
  D09: "explicit delete",
  R01: "repeated suite",
};
const FAULT_CASES = [
  "D-crash-loop-seeded",
  "F6-transaction-before-commit",
  "F7-write-confirmed-response-failure",
  "F8-idle-sigkill",
  "F9-concurrent-sigkill",
  "F10-promote-without-abort",
  "F11-abort-before-get",
];
const CLASSIFICATIONS = new Set([
  "not-applied",
  "applied",
  "result-unknown",
  "runtime-unavailable",
  "platform-invariant-violation",
]);
const WINDOW_KEYS = [
  "abortIssued",
  "nextGetIssued",
  "oldCodeVersion",
  "newExecutionTarget",
  "observedCodeVersion",
  "storageValue",
  "lastKnownStorageValue",
  "note",
];
const BANNED_REPORT_TOKENS = [
  "supervisor-only",
  "g0-master-key",
  "g0-body-token-xyz",
  "/Users/g0/secret.js",
  "secret.sqlite",
  "internal adapter failure",
];

const EXPECTED_CASES = {
  bootstrap: [
    "lock-version-checksum",
    "checksum-mismatch-before-spawn",
    "config-parses-with-pinned-binary",
    "invalid-config-nonzero",
    "port-collision-fail-closed",
    "unwritable-data-dir-fail-closed",
    "health-only-after-ready",
    "default-entrypoint",
    "named-entrypoint",
    "internal-paths-not-public",
    "handler-exception-contained",
    "sigterm-exits",
    "sigkill-observed",
    "restart-new-pid",
    "harness-exit-reaps-child",
    "no-leaked-workerd-child",
  ],
  loader: [
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
  ],
  binding: [
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
  ],
  "durable-object": [
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
  ],
  recovery: [
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
  ],
};

const HARD_MATRIX = [
  { id: "L01", suite: "loader", names: ["L01-cold-load-a"] },
  { id: "L02", suite: "loader", names: ["L02-warm-a"] },
  { id: "L03", suite: "loader", names: ["L03-coexist-a-b"] },
  { id: "L04", suite: "loader", names: ["L04-promote-a-to-b"] },
  { id: "L05", suite: "loader", names: ["L05-rollback-b-to-a"] },
  { id: "L06", suite: "loader", names: ["L06-invalid-bundle"] },
  { id: "L07", suite: "loader", names: ["L07-cold-concurrency"] },
  { id: "L08", suite: "loader", names: ["L08-outbound-denied"] },
  { id: "B01", suite: "binding", names: ["B01-resource-isolation"] },
  { id: "B02", suite: "binding", names: ["B02-forged-scope"] },
  { id: "B03", suite: "binding", names: ["B03-safe-error"] },
  { id: "D01", suite: "durable-object", names: ["D01-facet-fetch"] },
  { id: "D02", suite: "durable-object", names: ["D02-facet-rpc"] },
  { id: "D03", suite: "durable-object", names: ["D03-object-isolation"] },
  { id: "D04", suite: "durable-object", names: ["D04-storage-isolation"] },
  { id: "D05", suite: "durable-object", names: ["D05-transaction-rollback"] },
  {
    id: "D06",
    suite: "recovery",
    names: [
      "D06-process-restart",
      "D06-object-2-survives",
      "D06-supervisor-and-facet-recover",
      "D-failAfterWrite-does-not-corrupt-other",
      "F9-concurrent-sigkill",
      "no-leaked-workerd-child",
    ],
  },
  { id: "D07", suite: "recovery", names: ["D07-code-promotion"] },
  { id: "D08", suite: "recovery", names: ["D08-rollback"] },
  { id: "D09", suite: "recovery", names: ["D09-explicit-delete"] },
];

const KNOWN_STATUS = new Set(["passed", "failed", "not-run", "not-applicable"]);

let activeChild = null;
let shuttingDown = null;

function seedHex(seed) {
  return `0x${seed.toString(16)}`;
}

function replayCommand(seed) {
  return `G0_RECOVERY_SEED=${seed} ./poc/g0 test recovery`;
}

function seedRecord(round, seed) {
  return {
    round,
    seed,
    hex: seedHex(seed),
    replay: replayCommand(seed),
  };
}

function seedError(label, actual, expected) {
  if (!Number.isInteger(expected)) return `malformed: expected ${label}`;
  if (!Number.isInteger(actual)) return `malformed: ${label}`;
  if (actual !== expected) return `${label} mismatch: ${actual} != ${expected}`;
  return null;
}

function validateRecoverySeeds(summary, results, expectedSeed) {
  const fatals = [];
  const summaryErr = seedError("recovery seed", summary && summary.seed, expectedSeed);
  if (summaryErr) fatals.push(summaryErr);

  const crash = Array.isArray(results)
    ? results.find((row) => row && row.name === "D-crash-loop-seeded")
    : null;
  if (!crash) {
    fatals.push("malformed: D-crash-loop-seeded seed");
    return fatals;
  }

  const crashErr = seedError("D-crash-loop-seeded seed", crash.seed, expectedSeed);
  if (crashErr) fatals.push(crashErr);

  if (!Array.isArray(crash.cycles)) {
    fatals.push("malformed: D-crash-loop-seeded cycles");
    return fatals;
  }

  for (const [index, cycle] of crash.cycles.entries()) {
    const label = `D-crash-loop-seeded cycles[${index}].seed`;
    if (!cycle || typeof cycle !== "object" || Array.isArray(cycle)) {
      fatals.push(`malformed: ${label}`);
      continue;
    }
    const cycleErr = seedError(label, cycle.seed, expectedSeed);
    if (cycleErr) fatals.push(cycleErr);
  }
  return fatals;
}

function parseKnownAbortObservation(error) {
  if (typeof error !== "string") return null;
  const match = /^loaded Worker A never observed request.signal abort \(abortEvents (\d+) -> (\d+)\)$/.exec(
    error
  );
  if (!match) return null;
  const beforeAbortEvents = Number(match[1]);
  const afterAbortEvents = Number(match[2]);
  if (!Number.isInteger(beforeAbortEvents) || !Number.isInteger(afterAbortEvents)) return null;
  if (beforeAbortEvents < 0 || afterAbortEvents < 0) return null;
  if (beforeAbortEvents !== afterAbortEvents) return null;
  return { beforeAbortEvents, afterAbortEvents };
}

function mappingErrors() {
  const errors = [];
  const seen = new Set();
  const hardIds = HARD_MATRIX.map((entry) => entry.id);
  const expectedIds = HARD_REPORT_IDS.filter((id) => id !== "R01");
  if (hardIds.join(",") !== expectedIds.join(",")) {
    errors.push("hard matrix IDs are not L01-L08, B01-B03, D01-D09");
  }
  for (const entry of HARD_MATRIX) {
    const expected = EXPECTED_CASES[entry.suite];
    if (!expected) {
      errors.push(`hard ${entry.id}: unknown suite ${entry.suite}`);
      continue;
    }
    if (!Array.isArray(entry.names) || entry.names.length === 0) {
      errors.push(`hard ${entry.id}: empty case list`);
      continue;
    }
    for (const name of entry.names) {
      if (!expected.includes(name)) {
        errors.push(`hard ${entry.id}: ${entry.suite} missing ${name}`);
      }
      const key = `${entry.suite}\0${name}`;
      if (seen.has(key)) errors.push(`hard ${entry.id}: duplicate mapping ${entry.suite} ${name}`);
      seen.add(key);
    }
  }
  return errors;
}

function extractJsonObjects(text) {
  const objects = [];
  if (!text) return objects;
  let i = 0;
  while (i < text.length) {
    if (text[i] !== "{") {
      i += 1;
      continue;
    }
    let depth = 0;
    let inString = false;
    let escape = false;
    let j = i;
    for (; j < text.length; j += 1) {
      const c = text[j];
      if (inString) {
        if (escape) escape = false;
        else if (c === "\\") escape = true;
        else if (c === '"') inString = false;
        continue;
      }
      if (c === '"') inString = true;
      else if (c === "{") depth += 1;
      else if (c === "}") {
        depth -= 1;
        if (depth === 0) {
          try {
            objects.push(JSON.parse(text.slice(i, j + 1)));
          } catch {
            /* not a JSON object */
          }
          i = j + 1;
          break;
        }
      }
    }
    if (j >= text.length) break;
  }
  return objects;
}

function lastSuiteJson(stdout, suite) {
  let last = null;
  if (stdout) {
    for (const line of stdout.split(/\r?\n/)) {
      const trimmed = line.trim();
      if (!trimmed.startsWith("{") || !trimmed.endsWith("}")) continue;
      try {
        const obj = JSON.parse(trimmed);
        if (obj && typeof obj === "object" && !Array.isArray(obj) && obj.suite === suite) {
          last = obj;
        }
      } catch {
        /* ignore non-JSON lines */
      }
    }
  }
  if (last) return last;
  const matches = extractJsonObjects(stdout || "").filter(
    (obj) => obj && typeof obj === "object" && obj.suite === suite
  );
  return matches.length > 0 ? matches[matches.length - 1] : null;
}

function countByStatus(results, status) {
  return results.filter((row) => row.status === status).length;
}

function validateSummary(suite, summary, expectedSeed) {
  const fatals = [];
  if (!summary || typeof summary !== "object" || Array.isArray(summary)) {
    return { fatals: ["malformed: no JSON object"], results: [], counts: null };
  }
  if (summary.suite !== suite) fatals.push(`suite mismatch: ${summary.suite}`);
  if (!Array.isArray(summary.results)) {
    fatals.push("malformed: results is not an array");
    if (suite === "recovery") fatals.push(...validateRecoverySeeds(summary, [], expectedSeed));
    return { fatals, results: [], counts: null };
  }

  const results = [];
  const names = [];
  for (const [index, row] of summary.results.entries()) {
    if (!row || typeof row !== "object" || Array.isArray(row)) {
      fatals.push(`malformed: results[${index}] is not an object`);
      continue;
    }
    if (typeof row.name !== "string" || row.name.length === 0) {
      fatals.push(`malformed: results[${index}] missing name`);
      continue;
    }
    if (typeof row.status !== "string" || !KNOWN_STATUS.has(row.status)) {
      fatals.push(`malformed: ${row.name} status ${row.status}`);
    }
    names.push(row.name);
    results.push(row);
  }

  const seen = new Set();
  const duplicates = [];
  for (const name of names) {
    if (seen.has(name)) {
      if (!duplicates.includes(name)) duplicates.push(name);
    } else {
      seen.add(name);
    }
  }
  if (duplicates.length > 0) fatals.push(`duplicate: ${duplicates.join(", ")}`);

  const expected = EXPECTED_CASES[suite];
  const missing = expected.filter((name) => !seen.has(name));
  const extra = names.filter((name) => !expected.includes(name));
  if (missing.length > 0) fatals.push(`missing: ${missing.join(", ")}`);
  if (extra.length > 0) fatals.push(`extra: ${[...new Set(extra)].join(", ")}`);

  const passed = countByStatus(results, "passed");
  const failed = countByStatus(results, "failed");
  const notRun = countByStatus(results, "not-run");
  const notApplicable = countByStatus(results, "not-applicable");
  if (summary.passed !== passed) fatals.push(`passed count ${summary.passed} != ${passed}`);
  if (summary.failed !== failed) fatals.push(`failed count ${summary.failed} != ${failed}`);
  if (summary.notRun !== notRun) fatals.push(`notRun count ${summary.notRun} != ${notRun}`);
  if ("notApplicable" in summary && summary.notApplicable !== notApplicable) {
    fatals.push(`notApplicable count ${summary.notApplicable} != ${notApplicable}`);
  }
  if (passed + failed + notRun + notApplicable !== results.length) {
    fatals.push("status counts do not cover results");
  }
  if (notRun > 0) {
    const namesNotRun = results.filter((row) => row.status === "not-run").map((row) => row.name);
    fatals.push(`not-run: ${namesNotRun.join(", ")}`);
  }

  if (suite === "recovery") fatals.push(...validateRecoverySeeds(summary, results, expectedSeed));

  return {
    fatals,
    results,
    counts: { passed, failed, notRun, notApplicable },
  };
}

function resultByName(results, name) {
  return results.find((row) => row.name === name) || null;
}

function evaluateSuiteExit(suite, results, exitCode, signal) {
  const fatals = [];
  if (signal) fatals.push(`signaled ${signal}`);
  const failed = results.filter((row) => row.status === "failed");
  const otherNonPass = results.filter(
    (row) => row.status !== "passed" && !(suite === ALLOWED_FAIL.suite && row.name === ALLOWED_FAIL.name)
  );

  if (suite === "loader") {
    const abort = resultByName(results, ALLOWED_FAIL.name);
    const extraFails = failed.filter((row) => row.name !== ALLOWED_FAIL.name);
    if (otherNonPass.length > 0) {
      fatals.push(
        `non-pass: ${otherNonPass.map((row) => `${row.name}:${row.status}`).join(", ")}`
      );
    }
    if (extraFails.length > 0) {
      fatals.push(`unexpected fail: ${extraFails.map((row) => row.name).join(", ")}`);
    }
    if (!abort) fatals.push("D-abort missing");
    else if (abort.status === "failed") {
      if (exitCode !== 1) fatals.push(`D-abort failed but exit ${exitCode}`);
      if (!parseKnownAbortObservation(abort.error)) {
        fatals.push("D-abort error is not the known abort observation");
      }
    } else if (abort.status === "passed") {
      if (exitCode !== 0) fatals.push(`loader passed but exit ${exitCode}`);
    } else {
      fatals.push(`D-abort status ${abort.status}`);
    }
    return fatals;
  }

  if (failed.length > 0) fatals.push(`failed: ${failed.map((row) => row.name).join(", ")}`);
  if (otherNonPass.length > 0) {
    fatals.push(`non-pass: ${otherNonPass.map((row) => `${row.name}:${row.status}`).join(", ")}`);
  }
  if (exitCode !== 0) fatals.push(`exit ${exitCode}`);
  return fatals;
}

function evaluateHard(roundSuites) {
  const hard = {};
  let allPassed = true;
  for (const entry of HARD_MATRIX) {
    const suiteRun = roundSuites[entry.suite];
    const cases = [];
    let status = "passed";
    let pendingAtKill;
    if (!suiteRun || suiteRun.fatals.length > 0 || !suiteRun.results) {
      status = "failed";
    }
    for (const name of entry.names) {
      const row = suiteRun && suiteRun.results ? resultByName(suiteRun.results, name) : null;
      const caseStatus = row && row.status === "passed" ? "passed" : "failed";
      cases.push({ name, status: caseStatus });
      if (caseStatus !== "passed") status = "failed";
      if (entry.id === "D06" && name === "F9-concurrent-sigkill") {
        pendingAtKill = row && Number.isInteger(row.pendingAtKill) ? row.pendingAtKill : null;
        if (!(Number.isInteger(pendingAtKill) && pendingAtKill > 0)) status = "failed";
      }
    }
    const item = { status, cases: entry.names.slice() };
    if (entry.id === "D06") item.pendingAtKill = pendingAtKill == null ? null : pendingAtKill;
    hard[entry.id] = item;
    if (status !== "passed") allPassed = false;
  }
  return { hard, allPassed };
}

function requestShutdown(signal) {
  if (shuttingDown) return;
  shuttingDown = signal;
  if (activeChild) {
    try {
      activeChild.kill(signal);
    } catch {
      /* already gone */
    }
  }
}

function spawnSuite(suite, env) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, [G0_BIN, "test", suite], {
      cwd: REPO_ROOT,
      env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    activeChild = child;
    const stdoutChunks = [];
    const stderrChunks = [];
    let spawnErr = null;

    if (child.stdout) {
      child.stdout.on("data", (chunk) => {
        stdoutChunks.push(chunk);
        process.stdout.write(chunk);
      });
    }
    if (child.stderr) {
      child.stderr.on("data", (chunk) => {
        stderrChunks.push(chunk);
        process.stderr.write(chunk);
      });
    }
    child.on("error", (err) => {
      spawnErr = err;
    });
    child.on("close", (code, signal) => {
      if (activeChild === child) activeChild = null;
      resolve({
        exitCode: code,
        signal: signal || null,
        stdout: Buffer.concat(stdoutChunks).toString("utf8"),
        stderr: Buffer.concat(stderrChunks).toString("utf8"),
        spawnErr,
      });
    });

    if (shuttingDown) {
      try {
        child.kill(shuttingDown);
      } catch {
        /* ignore */
      }
    }
  });
}

function suitePublic(run) {
  return {
    exitCode: run.exitCode,
    signal: run.signal,
    passed: run.counts ? run.counts.passed : null,
    failed: run.counts ? run.counts.failed : null,
    notRun: run.counts ? run.counts.notRun : null,
    notApplicable: run.counts ? run.counts.notApplicable : null,
    fatals: run.fatals.slice(),
  };
}

function isAllowedNonPass(item) {
  return (
    item.suite === ALLOWED_FAIL.suite &&
    item.name === ALLOWED_FAIL.name &&
    item.status === "failed" &&
    Number.isInteger(item.beforeAbortEvents) &&
    item.beforeAbortEvents >= 0 &&
    item.beforeAbortEvents === item.afterAbortEvents
  );
}

function finalize(report, roundsInternal) {
  const reasons = [];
  const nonPasses = [];
  const abortEvidence = [];

  if (mappingErrors().length > 0) reasons.push(...mappingErrors());
  if (shuttingDown) reasons.push(`interrupted ${shuttingDown}`);
  if (roundsInternal.length !== ROUND_SEEDS.length) {
    reasons.push(`expected ${ROUND_SEEDS.length} rounds, ran ${roundsInternal.length}`);
  }

  let hardAllPassed = roundsInternal.length === ROUND_SEEDS.length;
  const r01Rounds = [];
  for (const round of roundsInternal) {
    r01Rounds.push({ round: round.round, status: round.hardPassed ? "passed" : "failed" });
    if (!round.hardPassed) hardAllPassed = false;
    for (const suite of SUITES) {
      const run = round.suites[suite];
      if (!run) {
        reasons.push(`round ${round.round} ${suite}: not-run`);
        nonPasses.push({
          round: round.round,
          suite,
          name: "*",
          status: "not-run",
        });
        continue;
      }
      if (run.fatals.length > 0) {
        for (const fatal of run.fatals) reasons.push(`round ${round.round} ${suite}: ${fatal}`);
      }
      for (const row of run.results || []) {
        if (row.status !== "passed") {
          const item = {
            round: round.round,
            suite,
            name: row.name,
            status: row.status,
          };
          if (suite === ALLOWED_FAIL.suite && row.name === ALLOWED_FAIL.name) {
            const observation = parseKnownAbortObservation(row.error);
            if (observation) {
              item.beforeAbortEvents = observation.beforeAbortEvents;
              item.afterAbortEvents = observation.afterAbortEvents;
              abortEvidence.push({
                round: round.round,
                suite,
                name: row.name,
                status: row.status,
                beforeAbortEvents: observation.beforeAbortEvents,
                afterAbortEvents: observation.afterAbortEvents,
              });
            }
          }
          nonPasses.push(item);
        }
      }
    }
  }

  report.R01 = {
    status: hardAllPassed ? "passed" : "failed",
    rounds: r01Rounds,
  };
  report.conditional = {
    allowlist: [`${ALLOWED_FAIL.suite}:${ALLOWED_FAIL.name}`],
    evidence: abortEvidence,
  };

  const disallowed = nonPasses.filter((item) => !isAllowedNonPass(item));
  if (!hardAllPassed) reasons.push("R01: hard matrix not passed in all 3 rounds");

  const complete = roundsInternal.length === ROUND_SEEDS.length && !shuttingDown;
  const clean = reasons.length === 0 && disallowed.length === 0 && complete && hardAllPassed;
  let verdict;
  if (!clean) verdict = "No-Go";
  else if (nonPasses.length === 0) verdict = "Go";
  else if (nonPasses.every(isAllowedNonPass)) verdict = "Conditional Go";
  else verdict = "No-Go";

  report.verdict = verdict;
  report.exitCode = verdict === "No-Go" ? 1 : 0;
  report.reasons = reasons;
  return report.exitCode;
}

function mdCell(value) {
  return String(value == null ? "" : value)
    .replace(/\|/g, "\\|")
    .replace(/\r?\n/g, " ");
}

function asClassification(value) {
  return typeof value === "string" && CLASSIFICATIONS.has(value) ? value : null;
}

function asInt(value) {
  return Number.isInteger(value) ? value : null;
}

function asBool(value) {
  return typeof value === "boolean" ? value : null;
}

function asShortString(value) {
  if (typeof value !== "string") return null;
  if (value.length === 0 || value.length > 200) return null;
  if (value.includes("\n") || value.includes("\r")) return null;
  if (value.includes(REPO_ROOT)) return null;
  if (/^\/(?:Users|var|tmp|private|home)\//.test(value)) return null;
  return value;
}

function formatScalar(value) {
  if (value == null) return "null";
  if (typeof value === "boolean" || typeof value === "number") return String(value);
  return asShortString(value) || "unavailable";
}

function whitelistWindow(window) {
  if (!window || typeof window !== "object" || Array.isArray(window)) return null;
  const out = {};
  for (const key of WINDOW_KEYS) {
    if (!(key in window)) continue;
    const raw = window[key];
    if (raw == null) {
      out[key] = null;
      continue;
    }
    if (typeof raw === "boolean") {
      const flag = asBool(raw);
      if (flag != null) out[key] = flag;
      continue;
    }
    if (typeof raw === "number") {
      const n = asInt(raw);
      if (n != null) out[key] = n;
      continue;
    }
    const text = asShortString(raw);
    if (text != null) out[key] = text;
  }
  return Object.keys(out).length > 0 ? out : null;
}

function whitelistFaultRow(row) {
  if (!row || typeof row !== "object" || Array.isArray(row)) return { status: "not-run" };
  const status = KNOWN_STATUS.has(row.status) ? row.status : "failed";
  const out = { status };
  const classification = asClassification(row.classification);
  if (classification) out.classification = classification;
  if (Array.isArray(row.cycles)) {
    out.cycles = row.cycles.map((cycle, index) => ({
      cycle: cycle && asInt(cycle.cycle) != null ? cycle.cycle : index,
      classification: cycle ? asClassification(cycle.classification) : null,
    }));
  }
  const pendingAtKill = asInt(row.pendingAtKill);
  if (pendingAtKill != null) out.pendingAtKill = pendingAtKill;
  if (row.window) {
    const window = whitelistWindow(row.window);
    if (window) out.window = window;
  }
  return out;
}

function recoveryRow(roundInternal, name) {
  const run = roundInternal && roundInternal.suites ? roundInternal.suites.recovery : null;
  if (!run || !Array.isArray(run.results) || run.results.length === 0) return null;
  return resultByName(run.results, name);
}

function hardEntry(id) {
  return HARD_MATRIX.find((entry) => entry.id === id) || null;
}

function mappedCaseNames(id) {
  if (id === "R01") return "all hard IDs L01-L08, B01-B03, D01-D09 across 3 rounds";
  const entry = hardEntry(id);
  return entry ? entry.names.join(", ") : "";
}

function roundHardCell(roundInternal, id) {
  if (!roundInternal) return "not-run";
  if (id === "R01") {
    if (!roundInternal.suites) return "not-run";
    const ran = SUITES.some((suite) => {
      const run = roundInternal.suites[suite];
      return run && Array.isArray(run.results) && run.results.length > 0;
    });
    if (!ran) return "not-run";
    return roundInternal.hardPassed ? "passed" : "failed";
  }
  const item = roundInternal.hard && roundInternal.hard[id];
  const entry = hardEntry(id);
  const suiteRun = entry && roundInternal.suites ? roundInternal.suites[entry.suite] : null;
  if (!suiteRun || (suiteRun.fatals && suiteRun.fatals.includes("not-run") && (!suiteRun.results || suiteRun.results.length === 0))) {
    return "not-run";
  }
  if (!item) return "not-run";
  if (item.status === "passed") return "passed";
  if (item.status === "not-run") return "not-run";
  return "failed";
}

function hardRowStatus(roundsInternal, id) {
  const cells = ROUND_SEEDS.map((_, index) => roundHardCell(roundsInternal[index], id));
  const final = cells.every((status) => status === "passed")
    ? "passed"
    : cells.every((status) => status === "not-run")
      ? "not-run"
      : "failed";
  return { cells, final };
}

function casesStatus(roundsInternal, suite, names) {
  if (roundsInternal.length !== ROUND_SEEDS.length) return "not-run";
  for (const round of roundsInternal) {
    const run = round.suites && round.suites[suite];
    if (!run || !Array.isArray(run.results) || run.results.length === 0) return "not-run";
    for (const name of names) {
      const row = resultByName(run.results, name);
      if (!row) return "not-run";
      if (row.status !== "passed") return "failed";
    }
  }
  return "passed";
}

function combineEvidence(statuses) {
  if (statuses.some((status) => status === "failed")) return "failed";
  if (statuses.some((status) => status === "not-run")) return "not-run";
  return "passed";
}

function evidenceWord(status) {
  if (status === "passed") return "Met";
  if (status === "not-run") return "Not evidenced";
  return "Not met";
}

function noGoWord(status, observedWhenFailed, notObserved) {
  if (status === "passed") return `Not observed. ${notObserved}`;
  if (status === "not-run") return "Not evidenced in this run.";
  return observedWhenFailed;
}

function loadPinnedLock() {
  const lock = JSON.parse(fs.readFileSync(path.join(REPO_ROOT, LOCK_REL), "utf8"));
  if (!lock || typeof lock !== "object" || Array.isArray(lock)) throw new Error("lock");
  return {
    release: asShortString(lock.release) || "unavailable",
    version: asShortString(lock.version) || "unavailable",
    versionOutput: asShortString(lock.versionOutput) || "unavailable",
    artifactUrl:
      lock.artifact && asShortString(lock.artifact.url) ? lock.artifact.url : "unavailable",
    archiveSha256:
      lock.artifact && asShortString(lock.artifact.sha256) ? lock.artifact.sha256 : "unavailable",
    binarySha256:
      (lock.binary && asShortString(lock.binary.sha256) ? lock.binary.sha256 : null) ||
      asShortString(lock.sha256) ||
      "unavailable",
    compatibilityDate: asShortString(lock.compatibilityDate) || "unavailable",
    processFlags: Array.isArray(lock.requiredProcessFlags)
      ? lock.requiredProcessFlags.map((flag) => asShortString(flag)).filter(Boolean)
      : [],
    compatibilityFlags: Array.isArray(lock.compatibilityFlags)
      ? lock.compatibilityFlags.map((flag) => asShortString(flag)).filter(Boolean)
      : [],
    releaseUrl:
      lock.upstream && asShortString(lock.upstream.releaseUrl)
        ? lock.upstream.releaseUrl
        : "unavailable",
  };
}

function formatFlags(flags) {
  return flags.length > 0 ? flags.map((flag) => `\`${flag}\``).join(", ") : "unavailable";
}

function formatWindow(window) {
  if (!window) return "unavailable";
  const parts = [];
  for (const key of WINDOW_KEYS) {
    if (key in window) parts.push(`${key}=${formatScalar(window[key])}`);
  }
  return parts.length > 0 ? parts.join("; ") : "unavailable";
}

function suiteCounts(run) {
  if (!run) {
    return { exit: "not-run", passed: "not-run", failed: "not-run", notRun: "not-run" };
  }
  if (run.fatals && run.fatals.includes("not-run") && (!run.results || run.results.length === 0)) {
    return { exit: "not-run", passed: "not-run", failed: "not-run", notRun: "not-run" };
  }
  const exit = run.signal ? run.signal : run.exitCode == null ? "not-run" : String(run.exitCode);
  return {
    exit,
    passed: run.counts && Number.isInteger(run.counts.passed) ? String(run.counts.passed) : "not-run",
    failed: run.counts && Number.isInteger(run.counts.failed) ? String(run.counts.failed) : "not-run",
    notRun: run.counts && Number.isInteger(run.counts.notRun) ? String(run.counts.notRun) : "not-run",
  };
}

function abortCell(report, roundsInternal, roundIndex) {
  const roundNo = roundIndex + 1;
  const evidence = Array.isArray(report.conditional && report.conditional.evidence)
    ? report.conditional.evidence.find((row) => row && row.round === roundNo)
    : null;
  if (
    evidence &&
    Number.isInteger(evidence.beforeAbortEvents) &&
    evidence.beforeAbortEvents === evidence.afterAbortEvents
  ) {
    return {
      status: "failed",
      counts: `${evidence.beforeAbortEvents} -> ${evidence.afterAbortEvents}`,
    };
  }
  const round = roundsInternal[roundIndex];
  if (!round || !round.suites || !round.suites.loader) return { status: "not-run", counts: "not-run" };
  const row = resultByName(round.suites.loader.results || [], ALLOWED_FAIL.name);
  if (!row) return { status: "not-run", counts: "not-run" };
  if (row.status === "passed") return { status: "passed", counts: "not applicable" };
  const parsed = parseKnownAbortObservation(row.error);
  if (parsed) return { status: "failed", counts: `${parsed.beforeAbortEvents} -> ${parsed.afterAbortEvents}` };
  return { status: row.status === "not-run" ? "not-run" : "failed", counts: "unavailable" };
}

function assertReportSafe(markdown) {
  if (markdown.includes(REPO_ROOT)) throw new Error("unsafe repository path");
  for (const [index, token] of BANNED_REPORT_TOKENS.entries()) {
    if (markdown.includes(token)) throw new Error(`unsafe banned token ${index}`);
  }
  if (/(^|\n)\s+at \S+/.test(markdown)) throw new Error("unsafe stack trace");
  if (/\bWAL\b/.test(markdown) && !markdown.includes("no direct SQLite/WAL inspection")) {
    throw new Error("unsafe WAL detail");
  }
}

function renderResultsMarkdown(report, roundsInternal) {
  const lock = loadPinnedLock();
  const generatedAt = new Date().toISOString();
  const lines = [];
  const completed = roundsInternal.length;
  lines.push("# G0 results");
  lines.push("");
  lines.push(`- Generated: ${mdCell(generatedAt)}`);
  lines.push(`- Hostname: ${mdCell(os.hostname())}`);
  lines.push(`- OS: ${mdCell(os.platform())} ${mdCell(os.release())} ${mdCell(os.arch())}`);
  lines.push(`- Node: ${mdCell(process.version)}`);
  lines.push(`- Command: \`./poc/g0 test all\``);
  lines.push(
    `- Rounds: exactly 3 sequential fresh-process rounds; this run completed ${completed} of 3`
  );
  lines.push("");
  lines.push("## Pinned workerd");
  lines.push("");
  lines.push(`- Release: \`${mdCell(lock.release)}\``);
  lines.push(`- Version: \`${mdCell(lock.version)}\``);
  lines.push(`- Version output: \`${mdCell(lock.versionOutput)}\``);
  lines.push(`- Artifact URL: ${mdCell(lock.artifactUrl)}`);
  lines.push(`- Archive SHA256: \`${mdCell(lock.archiveSha256)}\``);
  lines.push(`- Binary SHA256: \`${mdCell(lock.binarySha256)}\``);
  lines.push(`- Compatibility date: \`${mdCell(lock.compatibilityDate)}\``);
  lines.push(`- Process flags: ${formatFlags(lock.processFlags)}`);
  lines.push(`- Compatibility flags: ${formatFlags(lock.compatibilityFlags)}`);
  lines.push(`- Release URL: ${mdCell(lock.releaseUrl)}`);
  lines.push("");
  lines.push("## Rounds");
  lines.push("");
  for (let i = 0; i < ROUND_SEEDS.length; i += 1) {
    const round = roundsInternal[i];
    const seed = round ? round.seed : ROUND_SEEDS[i];
    const hex = round ? round.hex : seedHex(ROUND_SEEDS[i]);
    const replay = round ? round.replay : replayCommand(ROUND_SEEDS[i]);
    lines.push(`### Round ${i + 1}`);
    lines.push("");
    lines.push(`- Status: ${round ? "ran" : "not-run"}`);
    lines.push(`- Recovery seed: ${seed} (\`${hex}\`)`);
    lines.push(`- Replay: \`${mdCell(replay)}\``);
    lines.push("");
    lines.push("| suite | exit | passed | failed | not-run |");
    lines.push("| --- | --- | --- | --- | --- |");
    for (const suite of SUITES) {
      const counts = suiteCounts(round && round.suites ? round.suites[suite] : null);
      lines.push(
        `| ${suite} | ${mdCell(counts.exit)} | ${mdCell(counts.passed)} | ${mdCell(counts.failed)} | ${mdCell(counts.notRun)} |`
      );
    }
    lines.push("");
  }

  lines.push("## Hard matrix");
  lines.push("");
  lines.push("| ID | case | mapped names | R1 | R2 | R3 | final |");
  lines.push("| --- | --- | --- | --- | --- | --- | --- |");
  for (const id of HARD_REPORT_IDS) {
    const row = hardRowStatus(roundsInternal, id);
    lines.push(
      `| ${id} | ${mdCell(HARD_LABELS[id])} | ${mdCell(mappedCaseNames(id))} | ${row.cells[0]} | ${row.cells[1]} | ${row.cells[2]} | ${row.final} |`
    );
  }
  lines.push("");

  lines.push("## Fault evidence");
  lines.push("");
  lines.push("Whitelisted recovery fields only. Missing cases are `not-run`.");
  lines.push("");
  lines.push("| round | case | status | classification | cycles | pendingAtKill | window |");
  lines.push("| --- | --- | --- | --- | --- | --- | --- |");
  for (let i = 0; i < ROUND_SEEDS.length; i += 1) {
    for (const name of FAULT_CASES) {
      const raw = recoveryRow(roundsInternal[i], name);
      if (!raw) {
        lines.push(
          `| ${i + 1} | ${name} | not-run | not-run | not-run | not-run | not-run |`
        );
        continue;
      }
      const row = whitelistFaultRow(raw);
      const cycles = Array.isArray(row.cycles)
        ? row.cycles
            .map((cycle) => `${cycle.cycle}:${cycle.classification || "unavailable"}`)
            .join(", ")
        : "n/a";
      const pending = name === "F9-concurrent-sigkill"
        ? row.pendingAtKill == null
          ? "unavailable"
          : String(row.pendingAtKill)
        : "n/a";
      const window =
        name === "F10-promote-without-abort" || name === "F11-abort-before-get"
          ? formatWindow(row.window)
          : "n/a";
      lines.push(
        `| ${i + 1} | ${name} | ${row.status} | ${row.classification || "unavailable"} | ${mdCell(cycles)} | ${mdCell(pending)} | ${mdCell(window)} |`
      );
    }
  }
  lines.push("");

  lines.push("## Conditional evidence (D-abort)");
  lines.push("");
  lines.push("Parsed `abortEvents` counts only; raw error text is omitted.");
  lines.push("");
  lines.push("| round | status | abortEvents |");
  lines.push("| --- | --- | --- |");
  for (let i = 0; i < ROUND_SEEDS.length; i += 1) {
    const cell = abortCell(report, roundsInternal, i);
    lines.push(`| ${i + 1} | ${cell.status} | ${mdCell(cell.counts)} |`);
  }
  lines.push("");

  lines.push("## Accepted limitations / risk register");
  lines.push("");
  lines.push(
    "- Client disconnect does not abort the loaded worker `request.signal` on this pinned stock workerd (`D-abort`)."
  );
  lines.push(
    "- `localDisk` is experimental; it is version-bound to the pinned workerd release and needs forward-only upgrade planning."
  );
  lines.push(
    "- An in-flight write may be `result-unknown`; this suite does not claim exactly-once."
  );
  lines.push(
    "- No alarm, WebSocket hibernation, Durable Object migration, or cross-node relocation validation."
  );
  lines.push("");

  const go = [
    {
      n: 1,
      title: "Stock, pinned workerd, no source patch",
      status: casesStatus(roundsInternal, "bootstrap", [
        "lock-version-checksum",
        "checksum-mismatch-before-spawn",
        "config-parses-with-pinned-binary",
      ]),
      evidence:
        "bootstrap pin/checksum/config cases; official artifact URL from poc/workerd.lock; harness starts that binary",
    },
    {
      n: 2,
      title: "One workerd process hosts the required static host services",
      status: casesStatus(roundsInternal, "bootstrap", [
        "health-only-after-ready",
        "default-entrypoint",
        "named-entrypoint",
        "internal-paths-not-public",
      ]),
      evidence: "bootstrap health, default/named entrypoints, internal paths not public",
    },
    {
      n: 3,
      title: "workerLoader loads, caches, and isolates immutable A/B keys",
      status: hardRowStatus(roundsInternal, "L03").final === "passed" &&
        hardRowStatus(roundsInternal, "L01").final === "passed" &&
        hardRowStatus(roundsInternal, "L02").final === "passed"
        ? "passed"
        : combineEvidence([
            hardRowStatus(roundsInternal, "L01").final,
            hardRowStatus(roundsInternal, "L02").final,
            hardRowStatus(roundsInternal, "L03").final,
          ]),
      evidence: "L01 cold load A, L02 warm A, L03 coexist A/B",
    },
    {
      n: 4,
      title: "Promotion/rollback do not overwrite bundles or invalidate cache",
      status: combineEvidence([
        hardRowStatus(roundsInternal, "L04").final,
        hardRowStatus(roundsInternal, "L05").final,
      ]),
      evidence: "L04 promote A to B, L05 rollback B to A",
    },
    {
      n: 5,
      title: "Loaded Worker can access only binding-scoped capability",
      status: combineEvidence([
        hardRowStatus(roundsInternal, "B01").final,
        hardRowStatus(roundsInternal, "B02").final,
        hardRowStatus(roundsInternal, "B03").final,
      ]),
      evidence: "B01 resource isolation, B02 forged scope, B03 safe error",
    },
    {
      n: 6,
      title: "Dynamic DO class executes fetch, RPC, and SQLite through native facets",
      status: combineEvidence([
        hardRowStatus(roundsInternal, "D01").final,
        hardRowStatus(roundsInternal, "D02").final,
        hardRowStatus(roundsInternal, "D05").final,
      ]),
      evidence: "D01 facet fetch, D02 facet RPC, D05 transaction rollback",
    },
    {
      n: 7,
      title: "Supervisor and facet storage are isolated",
      status: hardRowStatus(roundsInternal, "D04").final,
      evidence: "D04 storage isolation",
    },
    {
      n: 8,
      title: "Confirmed DO writes survive SIGKILL/restart",
      status: hardRowStatus(roundsInternal, "D06").final,
      evidence: "D06 process restart and mapped recovery cases",
    },
    {
      n: 9,
      title: "abort() changes code and keeps storage; only delete() drops storage",
      status: combineEvidence([
        hardRowStatus(roundsInternal, "D07").final,
        hardRowStatus(roundsInternal, "D08").final,
        hardRowStatus(roundsInternal, "D09").final,
      ]),
      evidence: "D07 code promotion, D08 rollback, D09 explicit delete",
    },
    {
      n: 10,
      title: "Suite repeats unattended for three rounds",
      status: hardRowStatus(roundsInternal, "R01").final,
      evidence: "R01: hard matrix passed in all 3 sequential fresh-process rounds",
    },
  ];

  lines.push("## Hard Go conditions");
  lines.push("");
  lines.push("| # | condition | evidence | result |");
  lines.push("| --- | --- | --- | --- |");
  for (const item of go) {
    lines.push(
      `| ${item.n} | ${mdCell(item.title)} | ${mdCell(item.evidence)} | ${evidenceWord(item.status)} |`
    );
  }
  lines.push("");

  const noGo = [
    {
      title: "Core path requires fork/patch workerd",
      text: noGoWord(
        go[0].status,
        "Unresolved: pin/checksum evidence did not pass.",
        "This run used the pinned official artifact and self-owned config."
      ),
    },
    {
      title: "Loader key cannot keep immutable A/B loaded together",
      text: noGoWord(
        hardRowStatus(roundsInternal, "L03").final,
        "Unresolved: L03 did not pass.",
        "L03 coexisted A and B."
      ),
    },
    {
      title: "Loaded Worker must hold a generic backend credential/Fetcher",
      text: noGoWord(
        combineEvidence([
          hardRowStatus(roundsInternal, "B01").final,
          hardRowStatus(roundsInternal, "B02").final,
        ]),
        "Unresolved: binding isolation cases did not pass.",
        "B01/B02 kept access binding-scoped."
      ),
    },
    {
      title: "Tenant can change props or choose another resource",
      text: noGoWord(
        hardRowStatus(roundsInternal, "B02").final,
        "Unresolved: B02 did not pass.",
        "B02 rejected forged scope."
      ),
    },
    {
      title: "Dynamic DO can only be simulated with an ordinary adapter",
      text: noGoWord(
        combineEvidence([
          hardRowStatus(roundsInternal, "D01").final,
          hardRowStatus(roundsInternal, "D02").final,
          hardRowStatus(roundsInternal, "D05").final,
        ]),
        "Unresolved: native facet cases did not pass.",
        "D01/D02/D05 used native facets."
      ),
    },
    {
      title: "Facet storage identity must include deployment ID",
      text: noGoWord(
        combineEvidence([
          hardRowStatus(roundsInternal, "D07").final,
          hardRowStatus(roundsInternal, "D08").final,
        ]),
        "Unresolved: promotion/rollback cases did not pass.",
        "D07/D08 kept storage identity across abort/get."
      ),
    },
    {
      title: "Code promotion only works by deleting SQLite",
      text: noGoWord(
        combineEvidence([
          hardRowStatus(roundsInternal, "D07").final,
          hardRowStatus(roundsInternal, "D08").final,
        ]),
        "Unresolved: D07/D08 did not pass.",
        "abort/get preserved storage; only D09 delete reset it."
      ),
    },
    {
      title: "Normal restart loses confirmed DO writes",
      text: noGoWord(
        hardRowStatus(roundsInternal, "D06").final,
        "Unresolved: D06 did not pass.",
        "D06 recovered confirmed writes after SIGKILL."
      ),
    },
    {
      title: "A malformed bundle/facet stably corrupts other tenant data",
      text: noGoWord(
        hardRowStatus(roundsInternal, "L06").final,
        "Unresolved: L06 did not pass.",
        "L06 failed closed without taking A/B down."
      ),
    },
    {
      title: "localDisk version/recovery risk cannot be controlled by pin and release migration",
      text:
        go[0].status === "passed"
          ? "Not observed as a Hard No-Go. localDisk remains experimental and is version-pinned with forward-only upgrade planning."
          : go[0].status === "not-run"
            ? "Not evidenced in this run."
            : "Unresolved: pin/checksum evidence did not pass.",
    },
  ];

  lines.push("## Hard No-Go conditions");
  lines.push("");
  lines.push("| condition | evaluation |");
  lines.push("| --- | --- |");
  for (const item of noGo) {
    lines.push(`| ${mdCell(item.title)} | ${mdCell(item.text)} |`);
  }
  lines.push("");

  lines.push("## Verdict");
  lines.push("");
  lines.push(`**${mdCell(report.verdict)}** (exit ${report.exitCode}).`);
  lines.push("");
  if (report.verdict === "Conditional Go") {
    lines.push(
      "All hard matrix IDs L01-L08, B01-B03, D01-D09, and R01 passed in all 3 rounds. The only allowlisted non-pass is loader `D-abort`, with parsed abortEvents equal before and after in each completed round. That client-disconnect limitation is accepted and does not flip the run to No-Go."
    );
  } else if (report.verdict === "Go") {
    lines.push("All required cases passed in all 3 rounds, including loader `D-abort`.");
  } else {
    const reasons = Array.isArray(report.reasons) ? report.reasons.filter((row) => typeof row === "string") : [];
    lines.push(
      reasons.length > 0
        ? `No-Go because: ${mdCell(reasons.join("; "))}.`
        : "No-Go because required evidence was missing or a disallowed failure was observed."
    );
  }
  lines.push("");
  lines.push(
    "This used pinned stock Cloudflare workerd, self-owned config, native workerLoader/JSRPC/facets/localDisk, no workerd patch, no Miniflare API/mock workerd, and no direct SQLite/WAL inspection."
  );
  lines.push("");
  return `${lines.join("\n")}\n`;
}

function writeResultsFile(report, roundsInternal) {
  const markdown = renderResultsMarkdown(report, roundsInternal);
  assertReportSafe(markdown);
  const dest = path.join(REPO_ROOT, RESULTS_REL);
  const dir = path.dirname(dest);
  fs.mkdirSync(dir, { recursive: true });
  const tmp = path.join(dir, "g0-results.md.tmp");
  try {
    fs.writeFileSync(tmp, markdown, { encoding: "utf8" });
    fs.renameSync(tmp, dest);
  } catch (err) {
    try {
      fs.rmSync(tmp, { force: true });
    } catch {
      /* ignore */
    }
    throw err;
  }
}

async function runRound(round, seed) {
  const env = { ...process.env, G0_RECOVERY_SEED: String(seed) };
  const record = seedRecord(round, seed);
  console.log(`round ${round}/${ROUND_SEEDS.length} seed ${record.seed} (${record.hex})`);
  console.log(`replay: ${record.replay}`);

  const suites = {};
  for (const suite of SUITES) {
    if (shuttingDown) {
      suites[suite] = {
        exitCode: null,
        signal: shuttingDown,
        results: [],
        counts: null,
        fatals: ["not-run"],
      };
      continue;
    }
    console.log(`${suite}`);
    const child = await spawnSuite(suite, env);
    const fatals = [];
    if (child.spawnErr) fatals.push(`spawn failed: ${child.spawnErr.code || "error"}`);
    const summary = lastSuiteJson(child.stdout, suite);
    if (!summary) fatals.push("malformed: missing suite JSON");
    const checked = validateSummary(suite, summary, seed);
    fatals.push(...checked.fatals);
    if (fatals.length === 0) {
      fatals.push(...evaluateSuiteExit(suite, checked.results, child.exitCode, child.signal));
    } else if (child.signal) {
      fatals.push(`signaled ${child.signal}`);
    }
    suites[suite] = {
      exitCode: child.exitCode,
      signal: child.signal,
      results: checked.results,
      counts: checked.counts,
      fatals,
    };
    const counts = checked.counts;
    console.log(
      `${suite} exit ${child.signal ? child.signal : child.exitCode} passed=${counts ? counts.passed : "?"} failed=${counts ? counts.failed : "?"}`
    );
  }

  const { hard, allPassed } = evaluateHard(suites);
  return {
    round,
    seed: record.seed,
    hex: record.hex,
    replay: record.replay,
    suites,
    hard,
    hardPassed: allPassed,
  };
}

async function run() {
  process.on("SIGINT", () => requestShutdown("SIGINT"));
  process.on("SIGTERM", () => requestShutdown("SIGTERM"));

  console.log("G0.8 three-run regression");
  const mapErrs = mappingErrors();
  const roundsInternal = [];
  const report = {
    suite: "all",
    gates: ["G0.8"],
    seeds: ROUND_SEEDS.map((seed, index) => seedRecord(index + 1, seed)),
    rounds: [],
    R01: null,
    conditional: null,
    verdict: "No-Go",
    exitCode: 1,
    reasons: mapErrs.slice(),
  };

  if (mapErrs.length === 0) {
    for (let i = 0; i < ROUND_SEEDS.length; i += 1) {
      if (shuttingDown) break;
      const round = await runRound(i + 1, ROUND_SEEDS[i]);
      roundsInternal.push(round);
      report.rounds.push({
        round: round.round,
        seed: round.seed,
        hex: round.hex,
        replay: round.replay,
        suites: Object.fromEntries(SUITES.map((suite) => [suite, suitePublic(round.suites[suite])])),
        hard: round.hard,
      });
    }
  }

  finalize(report, roundsInternal);
  report.resultsFile = RESULTS_REL;
  try {
    writeResultsFile(report, roundsInternal);
  } catch (err) {
    const detail = err instanceof Error ? err.message : "unknown error";
    console.error(`${REPORT_WRITE_FAIL}: ${detail}`);
    report.verdict = "No-Go";
    report.exitCode = 1;
    if (!Array.isArray(report.reasons)) report.reasons = [];
    if (!report.reasons.includes(REPORT_WRITE_FAIL)) report.reasons.push(REPORT_WRITE_FAIL);
  }
  console.log(`verdict: ${report.verdict}`);
  console.log(JSON.stringify(report));
  return report.exitCode;
}

module.exports = { run };
