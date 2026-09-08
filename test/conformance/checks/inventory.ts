import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { JsonRecord } from "../adapters/types.ts";
import { validateCaseEvidence } from "../case-evidence.ts";
import { generateInventoryTwice } from "../inventory.ts";
import {
  array,
  baseline,
  capabilities,
  catalog,
  contracts,
  inventory,
  inventoryMembers,
  json,
  record,
  ROOT,
  string,
  strings,
} from "./context.ts";

export async function inventoryGenerationDrift(): Promise<void> {
  const { encoded } = await generateInventoryTwice();
  const committed = readFileSync(
    join(ROOT, "share/cloudflare-capabilities.json"),
    "utf8",
  );
  if (encoded !== committed)
    throw new Error(
      "share/cloudflare-capabilities.json drifted from generated inventory",
    );
  const value = inventory();
  if (value.schema_version !== 1)
    throw new Error("inventory schema_version must be 1");
  const source = record(value.source, "inventory.source");
  const lock = record(
    json("packages/runtime/workerd.lock.json"),
    "workerd lock",
  );
  const lockTypes = record(lock.workersTypes, "lock.workersTypes");
  if (
    string(source.workers_types_version, "source.workers_types_version") !==
      string(lockTypes.version, "lock.workersTypes.version") ||
    string(source.git_head, "source.git_head") !==
      string(lockTypes.gitHead, "lock.workersTypes.gitHead") ||
    string(source.package_sha256, "source.package_sha256") !==
      string(lockTypes.packageSha256, "lock.workersTypes.packageSha256") ||
    string(source.ast_sha256, "source.ast_sha256") !==
      string(lockTypes.astSha256, "lock.workersTypes.astSha256")
  ) {
    throw new Error(
      "inventory source identity does not match the formal workers-types pin",
    );
  }
}

export function inventoryMemberEvidence(): void {
  const members = inventoryMembers();
  const byId = new Map<string, JsonRecord>();
  for (const member of members) {
    const id = string(member.id, "member.id");
    if (byId.has(id)) throw new Error(`duplicate inventory member: ${id}`);
    byId.set(id, member);
    const status = string(member.status, `${id}.status`);
    if (status === "unsupported")
      throw new Error(`${id}: unsupported is reserved for non-target products`);
    const compileCases = strings(
      member.compile_cases ?? [],
      `${id}.compile_cases`,
    );
    const runtimeCases = strings(
      member.runtime_cases ?? [],
      `${id}.runtime_cases`,
    );
    if (
      (status === "supported" || status === "supported_with_deviation") &&
      (!compileCases.length || !runtimeCases.length)
    ) {
      throw new Error(
        `${id}: supported member lacks compile and real-runtime cases`,
      );
    }
    if (status === "blocked" && (compileCases.length || runtimeCases.length)) {
      throw new Error(`${id}: blocked member must not carry evidence cases`);
    }
  }
  const evidenceIds = new Set<string>();
  for (const [index, raw] of array(
    catalog().memberEvidence ?? [],
    "memberEvidence",
  ).entries()) {
    const item = record(raw, `memberEvidence[${index}]`);
    const id = string(item.id, `memberEvidence[${index}].id`);
    const member = byId.get(id);
    if (member === undefined) throw new Error(`stale memberEvidence id: ${id}`);
    evidenceIds.add(id);
    if (
      string(member.status, `${id}.status`) !==
      string(item.status, `${id}.evidenceStatus`)
    ) {
      throw new Error(`${id}: inventory status does not match memberEvidence`);
    }
  }
  for (const member of members) {
    const status = string(member.status, "member.status");
    const id = string(member.id, "member.id");
    if (
      (status === "supported" || status === "supported_with_deviation") &&
      !evidenceIds.has(id)
    ) {
      throw new Error(`${id}: supported member is missing from memberEvidence`);
    }
  }
  const coveredBlocked = new Set<string>();
  for (const [index, raw] of array(
    catalog().blockedGaps ?? [],
    "blockedGaps",
  ).entries()) {
    const gap = record(raw, `blockedGaps[${index}]`);
    const gapId = string(gap.id, `blockedGaps[${index}].id`);
    for (const id of strings(gap.memberIds, `${gapId}.memberIds`)) {
      const member = byId.get(id);
      if (member === undefined)
        throw new Error(`${gapId}: stale blocked member ID: ${id}`);
      if (member.status !== "blocked")
        throw new Error(`${gapId}: ${id} must remain blocked`);
      if (coveredBlocked.has(id))
        throw new Error(`${id}: blocked member has more than one gap owner`);
      coveredBlocked.add(id);
    }
  }
  for (const member of members) {
    const id = string(member.id, "member.id");
    if (member.status === "blocked" && !coveredBlocked.has(id)) {
      throw new Error(`${id}: blocked member has no explicit gap owner`);
    }
  }
}

export function caseRegistryMapping(): void {
  const output = execFileSync(
    "python3",
    [join(ROOT, "test/gate_cases.py"), "--json"],
    {
      cwd: ROOT,
      encoding: "utf8",
      env: { PATH: process.env.PATH },
      timeout: 10_000,
    },
  );
  validateCaseEvidence(ROOT, catalog(), JSON.parse(output));
}

export function deviationBijection(): void {
  const registry = readFileSync(
    join(ROOT, "docs/references/p1-deviations.md"),
    "utf8",
  );
  const documented = [...registry.matchAll(/`(OC-[A-Z0-9-]+)`:/g)].flatMap(
    (match) => (match[1] === undefined ? [] : [match[1]]),
  );
  const advertised = new Set<string>();
  const mapped = new Set<string>();
  for (const id of strings(
    record(inventory().managementApi, "managementApi").deviations ?? [],
    "managementApi.deviations",
  ))
    advertised.add(id);
  for (const id of strings(
    record(catalog().managementApi, "catalog.managementApi").deviations ?? [],
    "catalog.managementApi.deviations",
  ))
    mapped.add(id);
  for (const raw of Object.values(capabilities())) {
    for (const id of strings(
      record(raw, "capability").deviations ?? [],
      "deviations",
    ))
      advertised.add(id);
  }
  for (const contract of contracts())
    for (const id of strings(contract.deviations, "contract.deviations"))
      mapped.add(id);
  const normalized = (items: Iterable<string>) => [...items].sort().join("\0");
  if (
    normalized(documented) !== normalized(advertised) ||
    normalized(advertised) !== normalized(mapped)
  ) {
    throw new Error("deviation registry, capabilities, and catalog differ");
  }
}

export function compatibilityCoverage(): void {
  const lock = record(
    json("packages/runtime/workerd.lock.json"),
    "workerd lock",
  );
  const date = string(
    lock.effectiveCompatibilityDate,
    "lock.effectiveCompatibilityDate",
  );
  if (
    string(
      baseline().effectiveCompatibilityDate,
      "baseline.effectiveCompatibilityDate",
    ) !== date
  ) {
    throw new Error(
      "baseline effective compatibility date does not match the formal lock",
    );
  }
  for (const contract of contracts()) {
    if (contract.compatibility !== undefined) {
      throw new Error(
        `${contract.id}: catalog must not carry tenant compatibility selectors`,
      );
    }
  }
}
