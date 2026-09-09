import { createHash } from "node:crypto";
import { readdir, readFile, stat } from "node:fs/promises";
import { dirname, join, relative, resolve } from "node:path";
import type { JsonRecord, PortableBinding, PortableFixture } from "./types.ts";

function record(value: unknown, label: string): JsonRecord {
  if (value === null || typeof value !== "object" || Array.isArray(value))
    throw new Error(`${label} must be an object`);
  return value as JsonRecord;
}

function string(value: unknown, label: string): string {
  if (typeof value !== "string" || value.length === 0)
    throw new Error(`${label} must be a string`);
  return value;
}

function strings(value: unknown, label: string): string[] {
  if (
    !Array.isArray(value) ||
    value.some((item) => typeof item !== "string" || item.length === 0)
  ) {
    throw new Error(`${label} must be a non-empty string array`);
  }
  const result = value as string[];
  if (!result.length || new Set(result).size !== result.length)
    throw new Error(`${label} is empty or ambiguous`);
  return result;
}

function exactKeys(
  value: JsonRecord,
  allowed: readonly string[],
  label: string,
): void {
  const unexpected = Object.keys(value).filter((key) => !allowed.includes(key));
  if (unexpected.length)
    throw new Error(
      `${label} contains unsupported fields: ${unexpected.sort().join(", ")}`,
    );
}

async function contracts(root: string): Promise<string[]> {
  const result: string[] = [];
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const path = join(root, entry.name);
    if (entry.isDirectory()) result.push(...(await contracts(path)));
    else if (entry.isFile() && entry.name === "contract.json")
      result.push(path);
  }
  return result;
}

async function fixtureDigest(root: string): Promise<string> {
  const files: string[] = [];
  const visit = async (directory: string): Promise<void> => {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) await visit(path);
      else if (entry.isFile()) files.push(path);
      else
        throw new Error(
          "portable fixture contains a non-regular filesystem entry",
        );
    }
  };
  await visit(root);
  const digest = createHash("sha256");
  for (const path of files.sort()) {
    digest.update(relative(root, path));
    digest.update("\0");
    digest.update(await readFile(path));
  }
  return digest.digest("hex");
}

function observationBody(
  value: unknown,
  label: string,
): Uint8Array | undefined {
  if (value === undefined) return undefined;
  const body = record(value, label);
  exactKeys(body, ["text", "base64", "json"], label);
  const selected = ["text", "base64", "json"].filter((key) => key in body);
  if (selected.length !== 1)
    throw new Error(`${label} must choose exactly one encoding`);
  if (selected[0] === "text")
    return new TextEncoder().encode(string(body.text, `${label}.text`));
  if (selected[0] === "base64") {
    const encoded = string(body.base64, `${label}.base64`);
    const bytes = Buffer.from(encoded, "base64");
    if (bytes.toString("base64") !== encoded)
      throw new Error(`${label}.base64 is not canonical`);
    return bytes;
  }
  return new TextEncoder().encode(JSON.stringify(body.json));
}

function stringRecord(
  value: unknown,
  label: string,
): Readonly<Record<string, string>> {
  if (value === undefined) return {};
  const input = record(value, label);
  const result: Record<string, string> = {};
  for (const [key, item] of Object.entries(input)) {
    if (!/^[a-z0-9-]+$/.test(key) || typeof item !== "string")
      throw new Error(`${label} is invalid`);
    result[key] = item;
  }
  return result;
}

