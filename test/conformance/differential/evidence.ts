import { createHash } from "node:crypto";
import { appendFile, readFile, stat } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { command } from "../adapters/command.ts";
import { fetchObservation, observationUrl } from "../adapters/transport.ts";
import type { JsonRecord, PortableFixture } from "../adapters/types.ts";
import { processEnv } from "./environment.ts";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");

export async function sourceIdentity(): Promise<string> {
  const env = processEnv({});
  const tracked = await command(
    "git",
    ["diff", "--name-only", "-z", "--no-ext-diff", "HEAD"],
    {
      cwd: ROOT,
      env,
      timeout: 30_000,
    },
  );
  const untracked = await command(
    "git",
    ["ls-files", "-z", "--others", "--exclude-standard"],
    {
      cwd: ROOT,
      env,
      timeout: 30_000,
    },
  );
  const names = [
    ...new Set(
      `${tracked.stdout}\0${untracked.stdout}`.split("\0").filter(Boolean),
    ),
  ]
    .filter((name) => !name.split("/").includes("__pycache__"))
    .sort();
  const digest = createHash("sha256").update("open-compute-working-tree/v2\0");
  for (const name of names) {
    const path = resolve(ROOT, name);
    if (!path.startsWith(`${ROOT}/`))
      throw new Error("working-tree source identity escapes the repository");
    try {
      if (!(await stat(path)).isFile())
        throw new Error("working-tree source identity is not a regular file");
      digest
        .update("file\0")
        .update(name)
        .update("\0")
        .update(await readFile(path));
    } catch (error) {
      if (
        error !== null &&
        typeof error === "object" &&
        Reflect.get(error, "code") === "ENOENT"
      ) {
        digest.update("deleted\0").update(name).update("\0");
      } else {
        throw error;
      }
    }
  }
  return digest.digest("hex");
}

export async function recordOwnership(
  path: string,
  entry: JsonRecord,
): Promise<void> {
  await appendFile(
    path,
    `${JSON.stringify({ ...entry, recordedAtMs: Date.now() })}\n`,
    { mode: 0o600 },
  );
}

export async function bestEffortFixtureCleanup(
  base: string,
  fixture: PortableFixture,
  requestHeaders: Readonly<Record<string, string>>,
): Promise<void> {
  const cleanup = fixture.observations.find(
    (observation) => observation.path === "/cleanup",
  );
  if (cleanup === undefined) return;
  try {
    const response = await fetchObservation(
      observationUrl(base, cleanup.path),
      {
        method: cleanup.method,
        headers: {
          ...requestHeaders,
          ...cleanup.headers,
          "cache-control": "no-cache",
        },
        ...(cleanup.body === undefined ? {} : { body: cleanup.body }),
      },
    );
    await response.body?.cancel();
  } catch {
    // Cleanup continues through the provider authority below.
  }
}
