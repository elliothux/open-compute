import { randomBytes } from "node:crypto";
import { createHash } from "node:crypto";
import { appendFile, cp, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  cloudflareBaseProject, cloudflareDeploymentUrl, cloudflareProject, cloudflareTransientFailure, cloudflareWorkerMissing,
  command, commandStatus, fetchObservation,
  loadPortableFixtures, observe, observationUrl, openComputeBaseProject, openComputeProject,
  WRANGLER_VERSION,
  type CommandResult, type JsonRecord, type PortableFixture,
} from "./adapters.ts";
import {
  cleanupQueue, cleanupWorkflow, createQueue, ensureQueueAbsent, ensureWorkflowAbsent,
  verifyWorkflowCreated,
} from "./differential-product-resources.ts";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const FIXTURES = join(ROOT, "test/conformance/fixtures");
const fixtures = await loadPortableFixtures(FIXTURES);
const args = process.argv.slice(2);

interface OwnedKvNamespace {
  readonly binding: string;
  readonly name: string;
  cloudflareAbsent: boolean;
  cloudflareOwned: boolean;
  cloudflareId?: string;
  openComputeAbsent: boolean;
  openComputeOwned: boolean;
  openComputeId?: string;
}

interface OwnedD1Database {
  readonly binding: string;
  readonly name: string;
  cloudflareAbsent: boolean;
  cloudflareOwned: boolean;
  cloudflareId?: string;
  openComputeAbsent: boolean;
  openComputeOwned: boolean;
  openComputeId?: string;
}

interface OwnedR2Bucket {
  readonly binding: string;
  readonly name: string;
  cloudflareAbsent: boolean;
  cloudflareOwned: boolean;
  openComputeAbsent: boolean;
  openComputeOwned: boolean;
}

interface OwnedQueue {
  readonly binding: string;
  readonly name: string;
  cloudflareAbsent: boolean;
  cloudflareOwned: boolean;
  openComputeAbsent: boolean;
  openComputeOwned: boolean;
}

interface OwnedDurableObjectNamespace {
  readonly binding: string;
  readonly className: string;
  openComputeOwned: boolean;
}