export async function loadPortableFixtures(
  root: string,
): Promise<PortableFixture[]> {
  const result: PortableFixture[] = [];
  for (const path of (await contracts(root)).sort()) {
    const input = record(
      JSON.parse(await readFile(path, "utf8")),
      relative(root, path),
    );
    exactKeys(
      input,
      [
        "schemaVersion",
        "id",
        "contracts",
        "source",
        "bindings",
        "observations",
        "normalization",
        "cleanup",
      ],
      "fixture",
    );
    if (input.schemaVersion !== 1)
      throw new Error("portable fixture schema version is unsupported");
    const fixtureRoot = dirname(path);
    const source = resolve(fixtureRoot, string(input.source, "fixture.source"));
    if (!source.startsWith(`${fixtureRoot}/`) || !(await stat(source)).isFile())
      throw new Error("portable fixture source escapes its root");
    const observations = input.observations;
    if (!Array.isArray(observations) || !observations.length)
      throw new Error("portable fixture has no observations");
    const rawBindings = record(input.bindings, "fixture.bindings");
    if (Object.keys(rawBindings).length > 16)
      throw new Error("portable fixture has too many bindings");
    const bindings: Record<string, PortableBinding> = {};
    for (const [name, raw] of Object.entries(rawBindings)) {
      if (!/^[A-Za-z_][A-Za-z0-9_]{0,63}$/.test(name))
        throw new Error("portable fixture binding name is invalid");
      const binding = record(raw, `fixture.bindings.${name}`);
      const classBound =
        binding.type === "do_namespace" || binding.type === "workflow";
      exactKeys(
        binding,
        classBound ? ["type", "className", "schedules"] : ["type"],
        `fixture.bindings.${name}`,
      );
      if (
        binding.type !== "kv_namespace" &&
        binding.type !== "d1_database" &&
        binding.type !== "r2_bucket" &&
        binding.type !== "do_namespace" &&
        binding.type !== "queue_producer" &&
        binding.type !== "workflow" &&
        binding.type !== "worker_loader"
      ) {
        throw new Error("portable fixture binding type is unsupported");
      }
      if (classBound) {
        const className = string(
          binding.className,
          `fixture.bindings.${name}.className`,
        );
        if (!/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(className)) {
          throw new Error("portable fixture binding class name is invalid");
        }
        const schedules =
          binding.schedules === undefined
            ? undefined
            : strings(binding.schedules, `fixture.bindings.${name}.schedules`);
        if (schedules !== undefined && binding.type !== "workflow") {
          throw new Error("only Workflow bindings accept direct schedules");
        }
        bindings[name] = {
          type: binding.type as "do_namespace" | "workflow",
          className,
          ...(schedules === undefined ? {} : { schedules }),
        };
      } else {
        bindings[name] = {
          type: binding.type as
            | "kv_namespace"
            | "d1_database"
            | "r2_bucket"
            | "queue_producer"
            | "worker_loader",
        };
      }
    }
    if (
      !Array.isArray(input.normalization) ||
      input.normalization.length !== 0
    ) {
      throw new Error(
        "portable fixture normalization rules are not implemented",
      );
    }
    const cleanup = record(input.cleanup, "fixture.cleanup");
    exactKeys(cleanup, ["cloudflare", "openCompute"], "fixture.cleanup");
    const expectedCleanup = [
      "worker",
      ...new Set(
        Object.values(bindings)
          .filter((binding) => binding.type !== "worker_loader")
          .map((binding) => binding.type),
      ),
    ];
    if (
      JSON.stringify(cleanup.cloudflare) !== JSON.stringify(expectedCleanup) ||
      JSON.stringify(cleanup.openCompute) !== JSON.stringify(expectedCleanup)
    ) {
      throw new Error(
        "portable fixture cleanup does not match its provisioned resources",
      );
    }
    result.push({
      id: string(input.id, "fixture.id"),
      root: fixtureRoot,
      source,
      sourceSha256: await fixtureDigest(fixtureRoot),
      contracts: strings(input.contracts, "fixture.contracts"),
      bindings,
      observations: observations.map((raw, index) => {
        const observation = record(raw, `observation ${index}`);
        exactKeys(
          observation,
          ["method", "path", "headers", "body", "expect"],
          `observation ${index}`,
        );
        const expect = record(
          observation.expect,
          `observation ${index}.expect`,
        );
        exactKeys(expect, ["status", "json"], `observation ${index}.expect`);
        if (!Number.isSafeInteger(expect.status))
          throw new Error("observation status must be an integer");
        const body = observationBody(
          observation.body,
          `observation ${index}.body`,
        );
        return {
          method: string(observation.method, `observation ${index}.method`),
          path: string(observation.path, `observation ${index}.path`),
          headers: stringRecord(
            observation.headers,
            `observation ${index}.headers`,
          ),
          ...(body === undefined ? {} : { body }),
          expect: { status: expect.status as number, json: expect.json },
        };
      }),
    });
  }
  const ids = result.map((fixture) => fixture.id);
  if (!ids.length || new Set(ids).size !== ids.length)
    throw new Error("portable fixture inventory is empty or ambiguous");
  return result;
}
