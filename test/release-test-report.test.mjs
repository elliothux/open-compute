import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { releaseTestReport } from "../scripts/release-test-report.ts";
import {
  createReleaseEvidence,
  writeJson,
} from "./fixtures/release-evidence.mjs";

const identity = {
  version: "1.2.3",
  revision: "a".repeat(40),
  workerd: "v1.20260830.1",
  workerdLockSha256: "b".repeat(64),
};
const digest = "c".repeat(64);

test("release report publishes selected metrics and escaped case names; rejects stale, incomplete or failed qualification", async () => {
  await mkdir(new URL("../.temp/release-report-tests/", import.meta.url), {
    recursive: true,
  });
  const directory = await mkdtemp(
    new URL("../.temp/release-report-tests/run-", import.meta.url).pathname,
  );
  await createReleaseEvidence(directory, identity, digest);
  const gatePath = join(directory, "gate-evidence-macos-15/report.json");
  const gate = JSON.parse(await readFile(gatePath, "utf8"));
  gate.results[0].targets[0].cases = ['<img src=x onerror="throw 1">'];
  gate.targets.push("open-compute-service.bin.ocd");
  gate.results[0].targets.push({
    target: "open-compute-service.bin.ocd",
    exit_code: 0,
    seconds: 0,
    cases: [],
    cases_passed: 0,
  });
  await writeJson(gatePath, gate);
  const rendered = await releaseTestReport(directory, identity, digest);
  assert.match(rendered.html, /open-compute/);
  assert.match(rendered.html, /&lt;img src=x onerror=&quot;throw 1&quot;&gt;/);
  assert.doesNotMatch(
    rendered.html,
    /<img src=x|<script|https?:\/\/[^" ]+\.js/,
  );
  for (const value of [rendered.json, rendered.html]) {
    assert.doesNotMatch(
      value,
      /not-for-publication-canary|private.example|private\/host/,
    );
  }
  const report = JSON.parse(rendered.json);
  assert.equal(report.gates.length, 6);
  assert.deepEqual(
    report.gates
      .find((entry) => entry.label === "gate-evidence-macos-15")
      .targets.at(-1).cases,
    [],
  );
  assert.equal(report.installs.length, 10);
  assert.equal(report.stress.runs.length, 5);
  const oldRevision = gate.revision;
  gate.revision = "f".repeat(40);
  await writeJson(gatePath, gate);
  await assert.rejects(
    releaseTestReport(directory, identity, digest),
    /revision/,
  );
  gate.revision = oldRevision;
  gate.results[0].targets[0].cases_passed = 0;
  await writeJson(gatePath, gate);
  await assert.rejects(
    releaseTestReport(directory, identity, digest),
    /all cases/,
  );
  const cases = gate.results[0].targets[0].cases;
  gate.results[0].targets[0].cases = [];
  gate.results[0].targets[0].cases_passed = 0;
  gate.test_cases = 0;
  gate.test_cases_passed = 0;
  await writeJson(gatePath, gate);
  await assert.rejects(
    releaseTestReport(directory, identity, digest),
    /case count/,
  );
  gate.results[0].targets[0].cases = cases;
  gate.results[0].targets[0].cases_passed = 1;
  gate.test_cases = 1;
  gate.test_cases_passed = 1;
  await writeJson(gatePath, gate);
  const stressPath = join(directory, "stress-qualification/qualification.json");
  const stress = JSON.parse(await readFile(stressPath, "utf8"));
  stress.runs.at(-1).scale = 0.1;
  await writeJson(stressPath, stress);
  await assert.rejects(
    releaseTestReport(directory, identity, digest),
    /full profile/,
  );
  stress.runs.at(-1).scale = 1;
  stress.runs.at(-1).soak.container_restarts_injected = 2;
  await writeJson(stressPath, stress);
  await assert.rejects(
    releaseTestReport(directory, identity, digest),
    /verified restart/,
  );
  stress.runs.at(-1).soak.container_restarts_injected = 3;
  stress.runs[1].stacks.http.samples = 0;
  await writeJson(stressPath, stress);
  await assert.rejects(
    releaseTestReport(directory, identity, digest),
    /measured SLO/,
  );
  await rm(join(directory, "package-gate-linux-arm64/report.json"));
  await assert.rejects(
    releaseTestReport(directory, identity, digest),
    /single Gate report/,
  );
  await rm(directory, { recursive: true });
});