interface OwnedWorkflow {
  readonly binding: string;
  readonly className: string;
  readonly name: string;
  cloudflareAbsent: boolean;
  cloudflareOwned: boolean;
  openComputeAbsent: boolean;
  openComputeOwned: boolean;
}

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
  const accountId = required("OPEN_COMPUTE_CF_ACCOUNT_ID");
  if (!/^[0-9a-f]{32}$/.test(accountId)) throw new Error("Cloudflare account ID is invalid");
  const accountAlias = safeAlias(required("OPEN_COMPUTE_CF_ACCOUNT_ALIAS"));
  const token = process.env.CLOUDFLARE_API_TOKEN;
  const wrangler = await executable("OPEN_COMPUTE_CF_WRANGLER");
  const endpoint = new URL(required("OPEN_COMPUTE_ENDPOINT"));
  if (endpoint.pathname !== "/" || endpoint.search || endpoint.hash
      || (endpoint.protocol !== "https:" && !(endpoint.protocol === "http:" && ["127.0.0.1", "localhost", "[::1]"].includes(endpoint.hostname)))) {
    throw new Error("open-compute endpoint must be HTTPS or loopback HTTP origin");
  }
  const openComputeInternalAccount = uuid(
    required("OPEN_COMPUTE_ACCOUNT_ID"),
    "open-compute internal data-plane account",
  );
  const adminToken = required("OPEN_COMPUTE_ADMIN_TOKEN");
  const openComputeApiBase = new URL("/client/v4", endpoint);
  const openComputeAccount = await verifyOpenComputeAccount(openComputeApiBase, adminToken);
  const cloudflareEnv = processEnv({
    CLOUDFLARE_ACCOUNT_ID: accountId,
    ...(token === undefined ? {} : { CLOUDFLARE_API_TOKEN: token }),
  });
  const openComputeEnv = processEnv({
    CLOUDFLARE_ACCOUNT_ID: openComputeAccount,
    CLOUDFLARE_API_BASE_URL: openComputeApiBase.href,
    CLOUDFLARE_API_TOKEN: adminToken,
  });
  const version = await command(wrangler, ["--version"], { cwd: ROOT, env: cloudflareEnv, timeout: 20_000 });
  const escapedWranglerVersion = WRANGLER_VERSION.replaceAll(".", "\\.");
  if (!new RegExp(`(?:^|\\s)${escapedWranglerVersion}(?:\\s|$)`).test(`${version.stdout}\n${version.stderr}`)) {
    throw new Error("Wrangler version differs from baseline");
  }
  await verifyWranglerAccount(wrangler, accountId, cloudflareEnv);
  await verifyWranglerAccount(wrangler, openComputeAccount, openComputeEnv);
  const prefix = `oc-p34-${Date.now().toString(36)}-${randomBytes(4).toString("hex")}`;
  const revision = (await command("git", ["rev-parse", "HEAD"], { cwd: ROOT, env: processEnv({}), timeout: 20_000 })).stdout.trim();
  const workingTreeSha256 = await sourceIdentity();
  const kvNamespaceCount = selected.reduce((count, fixture) => count
    + Object.values(fixture.bindings).filter(binding => binding.type === "kv_namespace").length, 0);
  const d1DatabaseCount = selected.reduce((count, fixture) => count
    + Object.values(fixture.bindings).filter(binding => binding.type === "d1_database").length, 0);
  const r2BucketCount = selected.reduce((count, fixture) => count
    + Object.values(fixture.bindings).filter(binding => binding.type === "r2_bucket").length, 0);
  const queueCount = selected.reduce((count, fixture) => count
    + Object.values(fixture.bindings).filter(binding => binding.type === "queue_producer").length, 0);
  const durableObjectNamespaceCount = selected.reduce((count, fixture) => count
    + Object.values(fixture.bindings).filter(binding => binding.type === "do_namespace").length, 0);
  const workflowCount = selected.reduce((count, fixture) => count
    + Object.values(fixture.bindings).filter(binding => binding.type === "workflow").length, 0);
  const plan = {
    schemaVersion: 1,
    phase: "preflight",
    revision,
    workingTreeSha256,
    accountAlias,
    prefix,
    fixtures: selected.length,
    mutationScope: `one uniquely named Worker per selected fixture and provider, ${kvNamespaceCount} uniquely named KV namespaces, ${d1DatabaseCount} uniquely named D1 databases, ${r2BucketCount} uniquely named R2 buckets, ${queueCount} uniquely named Queues, ${durableObjectNamespaceCount} Worker-owned Durable Object namespaces, and ${workflowCount} uniquely named Workflows per provider`,
    cleanup: [
      "fixed Wrangler delete --name of each exact Worker without dependency override",
      "exact owned KV namespace, D1 database, R2 bucket, Queue, Worker-owned Durable Object namespace, and Workflow deletion through the official v4 API followed by provider inventory absence verification",
    ],
  };
  process.stdout.write(`${JSON.stringify(plan)}\n`);
  const runRoot = join(ROOT, ".temp/gate-run");
  await mkdir(runRoot, { recursive: true });
  const directory = join(runRoot, prefix);
  await mkdir(directory, { recursive: false });
  await writeFile(join(directory, "plan.json"), `${JSON.stringify(plan, null, 2)}\n`, { mode: 0o600 });
  const journalPath = join(directory, "ownership.jsonl");
  const results: JsonRecord[] = [];
  const cleanup: JsonRecord[] = [];
  let failed: string | undefined;
  try {
    for (let index = 0; index < selected.length; index++) {
      const fixture = selected[index]!;
      const name = `${prefix}-${index}`;
      const projectRoot = join(directory, String(index));
      await cp(fixture.root, projectRoot, { recursive: true });
      const cfPreflightConfig = join(projectRoot, "wrangler-cloudflare-preflight.jsonc");
      const cfConfig = join(projectRoot, "wrangler-cloudflare.jsonc");
      const ocPreflightConfig = join(projectRoot, "wrangler-open-compute-preflight.jsonc");
      const ocConfig = join(projectRoot, "wrangler-open-compute.jsonc");
      await writeFile(join(projectRoot, "tsconfig.json"), `${JSON.stringify({
        extends: join(ROOT, "tsconfig.json"),
        compilerOptions: { types: ["@open-compute/workers-types"] },
        include: ["src/**/*.ts"],
      }, null, 2)}\n`, { mode: 0o600 });
      await writeFile(cfPreflightConfig, `${JSON.stringify(cloudflareBaseProject(fixture, name, accountId), null, 2)}\n`, { mode: 0o600 });
      await writeFile(ocPreflightConfig, `${JSON.stringify(
        openComputeBaseProject(fixture, name, openComputeAccount),
        null,
        2,
      )}\n`, { mode: 0o600 });
      let cloudflareWorkerAbsent = false;
      let cloudflareOwned = false;
      let openComputeWorkerAbsent = false;
      let openComputeOwned = false;
      let cloudflareUrl: string | undefined;
      const openComputeUrl = new URL(
        `/__workers/${openComputeInternalAccount}/${name}/`,
        endpoint,
      ).href;
      const kvNamespaces: OwnedKvNamespace[] = Object.entries(fixture.bindings)
        .filter(([, value]) => value.type === "kv_namespace")
        .map(([binding], bindingIndex) => ({
          binding,
          name: `${name}-kv-${bindingIndex}`,
          cloudflareAbsent: false,
          cloudflareOwned: false,
          openComputeAbsent: false,
          openComputeOwned: false,
        }));
      const d1Databases: OwnedD1Database[] = Object.entries(fixture.bindings)
        .filter(([, value]) => value.type === "d1_database")
        .map(([binding], bindingIndex) => ({
          binding,
          name: `${name}-d1-${bindingIndex}`,
          cloudflareAbsent: false,
          cloudflareOwned: false,
          openComputeAbsent: false,
          openComputeOwned: false,
        }));
      const r2Buckets: OwnedR2Bucket[] = Object.entries(fixture.bindings)
        .filter(([, value]) => value.type === "r2_bucket")
        .map(([binding], bindingIndex) => ({
          binding,
          name: `${name}-r2-${bindingIndex}`,
          cloudflareAbsent: false,
          cloudflareOwned: false,
          openComputeAbsent: false,
          openComputeOwned: false,
        }));
      const queues: OwnedQueue[] = Object.entries(fixture.bindings)
        .filter(([, value]) => value.type === "queue_producer")
        .map(([binding], bindingIndex) => ({
          binding,
          name: `${name}-queue-${bindingIndex}`,
          cloudflareAbsent: false,
          cloudflareOwned: false,
          openComputeAbsent: false,
          openComputeOwned: false,
        }));
      const durableObjectNamespaces: OwnedDurableObjectNamespace[] = Object.entries(fixture.bindings)
        .flatMap(([binding, value]) => value.type !== "do_namespace" ? [] : [{
          binding,
          className: value.className,
          openComputeOwned: false,
        }]);
      const workflows: OwnedWorkflow[] = Object.entries(fixture.bindings)
        .flatMap(([binding, value], bindingIndex) => value.type !== "workflow" ? [] : [{
          binding,
          className: value.className,
          name: `${name}-workflow-${bindingIndex}`,
          cloudflareAbsent: false,
          cloudflareOwned: false,
          openComputeAbsent: false,
          openComputeOwned: false,
        }]);
      try {
        await ensureCloudflareAbsent(name, cfPreflightConfig, wrangler, cloudflareEnv);
        cloudflareWorkerAbsent = true;
        await ensureCloudflareAbsent(name, ocPreflightConfig, wrangler, openComputeEnv);
        openComputeWorkerAbsent = true;
        for (const workflow of workflows) {
          await ensureWorkflowAbsent(
            workflow.name, cfPreflightConfig, wrangler, cloudflareEnv,
          );
          workflow.cloudflareAbsent = true;
          await ensureWorkflowAbsent(
            workflow.name, ocPreflightConfig, wrangler, openComputeEnv,
          );
          workflow.openComputeAbsent = true;
        }
        for (const namespace of kvNamespaces) {
          await ensureCloudflareKvAbsent(namespace.name, cfPreflightConfig, wrangler, cloudflareEnv);
          namespace.cloudflareAbsent = true;
          await ensureCloudflareKvAbsent(namespace.name, ocPreflightConfig, wrangler, openComputeEnv);
          namespace.openComputeAbsent = true;
          namespace.cloudflareOwned = true;
          namespace.cloudflareId = await createCloudflareKv(
            namespace.name, cfPreflightConfig, wrangler, cloudflareEnv,
          );
          await recordOwnership(journalPath, {
            target: "cloudflare", kind: "kv_namespace", name: namespace.name,
            binding: namespace.binding, id: namespace.cloudflareId,
          });
          namespace.openComputeOwned = true;
          namespace.openComputeId = await createCloudflareKv(
            namespace.name, ocPreflightConfig, wrangler, openComputeEnv,
          );
          await recordOwnership(journalPath, {
            target: "open-compute", kind: "kv_namespace", name: namespace.name,
            binding: namespace.binding, id: namespace.openComputeId,
          });
        }
        for (const database of d1Databases) {
          await ensureCloudflareD1Absent(database.name, cfPreflightConfig, wrangler, cloudflareEnv);
          database.cloudflareAbsent = true;
          await ensureCloudflareD1Absent(database.name, ocPreflightConfig, wrangler, openComputeEnv);
          database.openComputeAbsent = true;
          database.cloudflareOwned = true;
          database.cloudflareId = await createCloudflareD1(
            database.name, cfPreflightConfig, wrangler, cloudflareEnv,
          );
          await recordOwnership(journalPath, {
            target: "cloudflare", kind: "d1_database", name: database.name,
            binding: database.binding, id: database.cloudflareId,
          });
          database.openComputeOwned = true;
          database.openComputeId = await createCloudflareD1(
            database.name, ocPreflightConfig, wrangler, openComputeEnv,
          );
          await recordOwnership(journalPath, {
            target: "open-compute", kind: "d1_database", name: database.name,
            binding: database.binding, id: database.openComputeId,
          });
        }
        for (const bucket of r2Buckets) {
          await ensureCloudflareR2Absent(bucket.name, cfPreflightConfig, wrangler, cloudflareEnv);
          bucket.cloudflareAbsent = true;
          await ensureCloudflareR2Absent(bucket.name, ocPreflightConfig, wrangler, openComputeEnv);
          bucket.openComputeAbsent = true;
          bucket.cloudflareOwned = true;
          await createCloudflareR2(bucket.name, cfPreflightConfig, wrangler, cloudflareEnv);
          await recordOwnership(journalPath, {
            target: "cloudflare", kind: "r2_bucket", name: bucket.name, binding: bucket.binding,
          });
          bucket.openComputeOwned = true;
          await createCloudflareR2(bucket.name, ocPreflightConfig, wrangler, openComputeEnv);
          await recordOwnership(journalPath, {
            target: "open-compute", kind: "r2_bucket", name: bucket.name,
            binding: bucket.binding,
          });
        }
        for (const queue of queues) {
          await ensureQueueAbsent(queue.name, cfPreflightConfig, wrangler, cloudflareEnv);
          queue.cloudflareAbsent = true;
          await ensureQueueAbsent(queue.name, ocPreflightConfig, wrangler, openComputeEnv);
          queue.openComputeAbsent = true;
          queue.cloudflareOwned = true;
          await createQueue(queue.name, cfPreflightConfig, wrangler, cloudflareEnv);
          await recordOwnership(journalPath, {
            target: "cloudflare", kind: "queue_producer", name: queue.name, binding: queue.binding,
          });
          queue.openComputeOwned = true;
          await createQueue(queue.name, ocPreflightConfig, wrangler, openComputeEnv);
          await recordOwnership(journalPath, {
            target: "open-compute", kind: "queue_producer", name: queue.name,
            binding: queue.binding,
          });
        }
        const cloudflareBindingIds = Object.fromEntries([
          ...kvNamespaces.map(item => [item.binding, item.cloudflareId!] as const),
          ...d1Databases.map(item => [item.binding, item.cloudflareId!] as const),
        ]);
        const openComputeBindingIds = Object.fromEntries([
          ...kvNamespaces.map(item => [item.binding, item.openComputeId!] as const),
          ...d1Databases.map(item => [item.binding, item.openComputeId!] as const),
        ]);
        const cloudflareBindingNames = Object.fromEntries([
          ...d1Databases.map(item => [item.binding, item.name] as const),
          ...r2Buckets.map(item => [item.binding, item.name] as const),
          ...queues.map(item => [item.binding, item.name] as const),
          ...workflows.map(item => [item.binding, item.name] as const),
        ]);
        const openComputeBindingNames = Object.fromEntries([
          ...d1Databases.map(item => [item.binding, item.name] as const),
          ...r2Buckets.map(item => [item.binding, item.name] as const),
          ...queues.map(item => [item.binding, item.name] as const),
          ...workflows.map(item => [item.binding, item.name] as const),
        ]);
        await writeFile(cfConfig, `${JSON.stringify(cloudflareProject(
          fixture, name, accountId, cloudflareBindingIds, cloudflareBindingNames,
        ), null, 2)}\n`, { mode: 0o600 });
        await writeFile(ocConfig, `${JSON.stringify(openComputeProject(
          fixture, name, openComputeAccount, openComputeBindingIds, openComputeBindingNames,
        ), null, 2)}\n`, { mode: 0o600 });
        cloudflareOwned = true;
        for (const workflow of workflows) workflow.cloudflareOwned = true;
        const deployedCloudflare = await command(wrangler, [
          "deploy", "--name", name, "--config", cfConfig, "--latest=false", "--strict",
        ], {
          cwd: projectRoot,
          env: cloudflareEnv,
          timeout: 300_000,
        });
        await recordOwnership(journalPath, { target: "cloudflare", kind: "worker", name });
        for (const workflow of workflows) {
          await verifyWorkflowCreated(
            workflow.name, cfPreflightConfig, wrangler, cloudflareEnv,
          );
          await recordOwnership(journalPath, {
            target: "cloudflare", kind: "workflow", name: workflow.name, binding: workflow.binding,
          });
        }
        openComputeOwned = true;
        for (const namespace of durableObjectNamespaces) namespace.openComputeOwned = true;
        for (const workflow of workflows) workflow.openComputeOwned = true;
        await command(wrangler, [
          "deploy", "--name", name, "--config", ocConfig, "--latest=false", "--strict",
        ], {
          cwd: projectRoot,
          env: openComputeEnv,
          timeout: 300_000,
        });
        await recordOwnership(journalPath, { target: "open-compute", kind: "worker", name });
        for (const namespace of durableObjectNamespaces) {
          await recordOwnership(journalPath, {
            target: "open-compute", kind: "durable_object_namespace", name: namespace.binding,
            binding: namespace.binding, className: namespace.className, parent: name,
          });
        }
        for (const workflow of workflows) {
          await verifyWorkflowCreated(
            workflow.name, ocPreflightConfig, wrangler, openComputeEnv,
          );
          await recordOwnership(journalPath, {
            target: "open-compute", kind: "workflow", name: workflow.name,
            binding: workflow.binding, parent: name,
          });
        }
        cloudflareUrl = cloudflareDeploymentUrl(`${deployedCloudflare.stdout}\n${deployedCloudflare.stderr}`, name);
        const cloudflare = await observe(cloudflareUrl, fixture, "cloudflare");
        const openCompute = await observe(openComputeUrl, fixture, "open-compute", { connection: "close" });
        if (JSON.stringify(cloudflare) !== JSON.stringify(openCompute)) throw new Error(`${fixture.id}: normalized observations differ`);
        results.push({ id: fixture.id, status: "passed", sourceSha256: fixture.sourceSha256, cloudflare, openCompute });
      } catch (error) {
        failed = sanitizedError(error, [token, adminToken]);
        results.push({ id: fixture.id, status: "failed", sourceSha256: fixture.sourceSha256, error: failed });
      } finally {
        if (r2Buckets.length > 0 || durableObjectNamespaces.length > 0 || workflows.length > 0) {
          if (cloudflareUrl !== undefined) {
            await bestEffortFixtureCleanup(cloudflareUrl, fixture, {});
          }
          if (openComputeOwned) {
            await bestEffortFixtureCleanup(openComputeUrl, fixture, { connection: "close" });
          }
        }
        const cfWorker = cloudflareOwned
          ? await cleanupCloudflare(name, cfConfig, wrangler, cloudflareEnv)
          : { deleted: cloudflareWorkerAbsent, status: cloudflareWorkerAbsent ? "not-created" : "preflight-did-not-prove-absence" };
        const cfBindings = [];
        for (const namespace of [...kvNamespaces].reverse()) {
          cfBindings.push(namespace.cloudflareOwned
            ? await cleanupCloudflareKv(
              namespace.name, namespace.cloudflareId, cfPreflightConfig, wrangler, cloudflareEnv,
            )
            : {
              deleted: namespace.cloudflareAbsent,
              status: namespace.cloudflareAbsent ? "not-created" : "preflight-did-not-prove-absence",
            });
        }
        const cfD1Bindings = [];
        for (const database of [...d1Databases].reverse()) {
          cfD1Bindings.push(database.cloudflareOwned
            ? await cleanupCloudflareD1(
              database.name, database.cloudflareId, cfPreflightConfig, wrangler, cloudflareEnv,
            )
            : {
              deleted: database.cloudflareAbsent,
              status: database.cloudflareAbsent ? "not-created" : "preflight-did-not-prove-absence",
            });
        }
        const cfR2Bindings = [];
        for (const bucket of [...r2Buckets].reverse()) {
          cfR2Bindings.push(bucket.cloudflareOwned
            ? await cleanupCloudflareR2(
              bucket.name, cfPreflightConfig, wrangler, cloudflareEnv,
            )
            : {
              deleted: bucket.cloudflareAbsent,
              status: bucket.cloudflareAbsent ? "not-created" : "preflight-did-not-prove-absence",
            });
        }
        const cfQueueBindings = [];
        for (const queue of [...queues].reverse()) {
          cfQueueBindings.push(queue.cloudflareOwned
            ? await cleanupQueue(
              queue.name, cfPreflightConfig, wrangler, cloudflareEnv,
            )
            : {
              deleted: queue.cloudflareAbsent,
              status: queue.cloudflareAbsent ? "not-created" : "preflight-did-not-prove-absence",
            });
        }
        const cfDoBindings = durableObjectNamespaces.map(namespace => ({
          deleted: cfWorker.deleted === true,
          status: cfWorker.deleted === true ? "absent-with-owner-worker" : "owner-worker-still-present",
          binding: namespace.binding,
          className: namespace.className,
          owner: name,
        }));
        const cfWorkflowBindings = [];
        for (const workflow of [...workflows].reverse()) {
          cfWorkflowBindings.push(workflow.cloudflareOwned
            ? await cleanupWorkflow(
              workflow.name, cfPreflightConfig, wrangler, cloudflareEnv,
            )
            : {
              deleted: workflow.cloudflareAbsent,
              status: workflow.cloudflareAbsent ? "not-created" : "preflight-did-not-prove-absence",
            });
        }
        const ocWorker = openComputeOwned
          ? await cleanupCloudflare(name, ocConfig, wrangler, openComputeEnv)
          : { deleted: openComputeWorkerAbsent, status: openComputeWorkerAbsent ? "not-created" : "preflight-did-not-prove-absence" };
        const ocBindings = [];
        for (const namespace of [...kvNamespaces].reverse()) {
          ocBindings.push(namespace.openComputeOwned
            ? await cleanupCloudflareKv(
              namespace.name, namespace.openComputeId, ocPreflightConfig, wrangler, openComputeEnv,
            )
            : {
              deleted: namespace.openComputeAbsent,
              status: namespace.openComputeAbsent ? "not-created" : "preflight-did-not-prove-absence",
            });
        }
        const ocD1Bindings = [];
        for (const database of [...d1Databases].reverse()) {
          ocD1Bindings.push(database.openComputeOwned
            ? await cleanupCloudflareD1(
              database.name, database.openComputeId, ocPreflightConfig, wrangler, openComputeEnv,
            )
            : {
              deleted: database.openComputeAbsent,
              status: database.openComputeAbsent ? "not-created" : "preflight-did-not-prove-absence",
            });
        }
        const ocR2Bindings = [];
        for (const bucket of [...r2Buckets].reverse()) {
          ocR2Bindings.push(bucket.openComputeOwned
            ? await cleanupCloudflareR2(
              bucket.name, ocPreflightConfig, wrangler, openComputeEnv,
            )
            : {
              deleted: bucket.openComputeAbsent,
              status: bucket.openComputeAbsent ? "not-created" : "preflight-did-not-prove-absence",
            });
        }
        const ocQueueBindings = [];
        for (const queue of [...queues].reverse()) {
          ocQueueBindings.push(queue.openComputeOwned
            ? await cleanupQueue(
              queue.name, ocPreflightConfig, wrangler, openComputeEnv,
            )
            : {
              deleted: queue.openComputeAbsent,
              status: queue.openComputeAbsent ? "not-created" : "preflight-did-not-prove-absence",
            });
        }
        const ocDoBindings = durableObjectNamespaces.map(namespace => ({
          deleted: namespace.openComputeOwned ? ocWorker.deleted === true : true,
          status: namespace.openComputeOwned
            ? (ocWorker.deleted === true ? "absent-with-owner-worker" : "owner-worker-still-present")
            : "not-created",
          binding: namespace.binding,
          className: namespace.className,
          owner: name,
        }));
        const ocWorkflowBindings = [];
        for (const workflow of [...workflows].reverse()) {
          ocWorkflowBindings.push(workflow.openComputeOwned
            ? await cleanupWorkflow(
              workflow.name, ocPreflightConfig, wrangler, openComputeEnv,
            )
            : {
              deleted: workflow.openComputeAbsent,
              status: workflow.openComputeAbsent ? "not-created" : "preflight-did-not-prove-absence",
            });
        }
        const cf = {
          deleted: cfWorker.deleted === true
            && cfBindings.every(item => item.deleted === true)
            && cfD1Bindings.every(item => item.deleted === true)
            && cfR2Bindings.every(item => item.deleted === true)
            && cfQueueBindings.every(item => item.deleted === true)
            && cfDoBindings.every(item => item.deleted === true)
            && cfWorkflowBindings.every(item => item.deleted === true),
          worker: cfWorker,
          bindings: [
            ...cfBindings, ...cfD1Bindings, ...cfR2Bindings, ...cfQueueBindings, ...cfDoBindings,
            ...cfWorkflowBindings,
          ],
        };
        const oc = {
          deleted: ocWorker.deleted === true
            && ocBindings.every(item => item.deleted === true)
            && ocD1Bindings.every(item => item.deleted === true)
            && ocR2Bindings.every(item => item.deleted === true)
            && ocQueueBindings.every(item => item.deleted === true)
            && ocDoBindings.every(item => item.deleted === true)
            && ocWorkflowBindings.every(item => item.deleted === true),
          worker: ocWorker,
          bindings: [
            ...ocBindings, ...ocD1Bindings, ...ocR2Bindings, ...ocQueueBindings, ...ocDoBindings,
            ...ocWorkflowBindings,
          ],
        };
        cleanup.push({ id: fixture.id, cloudflare: cf, openCompute: oc });
        await recordOwnership(journalPath, { target: "cleanup", kind: fixture.id, name, result: { cloudflare: cf, openCompute: oc } });
        if (!cf.deleted || !oc.deleted) failed = `${fixture.id}: cleanup did not prove an empty final resource set`;
      }
      if (failed !== undefined) break;
    }
  } finally {
    if (failed === undefined) {
      await rm(directory, { recursive: true });
    } else {
      const failedDirectory = join(directory, "failed");
      await mkdir(failedDirectory, { recursive: true });
      await writeFile(join(failedDirectory, "result.json"), `${JSON.stringify({
        schemaVersion: 1, revision, workingTreeSha256, results, cleanup, error: failed,
      }, null, 2)}\n`, { mode: 0o600 });
    }
  }
  return {
    schemaVersion: 1,
    status: failed === undefined ? "passed" : "failed",
    cases: results.map(item => ({ id: item.id, status: item.status, ...(item.error === undefined ? {} : { error: item.error }) })),
    differential: { revision, workingTreeSha256, accountAlias, prefix, results, cleanup, error: failed },
  };
}

