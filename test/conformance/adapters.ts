import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { readFile, readdir, stat } from "node:fs/promises";
import { request as requestHttp } from "node:http";
import { request as requestHttps } from "node:https";
import { dirname, join, relative, resolve } from "node:path";
import { promisify } from "node:util";

const LOCK = JSON.parse(readFileSync(new URL("../../packages/runtime/workerd.lock.json", import.meta.url), "utf8")) as {
  effectiveCompatibilityDate: string;
  requiredCompatibilityFlags: string[];
  workersSdk: { wranglerVersion: string };
};

/** Wrangler version coordinated with the formal workerd/workers-types baseline. */
export const WRANGLER_VERSION = LOCK.workersSdk.wranglerVersion;

const executeFile = promisify(execFile);
const MAX_OUTPUT = 1024 * 1024;
export type JsonRecord = Record<string, unknown>;

export interface CommandResult {
  readonly status: number;
  readonly stdout: string;
  readonly stderr: string;
}

export interface Observation {
  readonly method: string;
  readonly path: string;
  readonly headers: Readonly<Record<string, string>>;
  readonly body?: Uint8Array;
  readonly expect: { readonly status: number; readonly json: unknown };
}

export interface PortableFixture {
  readonly id: string;
  readonly root: string;
  readonly source: string;
  readonly sourceSha256: string;
  readonly contracts: readonly string[];
  readonly bindings: Readonly<Record<string, PortableBinding>>;
  readonly observations: readonly Observation[];
}

export type PortableBinding =
  | { readonly type: "kv_namespace" | "d1_database" | "r2_bucket" | "queue_producer" }
  | { readonly type: "do_namespace" | "workflow"; readonly className: string;
      readonly schedules?: readonly string[] };

function isClassBinding(
  binding: PortableBinding,
): binding is Extract<PortableBinding, { readonly type: "do_namespace" | "workflow" }> {
  return binding.type === "do_namespace" || binding.type === "workflow";
}

function record(value: unknown, label: string): JsonRecord {
  if (value === null || typeof value !== "object" || Array.isArray(value)) throw new Error(`${label} must be an object`);
  return value as JsonRecord;
}

function string(value: unknown, label: string): string {
  if (typeof value !== "string" || value.length === 0) throw new Error(`${label} must be a string`);
  return value;
}

function strings(value: unknown, label: string): string[] {
  if (!Array.isArray(value) || value.some(item => typeof item !== "string" || item.length === 0)) {
    throw new Error(`${label} must be a non-empty string array`);
  }
  const result = value as string[];
  if (!result.length || new Set(result).size !== result.length) throw new Error(`${label} is empty or ambiguous`);
  return result;
}

function exactKeys(value: JsonRecord, allowed: readonly string[], label: string): void {
  const unexpected = Object.keys(value).filter(key => !allowed.includes(key));
  if (unexpected.length) throw new Error(`${label} contains unsupported fields: ${unexpected.sort().join(", ")}`);
}

