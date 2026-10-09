import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import type { ReleaseIdentity } from "./assemble-release.ts";

export const gateEvidence = [
  "coverage-gate-evidence",
  "gate-evidence-macos-15",
  "gate-evidence-ubuntu-24.04",
  "package-gate-darwin-arm64",
  "package-gate-linux-arm64",
  "package-gate-linux-x64",
] as const;
export const installEvidence = [
  "ubuntu-24-x64",
  "ubuntu-24-arm64",
  "debian-13-x64",
  "debian-13-arm64",
  "macos-arm64",
] as const;

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("invalid qualification record");
  return value as Record<string, unknown>;
}
function list(value: unknown): unknown[] {
  if (!Array.isArray(value)) throw new Error("invalid qualification inventory");
  return value;
}
function text(value: unknown): string {
  if (typeof value !== "string" || !value || value.length > 512)
    throw new Error("invalid qualification label");
  return value;
}
function number(value: unknown): number {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0)
    throw new Error("invalid qualification metric");
  return value;
}
function requirePass(condition: boolean, label: string): void {
  if (!condition) throw new Error(`release qualification failed: ${label}`);
}
async function json(path: string): Promise<Record<string, unknown>> {
  return record(JSON.parse(await readFile(path, "utf8")) as unknown);
}
async function reports(
  directory: string,
  filename = "report.json",
): Promise<Record<string, unknown>[]> {
  const result: Record<string, unknown>[] = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) result.push(...(await reports(path, filename)));
    else if (entry.isFile() && entry.name === filename)
      result.push(await json(path));
  }
  return result;
}