async function sourceIdentity(): Promise<string> {
  const env = processEnv({});
  const tracked = await command("git", ["diff", "--name-only", "-z", "--no-ext-diff", "HEAD"], {
    cwd: ROOT, env, timeout: 30_000,
  });
  const untracked = await command("git", ["ls-files", "-z", "--others", "--exclude-standard"], {
    cwd: ROOT, env, timeout: 30_000,
  });
  const names = [...new Set(`${tracked.stdout}\0${untracked.stdout}`.split("\0").filter(Boolean))].sort();
  const digest = createHash("sha256").update("open-compute-working-tree/v2\0");
  for (const name of names) {
    const path = resolve(ROOT, name);
    if (!path.startsWith(`${ROOT}/`)) throw new Error("working-tree source identity escapes the repository");
    try {
      if (!(await stat(path)).isFile()) throw new Error("working-tree source identity is not a regular file");
      digest.update("file\0").update(name).update("\0").update(await readFile(path));
    } catch (error) {
      if (error !== null && typeof error === "object" && Reflect.get(error, "code") === "ENOENT") {
        digest.update("deleted\0").update(name).update("\0");
      } else {
        throw error;
      }
    }
  }
  return digest.digest("hex");
}

async function recordOwnership(path: string, entry: JsonRecord): Promise<void> {
  await appendFile(path, `${JSON.stringify({ ...entry, recordedAtMs: Date.now() })}\n`, { mode: 0o600 });
}

