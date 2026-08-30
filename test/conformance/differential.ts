import { randomBytes } from "node:crypto";
import { cp, mkdir, rm, stat, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  cloudflareDeploymentUrl, cloudflareProject, cloudflareTransientFailure, cloudflareWorkerMissing,
  command, commandStatus,
  loadPortableFixtures, observe, openComputeProject,
  type CommandResult, type JsonRecord, type PortableFixture,
} from "./adapters.ts";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const FIXTURES = join(ROOT, "test/conformance/fixtures");
const fixtures = await loadPortableFixtures(FIXTURES);
const args = process.argv.slice(2);

if (args.length === 1 && args[0] === "--list") {
  process.stdout.write(`${JSON.stringify({ schemaVersion: 1, cases: fixtures.map(fixture => fixture.id) })}\n`);
} else {
  const selected: string[] = [];
  for (let index = 0; index < args.length; index += 2) {
    if (args[index] !== "--case" || args[index + 1] === undefined) throw new Error("use --case <id>");
    selected.push(args[index + 1]!);
  }
  const requested = selected.length ? selected : fixtures.map(fixture => fixture.id);
  const selectedFixtures = requested.map(id => {
    const fixture = fixtures.find(item => item.id === id);
    if (fixture === undefined) throw new Error(`unknown differential fixture: ${id}`);
    return fixture;
  });
  if (new Set(requested).size !== requested.length) throw new Error("duplicate differential fixture");
  const result = await run(selectedFixtures);
  process.stdout.write(`${JSON.stringify(result)}\n`);
  if (result.status !== "passed") process.exitCode = 1;
}

function required(name: string): string {
  const value = process.env[name];
  if (value === undefined || value.length === 0) throw new Error(`${name} is required`);
  return value;
}

function safeAlias(value: string): string {
  if (!/^[a-z0-9][a-z0-9._-]{0,63}$/.test(value)) throw new Error("Cloudflare account alias is invalid");
  return value;
}

function uuid(value: string, label: string): string {
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(value)) {
    throw new Error(`${label} must be a UUIDv7`);
  }
  return value;
}

async function executable(name: string): Promise<string> {
  const path = required(name);
  if (!path.startsWith("/") || !(await stat(path)).isFile()) throw new Error(`${name} must name an absolute regular file`);
  return path;
}

function processEnv(extra: Readonly<Record<string, string>>): Record<string, string> {
  const env: Record<string, string> = {
    ...extra,
    CI: "true",
    WRANGLER_HIDE_BANNER: "true",
    WRANGLER_SEND_METRICS: "false",
  };
  for (const name of ["PATH", "HOME", "TMPDIR", "TMP", "TEMP"]) {
    const value = process.env[name];
    if (value !== undefined) env[name] = value;
  }
  return env;
}

function sanitizedError(error: unknown, secrets: readonly (string | undefined)[]): string {
  let message = error instanceof Error ? error.message : "differential fixture failed";
  for (const secret of secrets) {
    if (secret !== undefined && secret.length > 0) message = message.replaceAll(secret, "[REDACTED]");
  }
  return message.slice(0, 2048);
}

