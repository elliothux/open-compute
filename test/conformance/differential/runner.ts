import { mkdir, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import type { JsonRecord, PortableFixture } from "../adapters/types.ts";
import { runFixture } from "./fixture-runner.ts";
import { prepareRun } from "./run-context.ts";

export async function runDifferential(
  root: string,
  selected: readonly PortableFixture[],
): Promise<JsonRecord> {
  const context = await prepareRun(root, selected);
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