async function bestEffortFixtureCleanup(
  base: string,
  fixture: PortableFixture,
  requestHeaders: Readonly<Record<string, string>>,
): Promise<void> {
  const cleanup = fixture.observations.find(observation => observation.path === "/cleanup");
  if (cleanup === undefined) return;
  try {
    const response = await fetchObservation(observationUrl(base, cleanup.path), {
      method: cleanup.method,
      headers: { ...requestHeaders, ...cleanup.headers, "cache-control": "no-cache" },
      ...(cleanup.body === undefined ? {} : { body: cleanup.body }),
    });
    await response.body?.cancel();
  } catch {
    // Cleanup continues through the provider authority below.
  }
}

async function verifyOpenComputeAccount(
  apiBase: URL,
  token: string,
): Promise<string> {
  const response = await fetch(new URL(`${apiBase.href.replace(/\/$/, "")}/accounts`), {
    headers: { authorization: `Bearer ${token}` },
    signal: AbortSignal.timeout(30_000),
  });
  if (!response.ok) {
    await response.body?.cancel();
    throw new Error("open-compute account verification failed");
  }
  const envelope: unknown = await response.json();
  if (envelope === null || typeof envelope !== "object"
      || Reflect.get(envelope, "success") !== true
      || !Array.isArray(Reflect.get(envelope, "errors"))
      || !Array.isArray(Reflect.get(envelope, "messages"))
      || !Array.isArray(Reflect.get(envelope, "result"))) {
    throw new Error("open-compute account envelope is invalid");
  }
  const accounts = Reflect.get(envelope, "result") as unknown[];
  if (accounts.length !== 1 || accounts[0] === null || typeof accounts[0] !== "object") {
    throw new Error("open-compute account selection is ambiguous");
  }
  const accountId = Reflect.get(accounts[0], "id");
  if (typeof accountId !== "string" || !/^[0-9a-f]{32}$/.test(accountId)) {
    throw new Error("open-compute account identity is invalid");
  }
  return accountId;
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

interface CloudflareKvNamespace { readonly id: string; readonly title: string }

async function listCloudflareKv(
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<CloudflareKvNamespace[]> {
  const result = await readOnlyWrangler(wrangler, ["kv", "namespace", "list", "--config", config], environment);
  if (result.status !== 0) throw new Error("Cloudflare KV namespace inventory failed");
  const parsed: unknown = JSON.parse(result.stdout);
  if (!Array.isArray(parsed)) throw new Error("Cloudflare KV namespace inventory is invalid");
  const namespaces = parsed.map(item => {
    if (item === null || typeof item !== "object") throw new Error("Cloudflare KV namespace inventory is invalid");
    const id = Reflect.get(item, "id");
    const title = Reflect.get(item, "title");
    if (typeof id !== "string" || !/^[0-9a-f]{32}$/.test(id)
        || typeof title !== "string" || title.length === 0) {
      throw new Error("Cloudflare KV namespace inventory is invalid");
    }
    return { id, title };
  });
  if (new Set(namespaces.map(item => item.id)).size !== namespaces.length) {
    throw new Error("Cloudflare KV namespace inventory contains duplicate identities");
  }
  return namespaces;
}

async function ensureCloudflareKvAbsent(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  if ((await listCloudflareKv(config, wrangler, environment)).some(item => item.title === name)) {
    throw new Error("refusing to overwrite a pre-existing Cloudflare KV namespace");
  }
}

async function createCloudflareKv(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<string> {
  const created = await command(wrangler, ["kv", "namespace", "create", name, "--config", config], {
    cwd: ROOT, env: environment, timeout: 120_000,
  });
  const ids = [...`${created.stdout}\n${created.stderr}`.matchAll(/"id"\s*:\s*"([0-9a-f]{32})"/g)]
    .map(match => match[1]!)
    .filter((id, index, values) => values.indexOf(id) === index);
  if (ids.length !== 1) throw new Error("Wrangler did not report one unambiguous KV namespace identity");
  const matches = (await listCloudflareKv(config, wrangler, environment))
    .filter(item => item.id === ids[0] || item.title === name);
  if (matches.length !== 1 || matches[0]!.id !== ids[0] || matches[0]!.title !== name) {
    throw new Error("Cloudflare KV namespace creation could not be verified");
  }
  return ids[0]!;
}

interface CloudflareD1Database { readonly id: string; readonly name: string }

function validD1Id(value: unknown): value is string {
  return typeof value === "string"
    && /^(?:[0-9a-f]{32}|[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12})$/.test(value);
}

async function listCloudflareD1(
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<CloudflareD1Database[]> {
  const result = await readOnlyWrangler(wrangler, ["d1", "list", "--config", config, "--json"], environment);
  if (result.status !== 0) throw new Error("Cloudflare D1 database inventory failed");
  const parsed: unknown = JSON.parse(result.stdout);
  if (!Array.isArray(parsed)) throw new Error("Cloudflare D1 database inventory is invalid");
  const databases = parsed.map(item => {
    if (item === null || typeof item !== "object") throw new Error("Cloudflare D1 database inventory is invalid");
    const id = Reflect.get(item, "uuid");
    const name = Reflect.get(item, "name");
    if (!validD1Id(id)
        || typeof name !== "string" || name.length === 0) {
      throw new Error("Cloudflare D1 database inventory is invalid");
    }
    return { id, name };
  });
  if (new Set(databases.map(item => item.id)).size !== databases.length) {
    throw new Error("Cloudflare D1 database inventory contains duplicate identities");
  }
  return databases;
}

async function ensureCloudflareD1Absent(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  if ((await listCloudflareD1(config, wrangler, environment)).some(item => item.name === name)) {
    throw new Error("refusing to overwrite a pre-existing Cloudflare D1 database");
  }
}

async function createCloudflareD1(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<string> {
  const created = await command(wrangler, ["d1", "create", name, "--config", config], {
    cwd: ROOT, env: environment, timeout: 120_000,
  });
  const ids = [...`${created.stdout}\n${created.stderr}`.matchAll(
    /"database_id"\s*:\s*"((?:[0-9a-f]{32}|[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}))"/g,
  )].map(match => match[1]!).filter((id, index, values) => values.indexOf(id) === index);
  if (ids.length !== 1) throw new Error("Wrangler did not report one unambiguous D1 database identity");
  const matches = (await listCloudflareD1(config, wrangler, environment))
    .filter(item => item.id === ids[0] || item.name === name);
  if (matches.length !== 1 || matches[0]!.id !== ids[0] || matches[0]!.name !== name) {
    throw new Error("Cloudflare D1 database creation could not be verified");
  }
  return ids[0]!;
}

function cloudflareR2Name(value: string): string {
  if (!/^[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$/.test(value)) {
    throw new Error("Cloudflare R2 bucket inventory is invalid");
  }
  return value;
}

async function listCloudflareR2(
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<string[]> {
  const result = await readOnlyWrangler(wrangler, ["r2", "bucket", "list", "--config", config], environment);
  if (result.status !== 0) throw new Error("Cloudflare R2 bucket inventory failed");
  const plain = result.stdout.replaceAll(/\u001b\[[0-9;]*m/g, "");
  const names = [...plain.matchAll(/^name:\s+(\S+)\s*$/gm)].map(match => cloudflareR2Name(match[1]!));
  if (new Set(names).size !== names.length) throw new Error("Cloudflare R2 bucket inventory contains duplicate names");
  return names;
}

async function ensureCloudflareR2Absent(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  cloudflareR2Name(name);
  if ((await listCloudflareR2(config, wrangler, environment)).includes(name)) {
    throw new Error("refusing to overwrite a pre-existing Cloudflare R2 bucket");
  }
}

async function createCloudflareR2(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  await command(wrangler, ["r2", "bucket", "create", name, "--config", config], {
    cwd: ROOT, env: environment, timeout: 120_000,
  });
  const matches = (await listCloudflareR2(config, wrangler, environment)).filter(item => item === name);
  if (matches.length !== 1) throw new Error("Cloudflare R2 bucket creation could not be verified");
}

async function cleanupCloudflare(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<JsonRecord> {
  const removed = await commandStatus(wrangler, ["delete", "--name", name, "--config", config], {
    cwd: ROOT, env: environment, timeout: 120_000,
  });
  const deadline = Date.now() + 30_000;
  let delayMs = 250;
  while (true) {
    const verify = await readOnlyWrangler(
      wrangler,
      ["deployments", "list", "--name", name, "--config", config, "--json"],
      environment,
    );
    if (verify.status !== 0) {
      return cloudflareWorkerMissing(`${verify.stdout}\n${verify.stderr}`)
        ? { deleted: true, status: removed.status === 0 ? "absent" : "absent-after-delete-error" }
        : { deleted: false, status: "verification-failed" };
    }
    if (Date.now() >= deadline) {
      return { deleted: false, status: removed.status === 0 ? "still-present" : "delete-failed" };
    }
    await new Promise(resolve => setTimeout(resolve, delayMs));
    delayMs = Math.min(delayMs * 2, 2_000);
  }
}

async function cleanupCloudflareKv(
  name: string,
  knownId: string | undefined,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<JsonRecord> {
  try {
    const matches = (await listCloudflareKv(config, wrangler, environment))
      .filter(item => item.title === name || item.id === knownId);
    if (matches.length === 0) return { deleted: true, status: "already-absent" };
    if (matches.length !== 1 || matches[0]!.title !== name
        || (knownId !== undefined && matches[0]!.id !== knownId)) {
      return { deleted: false, status: "ambiguous-owned-namespace" };
    }
    const id = matches[0]!.id;
    const removed = await commandStatus(wrangler, [
      "kv", "namespace", "delete", "--namespace-id", id, "--skip-confirmation", "--config", config,
    ], { cwd: ROOT, env: environment, timeout: 120_000 });
    if (removed.status !== 0) return { deleted: false, status: "delete-failed" };
    const remaining = (await listCloudflareKv(config, wrangler, environment))
      .some(item => item.id === id || item.title === name);
    return { deleted: !remaining, status: remaining ? "still-present" : "absent", id };
  } catch {
    return { deleted: false, status: "verification-failed" };
  }
}

async function cleanupCloudflareD1(
  name: string,
  knownId: string | undefined,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<JsonRecord> {
  try {
    const matches = (await listCloudflareD1(config, wrangler, environment))
      .filter(item => item.name === name || item.id === knownId);
    if (matches.length === 0) return { deleted: true, status: "already-absent" };
    if (matches.length !== 1 || matches[0]!.name !== name
        || (knownId !== undefined && matches[0]!.id !== knownId)) {
      return { deleted: false, status: "ambiguous-owned-database" };
    }
    const id = matches[0]!.id;
    const removed = await commandStatus(wrangler, [
      "d1", "delete", id, "--skip-confirmation", "--config", config,
    ], { cwd: ROOT, env: environment, timeout: 120_000 });
    const remaining = (await listCloudflareD1(config, wrangler, environment))
      .some(item => item.id === id || item.name === name);
    return {
      deleted: !remaining,
      status: remaining ? (removed.status === 0 ? "still-present" : "delete-failed")
        : (removed.status === 0 ? "absent" : "absent-after-delete-error"),
      id,
    };
  } catch {
    return { deleted: false, status: "verification-failed" };
  }
}

async function cleanupCloudflareR2(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<JsonRecord> {
  try {
    const matches = (await listCloudflareR2(config, wrangler, environment)).filter(item => item === name);
    if (matches.length === 0) return { deleted: true, status: "already-absent" };
    if (matches.length !== 1) return { deleted: false, status: "ambiguous-owned-bucket" };
    const removed = await commandStatus(wrangler, [
      "r2", "bucket", "delete", name, "--config", config,
    ], { cwd: ROOT, env: environment, timeout: 120_000 });
    const remaining = (await listCloudflareR2(config, wrangler, environment)).includes(name);
    return {
      deleted: !remaining,
      status: remaining ? (removed.status === 0 ? "still-present" : "delete-failed")
        : (removed.status === 0 ? "absent" : "absent-after-delete-error"),
      name,
    };
  } catch {
    return { deleted: false, status: "verification-failed" };
  }
}

async function readOnlyWrangler(
  wrangler: string,
  args: readonly string[],
  environment: Readonly<Record<string, string>>,
): Promise<CommandResult> {
  let result: CommandResult = { status: -1, stdout: "", stderr: "" };
  for (let attempt = 0; attempt < 3; attempt++) {
    result = await commandStatus(wrangler, args, { cwd: ROOT, env: environment, timeout: 60_000 });
    const output = `${result.stdout}\n${result.stderr}`;
    if (!cloudflareTransientFailure(output) && !output.includes("[code: 10000]")) return result;
    if (attempt < 2) await new Promise(resolveDelay => setTimeout(resolveDelay, 250 * (2 ** attempt)));
  }
  return result;
}
