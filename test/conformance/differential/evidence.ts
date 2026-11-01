import { appendFile } from "node:fs/promises";
import { fetchObservation, observationUrl } from "../adapters/transport.ts";
import type { JsonRecord, PortableFixture } from "../adapters/types.ts";
import { sourceIdentity as trackedSourceIdentity } from "../checks/context.ts";

export async function sourceIdentity(): Promise<string> {
  return trackedSourceIdentity();
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