/** Consume only explicitly selected metrics; logs and arbitrary evidence fields never enter public reports. */
export async function releaseTestReport(
  directory: string,
  identity: ReleaseIdentity,
  candidateSha256: string,
) {
  const gates = [];
  for (const label of gateEvidence) {
    const candidates = (await reports(join(directory, label))).filter(
      (value) => "rounds" in value,
    );
    requirePass(
      candidates.length === 1,
      `${label}: exact single Gate report required`,
    );
    const raw = record(candidates[0]);
    requirePass(
      raw.status === "passed" &&
        raw.revision === identity.revision &&
        raw.rounds === 1 &&
        raw.inventory_verified === true,
      `${label}: revision, inventory or status`,
    );
    const inputs = record(raw.inputs);
    requirePass(
      inputs.pin_sha256 === identity.workerdLockSha256 &&
        inputs.workerd === identity.workerd,
      `${label}: formal runtime pin`,
    );
    const rounds = list(raw.results);
    requirePass(rounds.length === 1, `${label}: one complete round`);
    const planned = list(raw.targets).map(text);
    const targets = list(record(rounds[0]).targets).map((item) => {
      const target = record(item);
      const cases = list(target.cases).map(text);
      requirePass(
        target.exit_code === 0 &&
          target.cases_passed === cases.length &&
          new Set(cases).size === cases.length,
        `${label}: all cases must execute once`,
      );
      return {
        name: text(target.target),
        seconds: number(target.seconds),
        cases,
      };
    });
    requirePass(
      planned.length === targets.length &&
        new Set(targets.map((target) => target.name)).size === planned.length &&
        targets.every((target) => planned.includes(target.name)),
      `${label}: target inventory`,
    );
    const count = targets.reduce(
      (total, target) => total + target.cases.length,
      0,
    );
    requirePass(
      count > 0 && raw.test_cases === count && raw.test_cases_passed === count,
      `${label}: case count`,
    );
    gates.push({ label, seconds: number(raw.seconds), cases: count, targets });
  }
  const coverageCandidates = await reports(
    join(directory, "coverage-gate-evidence"),
    "summary.json",
  );
  requirePass(coverageCandidates.length === 1, "one coverage summary required");
  const data = list(record(coverageCandidates[0]).data);
  requirePass(data.length === 1, "one workspace coverage dataset required");
  const lines = record(record(record(data[0]).totals).lines);
  const covered = number(lines.covered),
    count = number(lines.count),
    percent = number(lines.percent);
  requirePass(
    count > 0 &&
      covered <= count &&
      percent <= 100 &&
      percent >= 90 &&
      Math.abs(percent - (100 * covered) / count) < 0.02,
    "workspace line coverage >=90%",
  );

  const installs = [];
  for (const platform of installEvidence) {
    const entries = await reports(
      join(directory, `install-lifecycle-${platform}`),
    );
    requirePass(
      entries.length === 2 &&
        new Set(entries.map((entry) => entry.scope)).size === 2,
      `${platform}: both install scopes required`,
    );
    for (const raw of entries) {
      requirePass(
        raw.passed === true &&
          raw.candidate === `v${identity.version}` &&
          ["user", "system"].includes(text(raw.scope)),
        `${platform}: install lifecycle status or version`,
      );
      const events = list(raw.events).map((value) => {
        const event = record(value);
        return { step: text(event.step), exitCode: number(event.exitCode) };
      });
      requirePass(events.length > 0, `${platform}: missing lifecycle events`);
      installs.push({
        platform,
        scope: text(raw.scope),
        previous: text(raw.previous),
        events,
      });
    }
  }
  const stress = await json(
    join(directory, "stress-qualification/qualification.json"),
  );
  requirePass(
    stress.status === "passed" &&
      stress.revision === identity.revision &&
      stress.version === identity.version &&
      stress.candidateSha256 === candidateSha256 &&
      stress.workerdLockSha256 === identity.workerdLockSha256 &&
      stress.cpuLimit === 2 &&
      stress.memoryLimitBytes === 4 * 1024 ** 3,
    "candidate stress identity and resource limits",
  );
  const expectedProfiles = [
    "smoke",
    "p0-2c4g",
    "scenario",
    "p1-2c4g-peak",
    "p1-2c4g-soak",
  ];
  const runs = list(stress.runs).map((value) => {
    const raw = record(value),
      profile = text(raw.profile);
    requirePass(
      raw.verdict === "pass" &&
        expectedProfiles.includes(profile) &&
        list(raw.global_anomalies).length === 0,
      `${profile}: stress verdict`,
    );
    const stacks = Object.entries(record(raw.stacks)).map(([name, value]) => {
      const stack = record(value),
        latency = record(stack.latency_ms),
        thresholds = record(stack.thresholds);
      const samples = number(stack.samples),
        errors = number(stack.errors),
        rate = number(stack.error_rate);
      const p50 = number(latency.p50),
        p95 = number(latency.p95),
        p99 = number(latency.p99);
      const maxError = number(thresholds.error_rate_max),
        maxP95 = number(thresholds.p95_ms);
      const maxP99 =
        thresholds.p99_ms === undefined ? undefined : number(thresholds.p99_ms);
      requirePass(
        samples > 0 &&
          errors <= samples &&
          Math.abs(rate - errors / samples) < 0.000001 &&
          rate <= maxError &&
          p50 <= p95 &&
          p95 <= p99 &&
          p95 <= maxP95 &&
          (maxP99 === undefined || p99 <= maxP99) &&
          stack.verdict === "pass" &&
          list(stack.anomalies).length === 0,
        `${profile}/${name}: measured SLO`,
      );
      return {
        name,
        samples,
        errors,
        errorRate: rate,
        p50,
        p95,
        p99,
        maxError,
        maxP95,
        maxP99,
      };
    });
    if (profile !== "smoke" && profile !== "scenario") {
      requirePass(
        raw.scale === 1 && stacks.length > 0,
        `${profile}: full profile required`,
      );
      const required = [
        "http",
        "kv",
        "d1",
        "r2",
        "queue",
        "do",
        "workflow",
        "fetch",
      ];
      if (profile.endsWith("soak")) required.push("scenario_mega");
      else required.push("cpu");
      if (profile.startsWith("p0")) required.push("service", "scenario_mega");
      requirePass(
        required.length === stacks.length &&
          required.every((name) => stacks.some((stack) => stack.name === name)),
        `${profile}: stack inventory`,
      );
    }
    let restarts = 0;
    if (profile.endsWith("soak")) {
      const soak = record(raw.soak),
        events = list(soak.events).map(record);
      restarts = number(soak.container_restarts_injected);
      requirePass(
        restarts >= 3 &&
          events.filter((event) => event.event === "restart_reconcile_ok")
            .length === restarts &&
          events.some(
            (event) =>
              event.event === "soak_end" && number(event.elapsed_sec) >= 3600,
          ),
        "one-hour soak and verified restart recovery",
      );
    }
    return { profile, seconds: number(raw.seconds), stacks, restarts };
  });
  requirePass(
    runs.length === expectedProfiles.length &&
      new Set(runs.map((run) => run.profile)).size === runs.length,
    "complete stress suite inventory",
  );
  const report = {
    schemaVersion: 1,
    ...identity,
    verdict: "pass",
    coverage: { covered, count, percent },
    gates,
    installs,
    stress: {
      candidateSha256,
      cpuLimit: 2,
      memoryLimitBytes: 4 * 1024 ** 3,
      runs,
    },
  };
  return {
    json: JSON.stringify(report, null, 2) + "\n",
    html: await render(report),
  };
}