async function run(selected: readonly PortableFixture[]): Promise<JsonRecord> {
  if (required("OPEN_COMPUTE_CF_MUTATION_ACK") !== "p3-cf-diff") throw new Error("Cloudflare mutation acknowledgement is missing");
  if (required("OPEN_COMPUTE_TEST_RUNTIME_RESTART_ACK") !== "restart-generation") {
    throw new Error("test runtime restart acknowledgement is missing");
  }
  const accountId = required("OPEN_COMPUTE_CF_ACCOUNT_ID");
  if (!/^[0-9a-f]{32}$/.test(accountId)) throw new Error("Cloudflare account ID is invalid");
  const accountAlias = safeAlias(required("OPEN_COMPUTE_CF_ACCOUNT_ALIAS"));
  const token = process.env.CLOUDFLARE_API_TOKEN;
  const wrangler = await executable("OPEN_COMPUTE_CF_WRANGLER");
  const platformd = await executable("OPEN_COMPUTE_PLATFORMD");
  const endpoint = new URL(required("OPEN_COMPUTE_ENDPOINT"));
  if (endpoint.pathname !== "/" || endpoint.search || endpoint.hash
      || (endpoint.protocol !== "https:" && !(endpoint.protocol === "http:" && ["127.0.0.1", "localhost", "[::1]"].includes(endpoint.hostname)))) {
    throw new Error("open-compute endpoint must be HTTPS or loopback HTTP origin");
  }
  const openComputeAccount = uuid(required("OPEN_COMPUTE_ACCOUNT_ID"), "open-compute account");
  const adminToken = process.env.OPEN_COMPUTE_ADMIN_TOKEN;
  await verifyOpenComputeAccount(endpoint, openComputeAccount, adminToken);
  const cloudflareEnv = processEnv({
    CLOUDFLARE_ACCOUNT_ID: accountId,
    ...(token === undefined ? {} : { CLOUDFLARE_API_TOKEN: token }),
  });
  const version = await command(wrangler, ["--version"], { cwd: ROOT, env: cloudflareEnv, timeout: 20_000 });
  if (!/(?:^|\s)4\.125\.0(?:\s|$)/.test(`${version.stdout}\n${version.stderr}`)) throw new Error("Wrangler version differs from baseline");
  await verifyWranglerAccount(wrangler, accountId, cloudflareEnv);
  const prefix = `oc-p34-${Date.now().toString(36)}-${randomBytes(4).toString("hex")}`;
  const revision = (await command("git", ["rev-parse", "HEAD"], { cwd: ROOT, env: processEnv({}), timeout: 20_000 })).stdout.trim();
  const plan = {
    schemaVersion: 1,
    phase: "preflight",
    revision,
    accountAlias,
    prefix,
    fixtures: selected.length,
    mutationScope: "one uniquely named workers.dev Worker per selected fixture; no routes or shared bindings",
    cleanup: [
      "Wrangler delete of the exact Worker name with dependency override disabled",
      "open-compute Worker after a test-support runtime-generation restart when safely retained",
    ],
  };
  process.stdout.write(`${JSON.stringify(plan)}\n`);
  const directory = join(process.env.TMPDIR ?? "/tmp", prefix);
  await mkdir(directory, { recursive: false });
  const results: JsonRecord[] = [];
  const cleanup: JsonRecord[] = [];
  let failed: string | undefined;
  try {
    for (let index = 0; index < selected.length; index++) {
      const fixture = selected[index]!;
      const name = `${prefix}-${index}`;
      const projectRoot = join(directory, String(index));
      await cp(fixture.root, projectRoot, { recursive: true });
      const cfConfig = join(projectRoot, "wrangler.json");
      const ocConfig = join(projectRoot, "open-compute.json");
      await writeFile(join(projectRoot, "tsconfig.json"), `${JSON.stringify({
        extends: join(ROOT, "tsconfig.json"),
        compilerOptions: { types: ["@open-compute/workers-types"] },
        include: ["src/**/*.ts"],
      }, null, 2)}\n`, { mode: 0o600 });
      await writeFile(cfConfig, `${JSON.stringify(cloudflareProject(fixture, name, accountId), null, 2)}\n`, { mode: 0o600 });
      await writeFile(ocConfig, `${JSON.stringify(openComputeProject(fixture, name, endpoint.href, openComputeAccount), null, 2)}\n`, { mode: 0o600 });
      let workerId: string | undefined;
      let cloudflareOwned = false;
      try {
        await ensureCloudflareAbsent(name, cfConfig, wrangler, cloudflareEnv);
        cloudflareOwned = true;
        const deployedCloudflare = await command(wrangler, ["deploy", "--config", cfConfig], {
          cwd: projectRoot,
          env: cloudflareEnv,
          timeout: 300_000,
        });
        const oc = await command(process.execPath, [
          join(ROOT, "packages/toolchain/src/bin.ts"), "deploy", "--config", ocConfig,
          "--platformd", platformd, "--endpoint", endpoint.href, "--account", openComputeAccount,
          "--token-env", "OPEN_COMPUTE_ADMIN_TOKEN", "--json",
        ], {
          cwd: ROOT,
          env: processEnv(adminToken === undefined ? {} : { OPEN_COMPUTE_ADMIN_TOKEN: adminToken }),
          timeout: 300_000,
        });
        const deployed = JSON.parse(oc.stdout) as unknown;
        if (deployed === null || typeof deployed !== "object" || typeof Reflect.get(deployed, "workerId") !== "string"
            || typeof Reflect.get(deployed, "url") !== "string") throw new Error("open-compute deployment result is invalid");
        workerId = Reflect.get(deployed, "workerId") as string;
        const openComputeHostname = `${name}.p3-diff.invalid`;
        await createOpenComputeRoute(endpoint, openComputeAccount, workerId, openComputeHostname, adminToken);
        const cloudflareUrl = cloudflareDeploymentUrl(`${deployedCloudflare.stdout}\n${deployedCloudflare.stderr}`, name);
        const cloudflare = await observe(cloudflareUrl, fixture, "cloudflare");
        const openCompute = await observe(endpoint.href, fixture, "open-compute", {
          host: openComputeHostname,
          connection: "close",
        });
        if (JSON.stringify(cloudflare) !== JSON.stringify(openCompute)) throw new Error(`${fixture.id}: normalized observations differ`);
        results.push({ id: fixture.id, status: "passed", sourceSha256: fixture.sourceSha256, cloudflare, openCompute });
      } catch (error) {
        failed = sanitizedError(error, [token, adminToken]);
        results.push({ id: fixture.id, status: "failed", sourceSha256: fixture.sourceSha256, error: failed });
      } finally {
        const cf = cloudflareOwned
          ? await cleanupCloudflare(name, cfConfig, wrangler, cloudflareEnv)
          : { deleted: false, status: "preflight-did-not-prove-ownership" };
        const oc = await cleanupOpenCompute(name, workerId, endpoint, openComputeAccount, adminToken);
        cleanup.push({ id: fixture.id, cloudflare: cf, openCompute: oc });
        if (!cf.deleted || !oc.deleted) failed = `${fixture.id}: cleanup did not prove an empty final resource set`;
      }
      if (failed !== undefined) break;
    }
  } finally {
    await rm(directory, { recursive: true });
  }
  return {
    schemaVersion: 1,
    status: failed === undefined ? "passed" : "failed",
    cases: results.map(item => ({ id: item.id, status: item.status, ...(item.error === undefined ? {} : { error: item.error }) })),
    differential: { revision, accountAlias, prefix, results, cleanup, error: failed },
  };
}

