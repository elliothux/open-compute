import { createHash } from "node:crypto";
import { MAX_OUTPUT } from "./runtime-contract.ts";
import { fetchObservation, observationUrl } from "./transport.ts";
import type { JsonRecord, PortableFixture } from "./types.ts";

export function canonicalJson(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonicalJson);
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value as JsonRecord)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, item]) => [key, canonicalJson(item)]),
    );
  }
  return value;
}

function firstJsonDifference(
  actual: unknown,
  expected: unknown,
  path = "$",
): string | undefined {
  if (Object.is(actual, expected)) return undefined;
  if (Array.isArray(actual) || Array.isArray(expected)) {
    if (!Array.isArray(actual) || !Array.isArray(expected)) {
      return `${path}: actual=${JSON.stringify(actual)}; expected=${JSON.stringify(expected)}`;
    }
    if (actual.length !== expected.length) {
      return `${path}.length: actual=${actual.length}; expected=${expected.length}`;
    }
    for (let index = 0; index < actual.length; index++) {
      const difference = firstJsonDifference(
        actual[index],
        expected[index],
        `${path}[${index}]`,
      );
      if (difference !== undefined) return difference;
    }
    return undefined;
  }
  if (
    actual !== null &&
    expected !== null &&
    typeof actual === "object" &&
    typeof expected === "object"
  ) {
    const actualRecord = actual as JsonRecord;
    const expectedRecord = expected as JsonRecord;
    const actualKeys = Object.keys(actualRecord).sort();
    const expectedKeys = Object.keys(expectedRecord).sort();
    if (JSON.stringify(actualKeys) !== JSON.stringify(expectedKeys)) {
      const unexpected = actualKeys.find(
        (key) => !Object.hasOwn(expectedRecord, key),
      );
      if (unexpected !== undefined) {
        return `${path}.${unexpected}: unexpected=${JSON.stringify(actualRecord[unexpected]).slice(0, 1024)}`;
      }
      return `${path} keys: actual=${JSON.stringify(actualKeys)}; expected=${JSON.stringify(expectedKeys)}`;
    }
    for (const key of actualKeys) {
      const difference = firstJsonDifference(
        actualRecord[key],
        expectedRecord[key],
        `${path}.${key}`,
      );
      if (difference !== undefined) return difference;
    }
    return undefined;
  }
  return `${path}: actual=${JSON.stringify(actual)}; expected=${JSON.stringify(expected)}`;
}

export async function observe(
  base: string,
  fixture: PortableFixture,
  target: "cloudflare" | "open-compute",
  requestHeaders: Readonly<Record<string, string>> = {},
): Promise<unknown[]> {
  const results: unknown[] = [];
  for (let index = 0; index < fixture.observations.length; index++) {
    const observation = fixture.observations[index]!;
    const activationDeadline = Date.now() + 30_000;
    let activationDelayMs = 250;
    let response: Response;
    let text: string;
    for (;;) {
      response = await fetchObservation(
        observationUrl(base, observation.path),
        {
          method: observation.method,
          headers: {
            ...requestHeaders,
            ...observation.headers,
            "cache-control": "no-cache",
          },
          ...(observation.body === undefined ? {} : { body: observation.body }),
        },
      );
      text = await response.text();
      const activating =
        target === "cloudflare" &&
        index === 0 &&
        response.status === 404 &&
        response.headers.get("content-type")?.startsWith("text/html") === true;
      if (!activating || Date.now() >= activationDeadline) break;
      await new Promise((resolveDelay) =>
        setTimeout(resolveDelay, activationDelayMs),
      );
      activationDelayMs = Math.min(activationDelayMs * 2, 2_000);
    }
    if (text.length > MAX_OUTPUT)
      throw new Error(`${fixture.id}: response exceeds 1 MiB`);
    let body: unknown;
    try {
      body = JSON.parse(text);
    } catch {
      const preview = text.slice(0, 160).replaceAll(/\s+/g, " ");
      throw new Error(
        `${fixture.id}: ${target} response is not JSON at ${observation.path}; status=${response.status}; content-type=${response.headers.get("content-type") ?? "missing"}; sha256=${createHash("sha256").update(text).digest("hex")}; preview=${preview}`,
      );
    }
    const normalizedBody = canonicalJson(body);
    const normalizedExpected = canonicalJson(observation.expect.json);
    if (
      response.status !== observation.expect.status ||
      JSON.stringify(normalizedBody) !== JSON.stringify(normalizedExpected)
    ) {
      const difference =
        response.status === observation.expect.status
          ? firstJsonDifference(normalizedBody, normalizedExpected)
          : `$.status: actual=${response.status}; expected=${observation.expect.status}; body=${JSON.stringify(normalizedBody).slice(0, 1024)}`;
      throw new Error(
        `${fixture.id}: ${target} observation differs at ${observation.path}; ${difference ?? "unknown difference"}`,
      );
    }
    results.push({ status: response.status, json: normalizedBody });
  }
  return results;
}
