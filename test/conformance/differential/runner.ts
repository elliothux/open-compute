import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { format } from "prettier";
import { canonicalJson } from "../adapters/observations.ts";
import { CF_VERSION } from "../adapters/runtime-contract.ts";
import type { JsonRecord, PortableFixture } from "../adapters/types.ts";
import { sourceIdentity } from "../checks/context.ts";
import { runFixture } from "./fixture-runner.ts";
import { prepareRun, safeAlias } from "./run-context.ts";

export async function runDifferential(
  root: string,
  selected: readonly PortableFixture[],
  local = false,
  refresh = false,
): Promise<JsonRecord> {
  if (local && refresh)
    throw new Error("local observations cannot refresh Cloudflare recordings");
  const context = await prepareRun(root, selected, local, refresh);
  const results: JsonRecord[] = [];
  const cleanup: JsonRecord[] = [];
  let failed: string | undefined;
  try {
    for (let index = 0; index < selected.length; index++) {
      const fixtureResult = await runFixture(context, selected[index]!, index);
      results.push(fixtureResult.result);
      failed = fixtureResult.failure;
      const cleanupResult = await fixtureResult.cleanup();
      cleanup.push(cleanupResult.record);
      if (!cleanupResult.deleted) {
        failed = `${selected[index]!.id}: cleanup did not prove an empty final resource set`;
      }
      if (failed !== undefined) break;
    }
  } finally {
    if (failed === undefined) {
      await rm(context.directory, { recursive: true });
    } else {
      const failedDirectory = join(context.directory, "failed");
      await mkdir(failedDirectory, { recursive: true });
      await writeFile(
        join(failedDirectory, "result.json"),
        `${JSON.stringify(
          {
            schemaVersion: 1,
            revision: context.revision,
            workingTreeSha256: context.workingTreeSha256,
            results,
            cleanup,
            error: failed,
          },
          null,
          2,
        )}\n`,
        { mode: 0o600 },
      );
    }
  }
  if (refresh && failed === undefined)
    await refreshRecordings(root, selected, results, context.accountAlias, [
      context.accountId,
      context.token,
      context.adminToken,
      context.openComputeInternalAccount,
      context.openComputeAccount,
    ]);
  return {
    schemaVersion: 1,
    status: failed === undefined ? "passed" : "failed",
    cases: results.map((item) => ({
      id: item.id,
      status: item.status,
      ...(item.error === undefined ? {} : { error: item.error }),
    })),
    differential: {
      revision: context.revision,
      workingTreeSha256: context.workingTreeSha256,
      accountAlias: context.accountAlias,
      prefix: context.prefix,
      results,
      cleanup,
      error: failed,
    },
  };
}

export async function refreshRecordings(
  root: string,
  fixtures: readonly PortableFixture[],
  results: readonly JsonRecord[],
  accountAlias: string,
  secrets: readonly (string | undefined)[] = [],
): Promise<void> {
  safeAlias(accountAlias);
  if (secrets.some((secret) => secret && accountAlias.includes(secret)))
    throw new Error("recording alias contains a private identity or secret");
  if (
    results.length !== fixtures.length ||
    results.some(
      (result, index) =>
        result.status !== "passed" ||
        result.id !== fixtures[index]!.id ||
        !Array.isArray(result.cloudflare) ||
        result.cloudflare.length !== fixtures[index]!.observations.length,
    )
  )
    throw new Error(
      "only complete, passing Cloudflare observations can refresh recordings",
    );
  const writes: { path: string; content: string }[] = [];
  for (let index = 0; index < fixtures.length; index++) {
    const fixture = fixtures[index]!;
    const path = join(fixture.root, "contract.json");
    const original = await readFile(path, "utf8");
    const recording = JSON.parse(original) as {
      observations: { expect: unknown }[];
    };
    const observations = results[index]!.cloudflare as {
      status: number;
      json: unknown;
    }[];
    for (const [observationIndex, actual] of observations.entries()) {
      if (!Number.isInteger(actual.status) || !Object.hasOwn(actual, "json"))
        throw new Error("invalid Cloudflare observation");
      recording.observations[observationIndex]!.expect = actual;
    }
    if (
      JSON.stringify(canonicalJson(JSON.parse(original))) !==
      JSON.stringify(canonicalJson(recording))
    ) {
      const content = await format(JSON.stringify(recording), {
        parser: "json",
        filepath: path,
      });
      if (secrets.some((secret) => secret && content.includes(secret)))
        throw new Error(
          "recorded observation contains a private identity or secret",
        );
      writes.push({ path, content });
    }
  }
  for (const { path, content } of writes) await writeFile(path, content);
  const baselinePath = join(root, "test/conformance/baseline.json");
  const original = await readFile(baselinePath, "utf8");
  const baseline = JSON.parse(original) as JsonRecord;
  baseline.portableRecording = {
    date: new Date().toISOString().slice(0, 10),
    cfVersion: CF_VERSION,
    accountAlias,
  };
  baseline.sourceDigest = sourceIdentity();
  const content = await format(JSON.stringify(baseline), {
    parser: "json",
    filepath: baselinePath,
  });
  if (content !== original) await writeFile(baselinePath, content);
}