async function contracts(root: string): Promise<string[]> {
  const result: string[] = [];
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const path = join(root, entry.name);
    if (entry.isDirectory()) result.push(...await contracts(path));
    else if (entry.isFile() && entry.name === "contract.json") result.push(path);
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
      else throw new Error("portable fixture contains a non-regular filesystem entry");
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

function observationBody(value: unknown, label: string): Uint8Array | undefined {
  if (value === undefined) return undefined;
  const body = record(value, label);
  exactKeys(body, ["text", "base64", "json"], label);
  const selected = ["text", "base64", "json"].filter(key => key in body);
  if (selected.length !== 1) throw new Error(`${label} must choose exactly one encoding`);
  if (selected[0] === "text") return new TextEncoder().encode(string(body.text, `${label}.text`));
  if (selected[0] === "base64") {
    const encoded = string(body.base64, `${label}.base64`);
    const bytes = Buffer.from(encoded, "base64");
    if (bytes.toString("base64") !== encoded) throw new Error(`${label}.base64 is not canonical`);
    return bytes;
  }
  return new TextEncoder().encode(JSON.stringify(body.json));
}

function stringRecord(value: unknown, label: string): Readonly<Record<string, string>> {
  if (value === undefined) return {};
  const input = record(value, label);
  const result: Record<string, string> = {};
  for (const [key, item] of Object.entries(input)) {
    if (!/^[a-z0-9-]+$/.test(key) || typeof item !== "string") throw new Error(`${label} is invalid`);
    result[key] = item;
  }
  return result;
}

export async function loadPortableFixtures(root: string): Promise<PortableFixture[]> {
  const result: PortableFixture[] = [];
  for (const path of (await contracts(root)).sort()) {
    const input = record(JSON.parse(await readFile(path, "utf8")), relative(root, path));
    exactKeys(input, ["schemaVersion", "id", "contracts", "source", "bindings", "observations", "normalization", "cleanup"], "fixture");
    if (input.schemaVersion !== 1) throw new Error("portable fixture schema version is unsupported");
    const fixtureRoot = dirname(path);
    const source = resolve(fixtureRoot, string(input.source, "fixture.source"));
    if (!source.startsWith(`${fixtureRoot}/`) || !(await stat(source)).isFile()) throw new Error("portable fixture source escapes its root");
    const observations = input.observations;
    if (!Array.isArray(observations) || !observations.length) throw new Error("portable fixture has no observations");
    const rawBindings = record(input.bindings, "fixture.bindings");
    if (Object.keys(rawBindings).length > 16) throw new Error("portable fixture has too many bindings");
    const bindings: Record<string, PortableBinding> = {};
    for (const [name, raw] of Object.entries(rawBindings)) {
      if (!/^[A-Za-z_][A-Za-z0-9_]{0,63}$/.test(name)) throw new Error("portable fixture binding name is invalid");
      const binding = record(raw, `fixture.bindings.${name}`);
      const classBound = binding.type === "do_namespace" || binding.type === "workflow";
      exactKeys(binding, classBound ? ["type", "className", "schedules"] : ["type"], `fixture.bindings.${name}`);
      if (binding.type !== "kv_namespace" && binding.type !== "d1_database" && binding.type !== "r2_bucket"
          && binding.type !== "do_namespace" && binding.type !== "queue_producer" && binding.type !== "workflow") {
        throw new Error("portable fixture binding type is unsupported");
      }
      if (classBound) {
        const className = string(binding.className, `fixture.bindings.${name}.className`);
        if (!/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(className)) {
          throw new Error("portable fixture binding class name is invalid");
        }
        const schedules = binding.schedules === undefined ? undefined : strings(
          binding.schedules,
          `fixture.bindings.${name}.schedules`,
        );
        if (schedules !== undefined && binding.type !== "workflow") {
          throw new Error("only Workflow bindings accept direct schedules");
        }
        bindings[name] = {
          type: binding.type as "do_namespace" | "workflow",
          className,
          ...(schedules === undefined ? {} : { schedules }),
        };
      } else {
        bindings[name] = { type: binding.type as "kv_namespace" | "d1_database" | "r2_bucket" | "queue_producer" };
      }
    }
    if (!Array.isArray(input.normalization) || input.normalization.length !== 0) {
      throw new Error("portable fixture normalization rules are not implemented");
    }
    const cleanup = record(input.cleanup, "fixture.cleanup");
    exactKeys(cleanup, ["cloudflare", "openCompute"], "fixture.cleanup");
    const expectedCleanup = ["worker", ...new Set(Object.values(bindings).map(binding => binding.type))];
    if (JSON.stringify(cleanup.cloudflare) !== JSON.stringify(expectedCleanup)
        || JSON.stringify(cleanup.openCompute) !== JSON.stringify(expectedCleanup)) {
      throw new Error("portable fixture cleanup does not match its provisioned resources");
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
        exactKeys(observation, ["method", "path", "headers", "body", "expect"], `observation ${index}`);
        const expect = record(observation.expect, `observation ${index}.expect`);
        exactKeys(expect, ["status", "json"], `observation ${index}.expect`);
        if (!Number.isSafeInteger(expect.status)) throw new Error("observation status must be an integer");
        const body = observationBody(observation.body, `observation ${index}.body`);
        return {
          method: string(observation.method, `observation ${index}.method`),
          path: string(observation.path, `observation ${index}.path`),
          headers: stringRecord(observation.headers, `observation ${index}.headers`),
          ...(body === undefined ? {} : { body }),
          expect: { status: expect.status as number, json: expect.json },
        };
      }),
    });
  }
  const ids = result.map(fixture => fixture.id);
  if (!ids.length || new Set(ids).size !== ids.length) throw new Error("portable fixture inventory is empty or ambiguous");
  return result;
}

export async function command(
  executable: string,
  args: readonly string[],
  options: { readonly cwd: string; readonly env: Readonly<Record<string, string>>; readonly timeout: number },
): Promise<{ stdout: string; stderr: string }> {
  const result = await commandStatus(executable, args, options);
  if (result.status !== 0) {
    throw new Error(`external command failed; stdout=${result.stdout.slice(0, 512)}; stderr=${result.stderr.slice(0, 512)}`);
  }
  return result;
}

