const CASES = [
  "baseline-identity",
  "catalog-schema",
  "capability-catalog-bijection",
  "inventory-generation-drift",
  "inventory-member-evidence",
  "case-registry-mapping",
  "deviation-bijection",
  "compatibility-coverage",
  "public-types-surface",
  "compile-fixtures",
  "conformance-self-tests",
  "portable-fixture-inventory",
  "cloudflare-runner-safety",
] as const;
type CaseId = (typeof CASES)[number];

export {};

const checks: Record<CaseId, () => Promise<void>> = {
  "baseline-identity": async () =>
    (await import("./checks/catalog.ts")).baselineIdentity(),
  "catalog-schema": async () =>
    (await import("./checks/catalog.ts")).catalogSchema(),
  "capability-catalog-bijection": async () =>
    (await import("./checks/catalog.ts")).capabilityCatalogBijection(),
  "inventory-generation-drift": async () =>
    (await import("./checks/inventory.ts")).inventoryGenerationDrift(),
  "inventory-member-evidence": async () =>
    (await import("./checks/inventory.ts")).inventoryMemberEvidence(),
  "case-registry-mapping": async () =>
    (await import("./checks/inventory.ts")).caseRegistryMapping(),
  "deviation-bijection": async () =>
    (await import("./checks/inventory.ts")).deviationBijection(),
  "compatibility-coverage": async () =>
    (await import("./checks/inventory.ts")).compatibilityCoverage(),
  "public-types-surface": async () =>
    (await import("./checks/types.ts")).publicTypesSurface(),
  "compile-fixtures": async () =>
    (await import("./checks/types.ts")).compileFixtures(),
  "conformance-self-tests": async () =>
    (await import("./checks/types.ts")).conformanceSelfTests(),
  "portable-fixture-inventory": async () =>
    (await import("./checks/runner.ts")).portableFixtureInventory(),
  "cloudflare-runner-safety": async () =>
    (await import("./checks/runner.ts")).cloudflareRunnerSafety(),
};

const args = process.argv.slice(2);
if (args.length === 1 && args[0] === "--list") {
  process.stdout.write(
    `${JSON.stringify({ schemaVersion: 1, cases: CASES })}\n`,
  );
} else {
  const selected: string[] = [];
  for (let index = 0; index < args.length; index += 2) {
    if (args[index] !== "--case" || args[index + 1] === undefined)
      throw new Error("use --case <id>");
    selected.push(args[index + 1]!);
  }
  const requested = selected.length ? selected : [...CASES];
  if (
    new Set(requested).size !== requested.length ||
    requested.some((id) => !CASES.includes(id as CaseId))
  ) {
    throw new Error("unknown or duplicate conformance case");
  }
  const results: { id: string; status: "passed" | "failed"; error?: string }[] =
    [];
  for (const id of requested) {
    const started = performance.now();
    try {
      await checks[id as CaseId]();
      results.push({ id, status: "passed" });
    } catch (error) {
      results.push({
        id,
        status: "failed",
        error:
          error instanceof Error ? error.message : "conformance check failed",
      });
    }
    // A killed run still shows which case consumed the Gate timeout.
    const seconds = ((performance.now() - started) / 1000).toFixed(2);
    process.stderr.write(`${id} ${results.at(-1)!.status} ${seconds}s\n`);
  }
  const status = results.every((result) => result.status === "passed")
    ? "passed"
    : "failed";
  process.stdout.write(
    `${JSON.stringify({ schemaVersion: 1, status, cases: results })}\n`,
  );
  if (status === "failed") process.exitCode = 1;
}
