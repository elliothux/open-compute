import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import {
  gateEvidence,
  installEvidence,
} from "../../scripts/release-test-report.ts";

export async function writeJson(path, value) {
  await writeFile(path, JSON.stringify(value));
}

export async function createReleaseEvidence(
  directory,
  identity,
  candidateSha256,
) {
  for (const label of gateEvidence) {
    await mkdir(join(directory, label), { recursive: true });
    await writeJson(join(directory, label, "report.json"), {
      rounds: 1,
      status: "passed",
      revision: identity.revision,
      inventory_verified: true,
      seconds: 2.5,
      targets: ["single-binary"],
      test_cases: 1,
      test_cases_passed: 1,
      inputs: {
        pin_sha256: identity.workerdLockSha256,
        workerd: identity.workerd,
      },
      results: [
        {
          targets: [
            {
              target: "single-binary",
              exit_code: 0,
              seconds: 1.5,
              cases: ["actual_case"],
              cases_passed: 1,
            },
          ],
        },
      ],
      environment: { secret: "not-for-publication-canary" },
      log: "/private/host/path",
    });
  }
  await writeJson(join(directory, "coverage-gate-evidence/summary.json"), {
    data: [{ totals: { lines: { count: 100, covered: 95, percent: 95 } } }],
  });
  for (const platform of installEvidence) {
    const root = join(directory, `install-lifecycle-${platform}`);
    for (const scope of ["user", "system"]) {
      await mkdir(join(root, scope), { recursive: true });
      await writeJson(join(root, scope, "report.json"), {
        passed: true,
        candidate: `v${identity.version}`,
        previous: "v1.2.2",
        scope,
        events: [
          { step: "fresh install", exitCode: 0 },
          { step: "expected rejection", exitCode: 1 },
        ],
      });
    }
  }
  const runs = [
    "smoke",
    "p0-2c4g",
    "scenario",
    "p1-2c4g-peak",
    "p1-2c4g-soak",
  ].map((profile) => {
    const stacks = {};
    if (!["smoke", "scenario"].includes(profile)) {
      const names = [
        "http",
        "kv",
        "d1",
        "r2",
        "queue",
        "do",
        "workflow",
        "fetch",
      ];
      names.push(profile.endsWith("soak") ? "scenario_mega" : "cpu");
      if (profile.startsWith("p0")) names.push("service", "scenario_mega");
      for (const name of names)
        stacks[name] = {
          samples: 100,
          errors: 0,
          error_rate: 0,
          verdict: "pass",
          anomalies: [],
          latency_ms: { p50: 10, p95: 20, p99: 30 },
          thresholds: { error_rate_max: 0.01, p95_ms: 800, p99_ms: 1500 },
        };
    }
    return {
      profile,
      verdict: "pass",
      seconds: profile.endsWith("soak") ? 3601 : 60,
      scale: 1,
      stacks,
      global_anomalies: [],
      ...(profile.endsWith("soak")
        ? {
            soak: {
              container_restarts_injected: 3,
              events: [
                { event: "restart_reconcile_ok" },
                { event: "restart_reconcile_ok" },
                { event: "restart_reconcile_ok" },
                { event: "soak_end", elapsed_sec: 3601 },
              ],
            },
          }
        : {}),
      worker_host: "private.example.invalid",
      secret: "not-for-publication-canary",
    };
  });
  await mkdir(join(directory, "stress-qualification"), { recursive: true });
  await writeJson(join(directory, "stress-qualification/qualification.json"), {
    schemaVersion: 1,
    status: "passed",
    ...identity,
    candidateSha256,
    cpuLimit: 2,
    memoryLimitBytes: 4 * 1024 ** 3,
    runs,
  });
  return directory;
}