export async function commandStatus(
  executable: string,
  args: readonly string[],
  options: { readonly cwd: string; readonly env: Readonly<Record<string, string>>; readonly timeout: number },
): Promise<CommandResult> {
  try {
    const result = await executeFile(executable, [...args], {
      cwd: options.cwd,
      env: options.env,
      timeout: options.timeout,
      maxBuffer: MAX_OUTPUT,
      encoding: "utf8",
    });
    return { status: 0, stdout: result.stdout, stderr: result.stderr };
  } catch (error) {
    if (error !== null && typeof error === "object") {
      const stderr: unknown = Reflect.get(error, "stderr");
      const stdout: unknown = Reflect.get(error, "stdout");
      const code: unknown = Reflect.get(error, "code");
      return {
        status: typeof code === "number" ? code : -1,
        stdout: typeof stdout === "string" ? stdout : "",
        stderr: typeof stderr === "string" ? stderr : "",
      };
    }
    return { status: -1, stdout: "", stderr: "" };
  }
}

export function cloudflareDeploymentUrl(output: string, workerName: string): string {
  if (!/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(workerName)) {
    throw new Error("Cloudflare Worker name is invalid");
  }
  const plain = output.replaceAll(/\u001b\[[0-9;]*m/g, "");
  const urls = [...plain.matchAll(/https:\/\/[a-z0-9.-]+\.workers\.dev(?:\/[^\s]*)?/gi)]
    .map(match => match[0])
    .filter(candidate => {
      const url = new URL(candidate);
      return url.hostname.startsWith(`${workerName}.`) && url.hostname.endsWith(".workers.dev");
    })
    .map(candidate => new URL(candidate).origin)
    .filter((candidate, index, values) => values.indexOf(candidate) === index);
  if (urls.length !== 1) throw new Error("Wrangler did not report one unambiguous workers.dev URL");
  return `${urls[0]}/`;
}

export function cloudflareWorkerMissing(output: string): boolean {
  return /\[\s*code:\s*(?:10007|10090)\s*\]/i.test(output)
    || /(?:Worker|script)[^\n]{0,80}not found/i.test(output);
}

export function cloudflareTransientFailure(output: string): boolean {
  return /(?:fetch failed|connectivity issue|network connectivity problems)/i.test(output);
}

export function observationUrl(base: string, path: string): string {
  if (!path.startsWith("/") || path.startsWith("//")) throw new Error("observation path must be origin-relative");
  const root = new URL(base);
  if (!root.pathname.endsWith("/")) root.pathname += "/";
  const result = new URL(path.slice(1), root);
  if (result.origin !== root.origin || !result.pathname.startsWith(root.pathname)) {
    throw new Error("observation path escapes its Worker route");
  }
  return result.href;
}

/** Issue a portable observation while preserving the explicit local route Host header. */
export async function fetchObservation(
  url: string,
  init: { readonly method: string; readonly headers: Readonly<Record<string, string>>;
    readonly body?: Uint8Array },
): Promise<Response> {
  if (!("host" in init.headers)) {
    return fetch(url, {
      method: init.method,
      redirect: "error",
      signal: AbortSignal.timeout(30_000),
      headers: init.headers,
      ...(init.body === undefined ? {} : { body: init.body }),
    });
  }
  const target = new URL(url);
  if (target.protocol !== "http:" && target.protocol !== "https:") {
    throw new Error("observation URL protocol is unsupported");
  }
  return new Promise<Response>((resolveResponse, rejectResponse) => {
    const request = (target.protocol === "https:" ? requestHttps : requestHttp)(target, {
      method: init.method,
      headers: init.headers,
      signal: AbortSignal.timeout(30_000),
    }, incoming => {
      const chunks: Buffer[] = [];
      let length = 0;
      incoming.on("data", (chunk: Buffer) => {
        length += chunk.length;
        if (length > MAX_OUTPUT) {
          incoming.destroy(new Error("observation response exceeds 1 MiB"));
          return;
        }
        chunks.push(chunk);
      });
      incoming.once("error", rejectResponse);
      incoming.once("end", () => {
        const status = incoming.statusCode ?? 0;
        if (status < 200 || status > 599) {
          rejectResponse(new Error("observation response status is invalid"));
          return;
        }
        if (status >= 300 && status < 400) {
          rejectResponse(new Error("observation redirect is forbidden"));
          return;
        }
        const headers = new Headers();
        for (const [name, value] of Object.entries(incoming.headers)) {
          if (Array.isArray(value)) value.forEach(item => headers.append(name, item));
          else if (value !== undefined) headers.set(name, value);
        }
        const withoutBody = status === 204 || status === 205 || status === 304;
        resolveResponse(new Response(withoutBody ? null : Buffer.concat(chunks), { status, headers }));
      });
    });
    request.once("error", rejectResponse);
    request.end(init.body === undefined ? undefined : Buffer.from(init.body));
  });
}

function exactBindingIds(
  fixture: PortableFixture,
  ids: Readonly<Record<string, string>>,
): Readonly<Record<string, string>> {
  const expected = Object.keys(fixture.bindings)
    .filter(binding => fixture.bindings[binding]?.type === "kv_namespace"
      || fixture.bindings[binding]?.type === "d1_database")
    .sort();
  const actual = Object.keys(ids).sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected)
      || Object.values(ids).some(id => id.length === 0)) throw new Error("portable fixture binding identities are incomplete");
  return ids;
}

