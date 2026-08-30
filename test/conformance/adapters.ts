import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import { readFile, readdir, stat } from "node:fs/promises";
import { dirname, join, relative, resolve } from "node:path";
import { promisify } from "node:util";

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
  readonly expect: { readonly status: number; readonly json: unknown };
}

export interface PortableFixture {
  readonly id: string;
  readonly root: string;
  readonly source: string;
  readonly sourceSha256: string;
  readonly compatibilityDate: string;
  readonly compatibilityFlags: readonly string[];
  readonly observations: readonly Observation[];
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
  if (!Array.isArray(value)) throw new Error(`${label} must be an array`);
  return value.map((item, index) => string(item, `${label}[${index}]`));
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

export async function loadPortableFixtures(root: string): Promise<PortableFixture[]> {
  const result: PortableFixture[] = [];
  for (const path of (await contracts(root)).sort()) {
    const input = record(JSON.parse(await readFile(path, "utf8")), relative(root, path));
    const fixtureRoot = dirname(path);
    const source = resolve(fixtureRoot, string(input.source, "fixture.source"));
    if (!source.startsWith(`${fixtureRoot}/`) || !(await stat(source)).isFile()) throw new Error("portable fixture source escapes its root");
    const observations = input.observations;
    if (!Array.isArray(observations) || !observations.length) throw new Error("portable fixture has no observations");
    result.push({
      id: string(input.id, "fixture.id"),
      root: fixtureRoot,
      source,
      sourceSha256: createHash("sha256").update(await readFile(source)).digest("hex"),
      compatibilityDate: string(input.compatibilityDate, "fixture.compatibilityDate"),
      compatibilityFlags: strings(input.compatibilityFlags, "fixture.compatibilityFlags"),
      observations: observations.map((raw, index) => {
        const observation = record(raw, `observation ${index}`);
        const expect = record(observation.expect, `observation ${index}.expect`);
        if (!Number.isSafeInteger(expect.status)) throw new Error("observation status must be an integer");
        return {
          method: string(observation.method, `observation ${index}.method`),
          path: string(observation.path, `observation ${index}.path`),
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

export function openComputeProject(fixture: PortableFixture, name: string, endpoint: string, accountId: string): JsonRecord {
  return {
    main: relative(fixture.root, fixture.source),
    name,
    tsconfig: "tsconfig.json",
    compatibilityDate: fixture.compatibilityDate,
    compatibilityFlags: [...fixture.compatibilityFlags],
    vars: {}, secrets: {}, bindings: {}, services: [], accountId, endpoint,
  };
}

export function cloudflareProject(fixture: PortableFixture, name: string, accountId: string): JsonRecord {
  return {
    name,
    main: relative(fixture.root, fixture.source),
    account_id: accountId,
    compatibility_date: fixture.compatibilityDate,
    compatibility_flags: [...fixture.compatibilityFlags],
    workers_dev: true,
    send_metrics: false,
  };
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
      response = await fetch(observationUrl(base, observation.path), {
        method: observation.method,
        redirect: "error",
        signal: AbortSignal.timeout(30_000),
        headers: { ...requestHeaders, "cache-control": "no-cache" },
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
    const expected = JSON.stringify(observation.expect.json);
    if (response.status !== observation.expect.status || JSON.stringify(body) !== expected) {
      throw new Error(`${fixture.id}: ${target} observation differs at ${observation.path}; actual=${JSON.stringify({
        status: response.status,
        json: body,
      })}; expected=${JSON.stringify(observation.expect)}`);
    }
    results.push({ status: response.status, json: body });
  }
  return results;
}
