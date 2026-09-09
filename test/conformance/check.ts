import {
  baselineIdentity,
  capabilityCatalogBijection,
  catalogSchema,
} from "./checks/catalog.ts";
import {
  caseRegistryMapping,
  compatibilityCoverage,
  deviationBijection,
  inventoryGenerationDrift,
  inventoryMemberEvidence,
} from "./checks/inventory.ts";
import {
  cloudflareRunnerSafety,
  portableFixtureInventory,
} from "./checks/runner.ts";
import {
  compileFixtures,
  conformanceSelfTests,
  publicTypesSurface,
  unsupportedConfigRejection,
} from "./checks/types.ts";

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
  "unsupported-config-rejection",
  "portable-fixture-inventory",
  "cloudflare-runner-safety",
] as const;
type CaseId = (typeof CASES)[number];

const checks: Record<CaseId, () => void | Promise<void>> = {
  "baseline-identity": baselineIdentity,
  "catalog-schema": catalogSchema,
  "capability-catalog-bijection": capabilityCatalogBijection,
  "inventory-generation-drift": inventoryGenerationDrift,
  "inventory-member-evidence": inventoryMemberEvidence,
  "case-registry-mapping": caseRegistryMapping,
  "deviation-bijection": deviationBijection,
  "compatibility-coverage": compatibilityCoverage,
  "public-types-surface": publicTypesSurface,
  "compile-fixtures": compileFixtures,
  "conformance-self-tests": conformanceSelfTests,
  "unsupported-config-rejection": unsupportedConfigRejection,
  "portable-fixture-inventory": portableFixtureInventory,
  "cloudflare-runner-safety": cloudflareRunnerSafety,
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
  }
  const status = results.every((result) => result.status === "passed")
    ? "passed"
    : "failed";
  process.stdout.write(
    `${JSON.stringify({ schemaVersion: 1, status, cases: results })}\n`,
  );
  if (status === "failed") process.exitCode = 1;
}