export function openComputeProject(
  fixture: PortableFixture,
  name: string,
  accountId: string,
  bindingIds: Readonly<Record<string, string>> = {},
  bindingNames: Readonly<Record<string, string>> = {},
): JsonRecord {
  return workerProject(fixture, name, accountId, bindingIds, bindingNames, false);
}

/** Minimal open-compute Wrangler project used before owned bindings are provisioned. */
export function openComputeBaseProject(fixture: PortableFixture, name: string, accountId: string): JsonRecord {
  return baseProject(fixture, name, accountId, false);
}

/** Minimal Worker project used before any owned binding has been provisioned. */
export function cloudflareBaseProject(fixture: PortableFixture, name: string, accountId: string): JsonRecord {
  return baseProject(fixture, name, accountId, true);
}

function baseProject(
  fixture: PortableFixture,
  name: string,
  accountId: string,
  workersDev: boolean,
): JsonRecord {
  return {
    name,
    main: relative(fixture.root, fixture.source),
    account_id: accountId,
    compatibility_date: LOCK.effectiveCompatibilityDate,
    compatibility_flags: [...LOCK.requiredCompatibilityFlags],
    workers_dev: workersDev,
    send_metrics: false,
  };
}

export function cloudflareProject(
  fixture: PortableFixture,
  name: string,
  accountId: string,
  bindingIds: Readonly<Record<string, string>> = {},
  bindingNames: Readonly<Record<string, string>> = {},
): JsonRecord {
  return workerProject(fixture, name, accountId, bindingIds, bindingNames, true);
}

function workerProject(
  fixture: PortableFixture,
  name: string,
  accountId: string,
  bindingIds: Readonly<Record<string, string>>,
  bindingNames: Readonly<Record<string, string>>,
  workersDev: boolean,
): JsonRecord {
  const ids = exactBindingIds(fixture, bindingIds);
  const expectedNames = Object.keys(fixture.bindings)
    .filter(binding => fixture.bindings[binding]?.type === "d1_database"
      || fixture.bindings[binding]?.type === "r2_bucket"
      || fixture.bindings[binding]?.type === "queue_producer"
      || fixture.bindings[binding]?.type === "workflow")
    .sort();
  const actualNames = Object.keys(bindingNames).sort();
  if (JSON.stringify(expectedNames) !== JSON.stringify(actualNames)
      || Object.values(bindingNames).some(bindingName => bindingName.length === 0)) {
    throw new Error("portable fixture named Cloudflare bindings are incomplete");
  }
  const durableBindings = Object.entries(fixture.bindings)
    .filter((entry): entry is [string, Extract<PortableBinding, { readonly type: "do_namespace" | "workflow" }>] =>
      isClassBinding(entry[1]) && entry[1].type === "do_namespace")
    .map(([binding, value]) => ({ name: binding, class_name: value.className }));
  const queueBindings = Object.entries(fixture.bindings)
    .filter(([, value]) => value.type === "queue_producer")
    .map(([binding]) => ({ binding, queue: bindingNames[binding] }));
  const workflowBindings = Object.entries(fixture.bindings)
    .filter((entry): entry is [string, Extract<PortableBinding, { readonly type: "do_namespace" | "workflow" }>] =>
      isClassBinding(entry[1]) && entry[1].type === "workflow")
    .map(([binding, value]) => ({
      binding,
      name: bindingNames[binding],
      class_name: value.className,
      ...(value.schedules === undefined ? {} : { schedules: value.schedules }),
    }));
  return {
    ...baseProject(fixture, name, accountId, workersDev),
    kv_namespaces: Object.entries(fixture.bindings)
      .filter(([, value]) => value.type === "kv_namespace")
      .map(([binding]) => ({ binding, id: ids[binding] })),
    d1_databases: Object.entries(fixture.bindings)
      .filter(([, value]) => value.type === "d1_database")
      .map(([binding]) => ({
        binding,
        database_id: ids[binding],
        database_name: bindingNames[binding],
      })),
    r2_buckets: Object.entries(fixture.bindings)
      .filter(([, value]) => value.type === "r2_bucket")
      .map(([binding]) => ({ binding, bucket_name: bindingNames[binding] })),
    ...(durableBindings.length === 0 ? {} : {
      durable_objects: { bindings: durableBindings },
      migrations: [{
        tag: "v1",
        new_sqlite_classes: [...new Set(durableBindings.map(value => value.class_name))],
      }],
    }),
    ...(queueBindings.length === 0 ? {} : { queues: { producers: queueBindings } }),
    ...(workflowBindings.length === 0 ? {} : { workflows: workflowBindings }),
  };
}