function escape(value: string | number): string {
  return String(value).replace(
    /[&<>"']/g,
    (char) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        char
      ] ?? char,
  );
}
async function render(report: {
  version: string;
  revision: string;
  workerd: string;
  coverage: { percent: number; covered: number; count: number };
  gates: {
    label: string;
    seconds: number;
    cases: number;
    targets: { name: string; seconds: number; cases: string[] }[];
  }[];
  installs: {
    platform: string;
    scope: string;
    previous: string;
    events: { step: string; exitCode: number }[];
  }[];
  stress: {
    runs: {
      profile: string;
      seconds: number;
      restarts: number;
      stacks: {
        name: string;
        samples: number;
        errors: number;
        errorRate: number;
        p50: number;
        p95: number;
        p99: number;
        maxP95: number;
        maxP99: number | undefined;
        maxError: number;
      }[];
    }[];
  };
}): Promise<string> {
  const brand = await readFile(
    new URL("../apps/website/public/favicon.svg", import.meta.url),
    "utf8",
  );
  const cases = report.gates.reduce((total, gate) => total + gate.cases, 0);
  return `<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>open-compute ${escape(report.version)} · Test report</title>
<style>
:root{color-scheme:light;--ink:#1a1a1a;--blue:#0055ff;--line:#e0e0e0;--paper:#fafafa}*{box-sizing:border-box}body{margin:0;color:var(--ink);background:var(--paper);font:15px/1.6 system-ui,sans-serif}header,main,footer{max-width:1160px;margin:auto;padding:32px}header{display:flex;justify-content:space-between;align-items:center;border-bottom:1px solid var(--line)}.brand{display:flex;align-items:center;gap:12px;font-weight:700;font-size:22px;letter-spacing:-.04em}.brand svg{width:36px;height:36px}a{color:var(--blue)}nav{display:flex;gap:20px}h1{font-size:clamp(40px,7vw,76px);line-height:1.05;letter-spacing:-.06em;margin:20px 0}h2{font-size:28px;letter-spacing:-.03em;margin-top:52px}.eyebrow{color:var(--blue);letter-spacing:.12em;text-transform:uppercase;font-size:12px;font-weight:700}.pill{display:inline-block;color:#075a32;background:#e7f7ee;padding:5px 12px;border-radius:30px;font-size:12px;font-weight:700}.hero{padding:38px 0 24px}.hero p{max-width:680px;color:#575757}.metrics{display:grid;grid-template-columns:repeat(4,1fr);gap:16px;margin:32px 0}.card{padding:24px;background:white;border:1px solid var(--line);border-radius:12px}.card strong{display:block;font-size:38px;letter-spacing:-.04em}.card small{color:#666}.accent{background:var(--blue);color:white;border-color:var(--blue)}.accent small{color:white}details{background:white;border:1px solid var(--line);border-radius:10px;margin:12px 0}summary{cursor:pointer;padding:18px 22px;font-weight:600}summary span{float:right;color:#666;font-weight:400;font-size:13px}details>div{padding:0 22px 22px}.table{overflow:auto}table{border-collapse:collapse;width:100%;font-variant-numeric:tabular-nums;font-size:13px}th,td{text-align:left;padding:12px 14px;border-bottom:1px solid var(--line);white-space:nowrap}th{font-size:11px;text-transform:uppercase;letter-spacing:.08em;color:#666}code{font:12px ui-monospace,monospace;overflow-wrap:anywhere}li{margin:4px 0}.identity{padding:24px;background:#f0f3fa;border-radius:12px}.identity p{margin:6px 0}footer{color:#666;font-size:12px;border-top:1px solid var(--line)}@media(max-width:700px){header,main,footer{padding:20px}nav{gap:10px;font-size:12px}.metrics{grid-template-columns:repeat(2,1fr)}summary span{float:none;display:block}.card{padding:16px}.card strong{font-size:30px}}@media print{details{break-inside:avoid}nav{display:none}}
</style>
<header><a class="brand" href="https://open-compute.dev" style="color:inherit;text-decoration:none">${brand}open-compute</a><nav><a href="#gates">Gates</a><a href="#stress">Stress</a><a href="#install">Lifecycle</a></nav></header><main>
<section class="hero"><div class="eyebrow">v${escape(report.version)}</div><h1>Release test report</h1><span class="pill">PASS</span><p>Stress environment: 2 CPUs / 4 GiB; soak duration: one hour.</p></section>
<div class="metrics"><div class="card accent"><small>Gate cases executed</small><strong>${cases.toLocaleString("en")}</strong><small>One round per qualification suite</small></div><div class="card"><small>Rust line coverage</small><strong>${report.coverage.percent.toFixed(2)}%</strong><small>${report.coverage.covered.toLocaleString("en")} / ${report.coverage.count.toLocaleString("en")} lines</small></div><div class="card"><small>Native release targets</small><strong>3</strong><small>macOS arm64 · Linux arm64 / x64</small></div><div class="card"><small>Soak qualification</small><strong>60 min</strong><small>2 CPUs · 4 GiB · restart recovery</small></div></div>
<section class="identity"><p><b>Version</b> ${escape(report.version)} &nbsp; <b>Workerd</b> ${escape(report.workerd)}</p><p><b>Source revision</b> <code>${escape(report.revision)}</code></p><p>Machine-readable metrics: <a href="test-report.json">test-report.json</a>. Verify both reports with this release’s SHA256SUMS.</p></section>
<h2 id="gates">Runtime &amp; workspace Gates</h2><p>Counts include coverage execution and package qualification.</p>
${report.gates.map((gate) => `<details><summary>${escape(gate.label)} <span>${gate.cases} cases · ${gate.seconds.toFixed(1)} s · PASS</span></summary><div>${gate.targets.map((target) => `<details><summary>${escape(target.name)} <span>${target.cases.length} cases · ${target.seconds.toFixed(1)} s</span></summary><div><ul>${target.cases.map((name) => `<li><code>${escape(name)}</code> — passed</li>`).join("")}</ul></div></details>`).join("")}</div></details>`).join("")}
<h2 id="stress">Load, latency &amp; recovery</h2>
${report.stress.runs.map((run) => `<details open><summary>${escape(run.profile)} <span>${run.seconds.toFixed(1)} s · ${run.restarts ? `${run.restarts} verified restarts · ` : ""}PASS</span></summary><div class="table">${run.stacks.length ? `<table><thead><tr><th>Stack</th><th>Samples</th><th>Errors / ceiling</th><th>p50</th><th>p95 / ceiling</th><th>p99 / ceiling</th></tr></thead><tbody>${run.stacks.map((stack) => `<tr><td>${escape(stack.name)}</td><td>${stack.samples.toLocaleString("en")}</td><td>${stack.errors} · ${(100 * stack.errorRate).toFixed(2)}% / ${(100 * stack.maxError).toFixed(2)}%</td><td>${stack.p50} ms</td><td>${stack.p95} / ${stack.maxP95} ms</td><td>${stack.p99}${stack.maxP99 === undefined ? " ms" : ` / ${stack.maxP99} ms`}</td></tr>`).join("")}</tbody></table>` : `<p>Functional checks passed; no global anomalies recorded.</p>`}</div></details>`).join("")}
<h2 id="install">Install, upgrade &amp; service lifecycle</h2>${report.installs.map((run) => `<details><summary>${escape(run.platform)} / ${escape(run.scope)} <span>${run.events.length} steps · PASS</span></summary><div><p>Upgrade from ${escape(run.previous)} to v${escape(report.version)}; includes expected rejection checks.</p><table><thead><tr><th>Step</th><th>Exit code</th></tr></thead><tbody>${run.events.map((event) => `<tr><td>${escape(event.step)}</td><td>${event.exitCode}</td></tr>`).join("")}</tbody></table></div></details>`).join("")}
</main><footer>open-compute · Test report</footer></html>\n`;
}