async function createOpenComputeRoute(
  endpoint: URL,
  accountId: string,
  workerId: string,
  hostname: string,
  token: string | undefined,
): Promise<void> {
  const headers: Record<string, string> = {
    "content-type": "application/json",
    "idempotency-key": `p3-cf-diff-route-${randomBytes(8).toString("hex")}`,
  };
  if (token !== undefined) headers.authorization = `Bearer ${token}`;
  const response = await fetch(new URL(`/v1/accounts/${accountId}/workers/${workerId}/routes`, endpoint), {
    method: "POST",
    headers,
    body: JSON.stringify({ hostname, pathPrefix: "/" }),
    signal: AbortSignal.timeout(60_000),
  });
  if (!response.ok) {
    await response.body?.cancel();
    throw new Error("open-compute differential route creation failed");
  }
  const body = await response.json() as unknown;
  const route = body !== null && typeof body === "object" ? Reflect.get(body, "route") : undefined;
  if (route === null || typeof route !== "object" || Reflect.get(route, "workerId") !== workerId
      || Reflect.get(route, "hostnameAscii") !== hostname || Reflect.get(route, "pathPrefix") !== "/") {
    throw new Error("open-compute differential route response is invalid");
  }
}

async function verifyOpenComputeAccount(
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<void> {
  const response = await fetch(new URL("/v1/account", endpoint), {
    headers: token === undefined ? {} : { authorization: `Bearer ${token}` },
    signal: AbortSignal.timeout(30_000),
  });
  if (!response.ok) {
    await response.body?.cancel();
    throw new Error("open-compute account verification failed");
  }
  const identity: unknown = await response.json();
  if (identity === null || typeof identity !== "object" || Reflect.get(identity, "accountId") !== accountId) {
    throw new Error("open-compute account differs from the explicitly selected account");
  }
}

async function verifyWranglerAccount(
  wrangler: string,
  accountId: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  const result = await readOnlyWrangler(wrangler, ["whoami", "--json"], environment);
  if (result.status !== 0) throw new Error("Wrangler account verification failed");
  const identity: unknown = JSON.parse(result.stdout);
  if (identity === null || typeof identity !== "object" || !Array.isArray(Reflect.get(identity, "accounts"))) {
    throw new Error("Wrangler identity response is invalid");
  }
  const accounts = Reflect.get(identity, "accounts") as unknown[];
  if (!accounts.some(account => account !== null && typeof account === "object" && Reflect.get(account, "id") === accountId)) {
    throw new Error("Wrangler is not authenticated for the explicitly selected Cloudflare account");
  }
}

async function ensureCloudflareAbsent(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  const result = await readOnlyWrangler(
    wrangler,
    ["deployments", "list", "--name", name, "--config", config, "--json"],
    environment,
  );
  if (result.status === 0) throw new Error("refusing to overwrite a pre-existing Cloudflare Worker");
  if (!cloudflareWorkerMissing(`${result.stdout}\n${result.stderr}`)) {
    throw new Error("could not prove the unique Cloudflare Worker name was unused");
  }
}

async function cleanupCloudflare(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<JsonRecord> {
  const removed = await commandStatus(wrangler, ["delete", name, "--config", config], {
    cwd: ROOT, env: environment, timeout: 120_000,
  });
  if (removed.status !== 0) {
    return cloudflareWorkerMissing(`${removed.stdout}\n${removed.stderr}`)
      ? { deleted: true, status: "already-absent" }
      : { deleted: false, status: "delete-failed" };
  }
  const verify = await readOnlyWrangler(
    wrangler,
    ["deployments", "list", "--name", name, "--config", config, "--json"],
    environment,
  );
  if (verify.status === 0) return { deleted: false, status: "still-present" };
  return cloudflareWorkerMissing(`${verify.stdout}\n${verify.stderr}`)
    ? { deleted: true, status: "absent" }
    : { deleted: false, status: "verification-failed" };
}

async function readOnlyWrangler(
  wrangler: string,
  args: readonly string[],
  environment: Readonly<Record<string, string>>,
): Promise<CommandResult> {
  let result: CommandResult = { status: -1, stdout: "", stderr: "" };
  for (let attempt = 0; attempt < 3; attempt++) {
    result = await commandStatus(wrangler, args, { cwd: ROOT, env: environment, timeout: 60_000 });
    if (!cloudflareTransientFailure(`${result.stdout}\n${result.stderr}`)) return result;
    if (attempt < 2) await new Promise(resolveDelay => setTimeout(resolveDelay, 250 * (2 ** attempt)));
  }
  return result;
}

async function cleanupOpenCompute(
  name: string,
  knownWorkerId: string | undefined,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<JsonRecord> {
  try {
    const headers: Record<string, string> = token === undefined ? {} : { authorization: `Bearer ${token}` };
    let workerId = knownWorkerId;
    if (workerId === undefined) {
      const listed = await fetch(new URL(`/v1/accounts/${accountId}/workers`, endpoint), { headers, signal: AbortSignal.timeout(30_000) });
      if (!listed.ok) {
        await listed.body?.cancel();
        return { deleted: false, status: listed.status };
      }
      const body = await listed.json() as unknown;
      if (body === null || typeof body !== "object" || !Array.isArray(Reflect.get(body, "workers"))) {
        return { deleted: false, status: "invalid-list" };
      }
      const match = (Reflect.get(body, "workers") as unknown[]).find(item =>
        item !== null && typeof item === "object" && Reflect.get(item, "name") === name && Reflect.get(item, "deletedAtMs") === null);
      if (match !== undefined && match !== null && typeof match === "object") {
        const id: unknown = Reflect.get(match, "id");
        if (typeof id !== "string") return { deleted: false, status: "invalid-worker" };
        workerId = id;
      }
    }
    if (workerId === undefined) return { deleted: true, status: 404 };
    const url = new URL(`/v1/accounts/${accountId}/workers/${workerId}`, endpoint);
    let restarted = false;
    let removed = false;
    for (let attempt = 0; attempt < 2; attempt++) {
      const response = await fetch(url, {
        method: "DELETE",
        headers: { ...headers, "idempotency-key": `p3-cf-diff-delete-${randomBytes(8).toString("hex")}` },
        signal: AbortSignal.timeout(60_000),
      });
      const text = await response.text();
      if (response.ok || response.status === 410) {
        removed = true;
        break;
      }
      if (attempt === 0 && response.status === 409 && platformErrorCode(text) === "DEPLOYMENT_REFERENCED") {
        await restartOpenComputeRuntime(endpoint, headers);
        restarted = true;
        continue;
      }
      return { deleted: false, status: response.status };
    }
    if (!removed) return { deleted: false, status: "delete-failed" };
    const verify = await fetch(new URL(`/v1/accounts/${accountId}/workers`, endpoint), {
      headers,
      signal: AbortSignal.timeout(30_000),
    });
    if (!verify.ok) {
      await verify.body?.cancel();
      return { deleted: false, status: verify.status };
    }
    const body = await verify.json() as unknown;
    if (body === null || typeof body !== "object" || !Array.isArray(Reflect.get(body, "workers"))) {
      return { deleted: false, status: "invalid-list" };
    }
    const present = (Reflect.get(body, "workers") as unknown[]).some(item => item !== null
      && typeof item === "object"
      && (Reflect.get(item, "id") === workerId || Reflect.get(item, "name") === name));
    return { deleted: !present, status: present ? "still-present" : "absent", restarted };
  } catch {
    return { deleted: false, status: "unavailable" };
  }
}

function platformErrorCode(text: string): string | undefined {
  try {
    const body: unknown = JSON.parse(text);
    const error = body !== null && typeof body === "object" ? Reflect.get(body, "error") : undefined;
    const code = error !== null && typeof error === "object" ? Reflect.get(error, "code") : undefined;
    return typeof code === "string" ? code : undefined;
  } catch {
    return undefined;
  }
}

async function restartOpenComputeRuntime(
  endpoint: URL,
  headers: Readonly<Record<string, string>>,
): Promise<void> {
  const before = await openComputeRuntimeStatus(endpoint, headers);
  const restarted = await fetch(new URL("/__test/runtime/restart", endpoint), {
    method: "POST",
    headers: { ...headers, "x-open-compute-test-ack": "restart-generation" },
    signal: AbortSignal.timeout(30_000),
  });
  await restarted.body?.cancel();
  if (restarted.status !== 202) throw new Error("test-support runtime restart was rejected");
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    try {
      const current = await openComputeRuntimeStatus(endpoint, headers);
      if (current.state === "RUNNING" && current.attempt > before.attempt) return;
    } catch {
      // The loopback listener can be briefly unavailable while the child generation rotates.
    }
    await new Promise(resolveDelay => setTimeout(resolveDelay, 100));
  }
  throw new Error("test-support runtime restart did not reach a new running generation");
}

async function openComputeRuntimeStatus(
  endpoint: URL,
  headers: Readonly<Record<string, string>>,
): Promise<{ readonly state: string; readonly attempt: number }> {
  const response = await fetch(new URL("/health/status", endpoint), {
    headers,
    signal: AbortSignal.timeout(10_000),
  });
  if (!response.ok) {
    await response.body?.cancel();
    throw new Error("open-compute runtime status is unavailable");
  }
  const body: unknown = await response.json();
  const supervisor = body !== null && typeof body === "object" ? Reflect.get(body, "supervisor") : undefined;
  const state = supervisor !== null && typeof supervisor === "object" ? Reflect.get(supervisor, "state") : undefined;
  const attempt = supervisor !== null && typeof supervisor === "object" ? Reflect.get(supervisor, "attempt") : undefined;
  if (typeof state !== "string" || typeof attempt !== "number" || !Number.isSafeInteger(attempt)) {
    throw new Error("open-compute runtime status response is invalid");
  }
  return { state, attempt };
}