/** Return JSON with recursively sorted object keys for stable cross-provider comparison. */
export function canonicalJson(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonicalJson);
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(Object.entries(value as JsonRecord)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, item]) => [key, canonicalJson(item)]));
  }
  return value;
}

function firstJsonDifference(actual: unknown, expected: unknown, path = "$"): string | undefined {
  if (Object.is(actual, expected)) return undefined;
  if (Array.isArray(actual) || Array.isArray(expected)) {
    if (!Array.isArray(actual) || !Array.isArray(expected)) {
      return `${path}: actual=${JSON.stringify(actual)}; expected=${JSON.stringify(expected)}`;
    }
    if (actual.length !== expected.length) {
      return `${path}.length: actual=${actual.length}; expected=${expected.length}`;
    }
    for (let index = 0; index < actual.length; index++) {
      const difference = firstJsonDifference(actual[index], expected[index], `${path}[${index}]`);
      if (difference !== undefined) return difference;
    }
    return undefined;
  }
  if (actual !== null && expected !== null && typeof actual === "object" && typeof expected === "object") {
    const actualRecord = actual as JsonRecord;
    const expectedRecord = expected as JsonRecord;
    const actualKeys = Object.keys(actualRecord).sort();
    const expectedKeys = Object.keys(expectedRecord).sort();
    if (JSON.stringify(actualKeys) !== JSON.stringify(expectedKeys)) {
      const unexpected = actualKeys.find(key => !Object.hasOwn(expectedRecord, key));
      if (unexpected !== undefined) {
        return `${path}.${unexpected}: unexpected=${JSON.stringify(actualRecord[unexpected]).slice(0, 1024)}`;
      }
      return `${path} keys: actual=${JSON.stringify(actualKeys)}; expected=${JSON.stringify(expectedKeys)}`;
    }
    for (const key of actualKeys) {
      const difference = firstJsonDifference(actualRecord[key], expectedRecord[key], `${path}.${key}`);
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
      response = await fetchObservation(observationUrl(base, observation.path), {
        method: observation.method,
        headers: { ...requestHeaders, ...observation.headers, "cache-control": "no-cache" },
        ...(observation.body === undefined ? {} : { body: observation.body }),
      });
      text = await response.text();
      const activating = target === "cloudflare" && index === 0 && response.status === 404
        && response.headers.get("content-type")?.startsWith("text/html") === true;
      if (!activating || Date.now() >= activationDeadline) break;
      await new Promise(resolveDelay => setTimeout(resolveDelay, activationDelayMs));
      activationDelayMs = Math.min(activationDelayMs * 2, 2_000);
    }
    if (text.length > MAX_OUTPUT) throw new Error(`${fixture.id}: response exceeds 1 MiB`);
    let body: unknown;
    try { body = JSON.parse(text); } catch {
      const preview = text.slice(0, 160).replaceAll(/\s+/g, " ");
      throw new Error(`${fixture.id}: ${target} response is not JSON at ${observation.path}; status=${response.status}; content-type=${response.headers.get("content-type") ?? "missing"}; sha256=${createHash("sha256").update(text).digest("hex")}; preview=${preview}`);
    }
    const normalizedBody = canonicalJson(body);
    const normalizedExpected = canonicalJson(observation.expect.json);
    if (response.status !== observation.expect.status
        || JSON.stringify(normalizedBody) !== JSON.stringify(normalizedExpected)) {
      const difference = response.status === observation.expect.status
        ? firstJsonDifference(normalizedBody, normalizedExpected)
        : `$.status: actual=${response.status}; expected=${observation.expect.status}; body=${JSON.stringify(normalizedBody).slice(0, 1024)}`;
      throw new Error(`${fixture.id}: ${target} observation differs at ${observation.path}; ${difference ?? "unknown difference"}`);
    }
    results.push({ status: response.status, json: normalizedBody });
  }
  return results;
}
