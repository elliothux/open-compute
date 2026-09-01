import { randomBytes } from "node:crypto";
import { createHash } from "node:crypto";
import { appendFile, cp, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  cloudflareBaseProject, cloudflareDeploymentUrl, cloudflareProject, cloudflareTransientFailure, cloudflareWorkerMissing,
  command, commandStatus, fetchObservation,
  loadPortableFixtures, observe, observationUrl, openComputeProject,
  WRANGLER_VERSION,
  type CommandResult, type JsonRecord, type PortableFixture,
} from "./adapters.ts";
import {
  activateOpenComputeWorkflowVersion, cleanupCloudflareQueue, cleanupCloudflareWorkflow,
  cleanupOpenComputeQueue, cleanupOpenComputeWorkflow, createCloudflareQueue, createOpenComputeQueue,
  cleanupOpenComputeDurableObjectNamespace, createOpenComputeDurableObjectNamespace,
  createOpenComputeWorkflow, ensureCloudflareQueueAbsent, ensureCloudflareWorkflowAbsent,
  ensureOpenComputeDurableObjectNamespaceAbsent, ensureOpenComputeQueueAbsent, ensureOpenComputeWorkflowAbsent,
  verifyCloudflareWorkflowCreated,
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
  openComputeId?: string;
}

interface OwnedQueue {
  readonly binding: string;
  readonly name: string;
  cloudflareAbsent: boolean;
  cloudflareOwned: boolean;
  openComputeAbsent: boolean;
  openComputeOwned: boolean;
  openComputeId?: string;
}

interface OwnedDurableObjectNamespace {
  readonly binding: string;
  readonly className: string;
  readonly name: string;
  openComputeAbsent: boolean;
  openComputeOwned: boolean;
  openComputeId?: string;
}

interface OwnedWorkflow {
  readonly binding: string;
  readonly className: string;
  readonly name: string;
  cloudflareAbsent: boolean;
  cloudflareOwned: boolean;
  openComputeAbsent: boolean;
  openComputeOwned: boolean;
  openComputeId?: string;
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
  const escapedWranglerVersion = WRANGLER_VERSION.replaceAll(".", "\\.");
  if (!new RegExp(`(?:^|\\s)${escapedWranglerVersion}(?:\\s|$)`).test(`${version.stdout}\n${version.stderr}`)) {
    throw new Error("Wrangler version differs from baseline");
  }
  await verifyWranglerAccount(wrangler, accountId, cloudflareEnv);
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
    mutationScope: `one uniquely named workers.dev Worker per selected fixture, ${kvNamespaceCount} uniquely named KV namespaces, ${d1DatabaseCount} uniquely named D1 databases, ${r2BucketCount} uniquely named R2 buckets, ${queueCount} uniquely named Queues, ${durableObjectNamespaceCount} Worker-owned Durable Object namespaces, and ${workflowCount} uniquely named Workflows per provider`,
    cleanup: [
      "Wrangler delete --name of the exact Worker with dependency override disabled",
      "exact open-compute route deletion followed by Worker tombstoning after a test-support runtime-generation restart when safely retained",
      "exact owned KV namespace, D1 database, R2 bucket, Queue, Worker-owned Durable Object namespace, and Workflow deletion followed by provider inventory absence verification",
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
      const cfPreflightConfig = join(projectRoot, "wrangler-preflight.json");
      const cfConfig = join(projectRoot, "wrangler.json");
      const ocConfig = join(projectRoot, "open-compute.json");
      const ocBootstrapConfig = join(projectRoot, "open-compute-bootstrap.json");
      await writeFile(join(projectRoot, "tsconfig.json"), `${JSON.stringify({
        extends: join(ROOT, "tsconfig.json"),
        compilerOptions: { types: ["@open-compute/workers-types"] },
        include: ["src/**/*.ts"],
      }, null, 2)}\n`, { mode: 0o600 });
      await writeFile(cfPreflightConfig, `${JSON.stringify(cloudflareBaseProject(fixture, name, accountId), null, 2)}\n`, { mode: 0o600 });
      let workerId: string | undefined;
      let deploymentId: string | undefined;
      let routeId: string | undefined;
      let cloudflareWorkerAbsent = false;
      let cloudflareOwned = false;
      let openComputeWorkerAbsent = false;
      let openComputeOwned = false;
      let cloudflareUrl: string | undefined;
      let openComputeHostname: string | undefined;
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
        .flatMap(([binding, value], bindingIndex) => value.type !== "do_namespace" ? [] : [{
          binding,
          className: value.className,
          name: `${name}-do-${bindingIndex}`,
          openComputeAbsent: false,
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
        await ensureOpenComputeAbsent(name, endpoint, openComputeAccount, adminToken);
        openComputeWorkerAbsent = true;
        for (const namespace of durableObjectNamespaces) {
          await ensureOpenComputeDurableObjectNamespaceAbsent(
            namespace.name, endpoint, openComputeAccount, adminToken,
          );
          namespace.openComputeAbsent = true;
        }
        for (const workflow of workflows) {
          await ensureCloudflareWorkflowAbsent(
            workflow.name, cfPreflightConfig, wrangler, cloudflareEnv,
          );
          workflow.cloudflareAbsent = true;
          await ensureOpenComputeWorkflowAbsent(
            workflow.name, endpoint, openComputeAccount, adminToken,
          );
          workflow.openComputeAbsent = true;
        }
        if (durableObjectNamespaces.length > 0 || workflows.length > 0) {
          const bootstrapFixture: PortableFixture = { ...fixture, bindings: {} };
          await writeFile(ocBootstrapConfig, `${JSON.stringify(openComputeProject(
            bootstrapFixture, name, endpoint.href, openComputeAccount,
          ), null, 2)}\n`, { mode: 0o600 });
          openComputeOwned = true;
          const bootstrap = await deployOpenComputeProject(
            ocBootstrapConfig, platformd, endpoint, openComputeAccount, adminToken,
          );
          workerId = bootstrap.workerId;
          deploymentId = bootstrap.deploymentId;
          await recordOwnership(journalPath, {
            target: "open-compute", kind: "worker", name, id: workerId,
            bootstrapDeploymentId: deploymentId,
          });
          for (const namespace of durableObjectNamespaces) {
            namespace.openComputeOwned = true;
            namespace.openComputeId = await createOpenComputeDurableObjectNamespace(
              namespace.name, workerId, namespace.className,
              endpoint, openComputeAccount, adminToken,
            );
            await recordOwnership(journalPath, {
              target: "open-compute", kind: "do_namespace", name: namespace.name,
              binding: namespace.binding, id: namespace.openComputeId, parent: workerId,
            });
          }
          for (const workflow of workflows) {
            workflow.openComputeOwned = true;
            workflow.openComputeId = await createOpenComputeWorkflow(
              workflow.name, endpoint, openComputeAccount, adminToken,
            );
            await recordOwnership(journalPath, {
              target: "open-compute", kind: "workflow", name: workflow.name,
              binding: workflow.binding, id: workflow.openComputeId,
            });
          }
        }
        for (const namespace of kvNamespaces) {
          await ensureCloudflareKvAbsent(namespace.name, cfPreflightConfig, wrangler, cloudflareEnv);
          namespace.cloudflareAbsent = true;
          await ensureOpenComputeKvAbsent(namespace.name, endpoint, openComputeAccount, adminToken);
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
          namespace.openComputeId = await createOpenComputeKv(
            namespace.name, endpoint, openComputeAccount, adminToken,
          );
          await recordOwnership(journalPath, {
            target: "open-compute", kind: "kv_namespace", name: namespace.name,
            binding: namespace.binding, id: namespace.openComputeId,
          });
        }
        for (const database of d1Databases) {
          await ensureCloudflareD1Absent(database.name, cfPreflightConfig, wrangler, cloudflareEnv);
          database.cloudflareAbsent = true;
          await ensureOpenComputeD1Absent(database.name, endpoint, openComputeAccount, adminToken);
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
          database.openComputeId = await createOpenComputeD1(
            database.name, endpoint, openComputeAccount, adminToken,
          );
          await recordOwnership(journalPath, {
            target: "open-compute", kind: "d1_database", name: database.name,
            binding: database.binding, id: database.openComputeId,
          });
        }
        for (const bucket of r2Buckets) {
          await ensureCloudflareR2Absent(bucket.name, cfPreflightConfig, wrangler, cloudflareEnv);
          bucket.cloudflareAbsent = true;
          await ensureOpenComputeR2Absent(bucket.name, endpoint, openComputeAccount, adminToken);
          bucket.openComputeAbsent = true;
          bucket.cloudflareOwned = true;
          await createCloudflareR2(bucket.name, cfPreflightConfig, wrangler, cloudflareEnv);
          await recordOwnership(journalPath, {
            target: "cloudflare", kind: "r2_bucket", name: bucket.name, binding: bucket.binding,
          });
          bucket.openComputeOwned = true;
          bucket.openComputeId = await createOpenComputeR2(
            bucket.name, endpoint, openComputeAccount, adminToken,
          );
          await recordOwnership(journalPath, {
            target: "open-compute", kind: "r2_bucket", name: bucket.name,
            binding: bucket.binding, id: bucket.openComputeId,
          });
        }
        for (const queue of queues) {
          await ensureCloudflareQueueAbsent(queue.name, cfPreflightConfig, wrangler, cloudflareEnv);
          queue.cloudflareAbsent = true;
          await ensureOpenComputeQueueAbsent(queue.name, endpoint, openComputeAccount, adminToken);
          queue.openComputeAbsent = true;
          queue.cloudflareOwned = true;
          await createCloudflareQueue(queue.name, cfPreflightConfig, wrangler, cloudflareEnv);
          await recordOwnership(journalPath, {
            target: "cloudflare", kind: "queue_producer", name: queue.name, binding: queue.binding,
          });
          queue.openComputeOwned = true;
          queue.openComputeId = await createOpenComputeQueue(
            queue.name, endpoint, openComputeAccount, adminToken,
          );
          await recordOwnership(journalPath, {
            target: "open-compute", kind: "queue_producer", name: queue.name,
            binding: queue.binding, id: queue.openComputeId,
          });
        }
        const cloudflareBindingIds = Object.fromEntries([
          ...kvNamespaces.map(item => [item.binding, item.cloudflareId!] as const),
          ...d1Databases.map(item => [item.binding, item.cloudflareId!] as const),
          ...r2Buckets.map(item => [item.binding, item.name] as const),
          ...queues.map(item => [item.binding, item.name] as const),
          ...durableObjectNamespaces.map(item => [item.binding, item.name] as const),
          ...workflows.map(item => [item.binding, item.name] as const),
        ]);
        const openComputeBindingIds = Object.fromEntries([
          ...kvNamespaces.map(item => [item.binding, item.openComputeId!] as const),
          ...d1Databases.map(item => [item.binding, item.openComputeId!] as const),
          ...r2Buckets.map(item => [item.binding, item.openComputeId!] as const),
          ...queues.map(item => [item.binding, item.openComputeId!] as const),
          ...durableObjectNamespaces.map(item => [item.binding, item.openComputeId!] as const),
          ...workflows.map(item => [item.binding, item.openComputeId!] as const),
        ]);
        const cloudflareBindingNames = Object.fromEntries([
          ...d1Databases.map(item => [item.binding, item.name] as const),
          ...r2Buckets.map(item => [item.binding, item.name] as const),
          ...queues.map(item => [item.binding, item.name] as const),
          ...workflows.map(item => [item.binding, item.name] as const),
        ]);
        await writeFile(cfConfig, `${JSON.stringify(cloudflareProject(
          fixture, name, accountId, cloudflareBindingIds, cloudflareBindingNames,
        ), null, 2)}\n`, { mode: 0o600 });
        await writeFile(ocConfig, `${JSON.stringify(openComputeProject(
          fixture, name, endpoint.href, openComputeAccount, openComputeBindingIds,
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
          await verifyCloudflareWorkflowCreated(
            workflow.name, cfPreflightConfig, wrangler, cloudflareEnv,
          );
          await recordOwnership(journalPath, {
            target: "cloudflare", kind: "workflow", name: workflow.name, binding: workflow.binding,
          });
        }
        openComputeOwned = true;
        const deployedOpenCompute = await deployOpenComputeProject(
          ocConfig, platformd, endpoint, openComputeAccount, adminToken,
        );
        if (workerId !== undefined && deployedOpenCompute.workerId !== workerId) {
          throw new Error("open-compute bootstrap changed Worker identity");
        }
        workerId = deployedOpenCompute.workerId;
        deploymentId = deployedOpenCompute.deploymentId;
        await recordOwnership(journalPath, { target: "open-compute", kind: "worker", name, id: workerId });
        await recordOwnership(journalPath, {
          target: "open-compute", kind: "deployment", name, id: deploymentId, parent: workerId,
        });
        for (const workflow of workflows) {
          await activateOpenComputeWorkflowVersion(
            workflow.openComputeId!, deploymentId, workflow.className,
            endpoint, openComputeAccount, adminToken,
          );
        }
        openComputeHostname = `${name}.p3-diff.invalid`;
        routeId = await createOpenComputeRoute(endpoint, openComputeAccount, workerId, openComputeHostname, adminToken);
        await recordOwnership(journalPath, {
          target: "open-compute", kind: "route", name: openComputeHostname, id: routeId, parent: workerId,
        });
        cloudflareUrl = cloudflareDeploymentUrl(`${deployedCloudflare.stdout}\n${deployedCloudflare.stderr}`, name);
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
        if (r2Buckets.length > 0 || durableObjectNamespaces.length > 0 || workflows.length > 0) {
          if (cloudflareUrl !== undefined) {
            await bestEffortFixtureCleanup(cloudflareUrl, fixture, {});
          }
          if (openComputeHostname !== undefined) {
            await bestEffortFixtureCleanup(endpoint.href, fixture, {
              host: openComputeHostname,
              connection: "close",
            });
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
            ? await cleanupCloudflareQueue(
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
          name: namespace.name,
        }));
        const cfWorkflowBindings = [];
        for (const workflow of [...workflows].reverse()) {
          cfWorkflowBindings.push(workflow.cloudflareOwned
            ? await cleanupCloudflareWorkflow(
              workflow.name, cfPreflightConfig, wrangler, cloudflareEnv,
            )
            : {
              deleted: workflow.cloudflareAbsent,
              status: workflow.cloudflareAbsent ? "not-created" : "preflight-did-not-prove-absence",
            });
        }
        const ocWorker = openComputeOwned
          ? await cleanupOpenCompute(name, workerId, routeId, endpoint, openComputeAccount, adminToken)
          : { deleted: openComputeWorkerAbsent, status: openComputeWorkerAbsent ? "not-created" : "preflight-did-not-prove-absence" };
        const ocBindings = [];
        for (const namespace of [...kvNamespaces].reverse()) {
          ocBindings.push(namespace.openComputeOwned
            ? await cleanupOpenComputeKv(
              namespace.name, namespace.openComputeId, endpoint, openComputeAccount, adminToken,
            )
            : {
              deleted: namespace.openComputeAbsent,
              status: namespace.openComputeAbsent ? "not-created" : "preflight-did-not-prove-absence",
            });
        }
        const ocD1Bindings = [];
        for (const database of [...d1Databases].reverse()) {
          ocD1Bindings.push(database.openComputeOwned
            ? await cleanupOpenComputeD1(
              database.name, database.openComputeId, endpoint, openComputeAccount, adminToken,
            )
            : {
              deleted: database.openComputeAbsent,
              status: database.openComputeAbsent ? "not-created" : "preflight-did-not-prove-absence",
            });
        }
        const ocR2Bindings = [];
        for (const bucket of [...r2Buckets].reverse()) {
          ocR2Bindings.push(bucket.openComputeOwned
            ? await cleanupOpenComputeR2(
              bucket.name, bucket.openComputeId, endpoint, openComputeAccount, adminToken,
            )
            : {
              deleted: bucket.openComputeAbsent,
              status: bucket.openComputeAbsent ? "not-created" : "preflight-did-not-prove-absence",
            });
        }
        const ocQueueBindings = [];
        for (const queue of [...queues].reverse()) {
          ocQueueBindings.push(queue.openComputeOwned
            ? await cleanupOpenComputeQueue(
              queue.name, queue.openComputeId, endpoint, openComputeAccount, adminToken,
            )
            : {
              deleted: queue.openComputeAbsent,
              status: queue.openComputeAbsent ? "not-created" : "preflight-did-not-prove-absence",
            });
        }
        const ocDoBindings = [];
        for (const namespace of [...durableObjectNamespaces].reverse()) {
          ocDoBindings.push(namespace.openComputeOwned
            ? await cleanupOpenComputeDurableObjectNamespace(
              namespace.name, namespace.openComputeId, endpoint, openComputeAccount, adminToken,
            )
            : {
              deleted: namespace.openComputeAbsent,
              status: namespace.openComputeAbsent ? "not-created" : "preflight-did-not-prove-absence",
            });
        }
        const ocWorkflowBindings = [];
        for (const workflow of [...workflows].reverse()) {
          ocWorkflowBindings.push(workflow.openComputeOwned
            ? await cleanupOpenComputeWorkflow(
              workflow.name, workflow.openComputeId, endpoint, openComputeAccount, adminToken,
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

interface OpenComputeDeploymentResult {
  readonly workerId: string;
  readonly deploymentId: string;
  readonly url: string;
}

async function deployOpenComputeProject(
  config: string,
  platformd: string,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<OpenComputeDeploymentResult> {
  const deployed = await command(process.execPath, [
    join(ROOT, "packages/toolchain/src/bin.ts"), "deploy", "--config", config,
    "--platformd", platformd, "--endpoint", endpoint.href, "--account", accountId,
    "--token-env", "OPEN_COMPUTE_ADMIN_TOKEN", "--json",
  ], {
    cwd: ROOT,
    env: processEnv(token === undefined ? {} : { OPEN_COMPUTE_ADMIN_TOKEN: token }),
    timeout: 300_000,
  });
  const result: unknown = JSON.parse(deployed.stdout);
  if (result === null || typeof result !== "object"
      || typeof Reflect.get(result, "workerId") !== "string"
      || typeof Reflect.get(result, "deploymentId") !== "string"
      || typeof Reflect.get(result, "url") !== "string") {
    throw new Error("open-compute deployment result is invalid");
  }
  return {
    workerId: uuid(Reflect.get(result, "workerId") as string, "open-compute Worker"),
    deploymentId: uuid(Reflect.get(result, "deploymentId") as string, "open-compute deployment"),
    url: Reflect.get(result, "url") as string,
  };
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

async function createOpenComputeRoute(
  endpoint: URL,
  accountId: string,
  workerId: string,
  hostname: string,
  token: string | undefined,
): Promise<string> {
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
  const id = Reflect.get(route, "id");
  if (typeof id !== "string" || id.length === 0) throw new Error("open-compute differential route identity is invalid");
  return id;
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

async function ensureOpenComputeAbsent(
  name: string,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<void> {
  const headers = token === undefined ? {} : { authorization: `Bearer ${token}` };
  const listed = await fetch(new URL(`/v1/accounts/${accountId}/workers`, endpoint), {
    headers,
    signal: AbortSignal.timeout(30_000),
  });
  if (!listed.ok) {
    await listed.body?.cancel();
    throw new Error("could not prove the unique open-compute Worker name was unused");
  }
  const body = await listed.json() as unknown;
  const workers = body !== null && typeof body === "object" ? Reflect.get(body, "workers") : undefined;
  if (!Array.isArray(workers)) throw new Error("open-compute Worker inventory is invalid");
  if (workers.some(item => item !== null && typeof item === "object"
      && Reflect.get(item, "name") === name && Reflect.get(item, "deletedAtMs") === null)) {
    throw new Error("refusing to overwrite a pre-existing open-compute Worker");
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

interface OpenComputeKvNamespace { readonly id: string; readonly name: string }

async function listOpenComputeKv(
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<OpenComputeKvNamespace[]> {
  const response = await fetch(new URL(`/v1/accounts/${accountId}/kv/namespaces`, endpoint), {
    headers: token === undefined ? {} : { authorization: `Bearer ${token}` },
    signal: AbortSignal.timeout(30_000),
  });
  if (!response.ok) {
    await response.body?.cancel();
    throw new Error("open-compute KV namespace inventory failed");
  }
  const body: unknown = await response.json();
  const raw = body !== null && typeof body === "object" ? Reflect.get(body, "namespaces") : undefined;
  if (!Array.isArray(raw)) throw new Error("open-compute KV namespace inventory is invalid");
  const namespaces = raw.map(item => {
    const resource = item !== null && typeof item === "object" ? Reflect.get(item, "resource") : undefined;
    const id = resource !== null && typeof resource === "object" ? Reflect.get(resource, "id") : undefined;
    const name = resource !== null && typeof resource === "object" ? Reflect.get(resource, "name") : undefined;
    if (typeof id !== "string" || typeof name !== "string" || name.length === 0) {
      throw new Error("open-compute KV namespace inventory is invalid");
    }
    uuid(id, "open-compute KV namespace");
    return { id, name };
  });
  if (new Set(namespaces.map(item => item.id)).size !== namespaces.length) {
    throw new Error("open-compute KV namespace inventory contains duplicate identities");
  }
  return namespaces;
}

async function ensureOpenComputeKvAbsent(
  name: string,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<void> {
  if ((await listOpenComputeKv(endpoint, accountId, token)).some(item => item.name === name)) {
    throw new Error("refusing to overwrite a pre-existing open-compute KV namespace");
  }
}

async function createOpenComputeKv(
  name: string,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<string> {
  const headers: Record<string, string> = {
    "content-type": "application/json",
    "idempotency-key": `p3-cf-diff-kv-${randomBytes(8).toString("hex")}`,
  };
  if (token !== undefined) headers.authorization = `Bearer ${token}`;
  const response = await fetch(new URL(`/v1/accounts/${accountId}/kv/namespaces`, endpoint), {
    method: "POST", headers, body: JSON.stringify({ name }), signal: AbortSignal.timeout(60_000),
  });
  if (response.status !== 200 && response.status !== 201) {
    await response.body?.cancel();
    throw new Error("open-compute KV namespace creation failed");
  }
  const body: unknown = await response.json();
  const id = body !== null && typeof body === "object" ? Reflect.get(body, "resourceId") : undefined;
  if (typeof id !== "string") throw new Error("open-compute KV namespace creation response is invalid");
  uuid(id, "open-compute KV namespace");
  const matches = (await listOpenComputeKv(endpoint, accountId, token))
    .filter(item => item.id === id || item.name === name);
  if (matches.length !== 1 || matches[0]!.id !== id || matches[0]!.name !== name) {
    throw new Error("open-compute KV namespace creation could not be verified");
  }
  return id;
}

interface CloudflareD1Database { readonly id: string; readonly name: string }

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
    if (typeof id !== "string" || !/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/.test(id)
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
    /"database_id"\s*:\s*"([0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12})"/g,
  )].map(match => match[1]!).filter((id, index, values) => values.indexOf(id) === index);
  if (ids.length !== 1) throw new Error("Wrangler did not report one unambiguous D1 database identity");
  const matches = (await listCloudflareD1(config, wrangler, environment))
    .filter(item => item.id === ids[0] || item.name === name);
  if (matches.length !== 1 || matches[0]!.id !== ids[0] || matches[0]!.name !== name) {
    throw new Error("Cloudflare D1 database creation could not be verified");
  }
  return ids[0]!;
}

interface OpenComputeD1Database { readonly id: string; readonly name: string }

async function listOpenComputeD1(
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<OpenComputeD1Database[]> {
  const response = await fetch(new URL(`/v1/accounts/${accountId}/d1/databases`, endpoint), {
    headers: token === undefined ? {} : { authorization: `Bearer ${token}` },
    signal: AbortSignal.timeout(30_000),
  });
  if (!response.ok) {
    await response.body?.cancel();
    throw new Error("open-compute D1 database inventory failed");
  }
  const body: unknown = await response.json();
  const raw = body !== null && typeof body === "object" ? Reflect.get(body, "databases") : undefined;
  if (!Array.isArray(raw)) throw new Error("open-compute D1 database inventory is invalid");
  const databases = raw.map(item => {
    const resource = item !== null && typeof item === "object" ? Reflect.get(item, "resource") : undefined;
    const id = resource !== null && typeof resource === "object" ? Reflect.get(resource, "id") : undefined;
    const name = resource !== null && typeof resource === "object" ? Reflect.get(resource, "name") : undefined;
    if (typeof id !== "string" || typeof name !== "string" || name.length === 0) {
      throw new Error("open-compute D1 database inventory is invalid");
    }
    uuid(id, "open-compute D1 database");
    return { id, name };
  });
  if (new Set(databases.map(item => item.id)).size !== databases.length) {
    throw new Error("open-compute D1 database inventory contains duplicate identities");
  }
  return databases;
}

async function ensureOpenComputeD1Absent(
  name: string,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<void> {
  if ((await listOpenComputeD1(endpoint, accountId, token)).some(item => item.name === name)) {
    throw new Error("refusing to overwrite a pre-existing open-compute D1 database");
  }
}

async function createOpenComputeD1(
  name: string,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<string> {
  const headers: Record<string, string> = {
    "content-type": "application/json",
    "idempotency-key": `p3-cf-diff-d1-${randomBytes(8).toString("hex")}`,
  };
  if (token !== undefined) headers.authorization = `Bearer ${token}`;
  const response = await fetch(new URL(`/v1/accounts/${accountId}/d1/databases`, endpoint), {
    method: "POST", headers, body: JSON.stringify({ name }), signal: AbortSignal.timeout(60_000),
  });
  if (response.status !== 200 && response.status !== 201) {
    await response.body?.cancel();
    throw new Error("open-compute D1 database creation failed");
  }
  const body: unknown = await response.json();
  const id = body !== null && typeof body === "object" ? Reflect.get(body, "resourceId") : undefined;
  if (typeof id !== "string") throw new Error("open-compute D1 database creation response is invalid");
  uuid(id, "open-compute D1 database");
  const matches = (await listOpenComputeD1(endpoint, accountId, token))
    .filter(item => item.id === id || item.name === name);
  if (matches.length !== 1 || matches[0]!.id !== id || matches[0]!.name !== name) {
    throw new Error("open-compute D1 database creation could not be verified");
  }
  return id;
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

interface OpenComputeR2Bucket { readonly id: string; readonly name: string }

async function listOpenComputeR2(
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<OpenComputeR2Bucket[]> {
  const response = await fetch(new URL(`/v1/accounts/${accountId}/r2/buckets`, endpoint), {
    headers: token === undefined ? {} : { authorization: `Bearer ${token}` },
    signal: AbortSignal.timeout(30_000),
  });
  if (!response.ok) {
    await response.body?.cancel();
    throw new Error("open-compute R2 bucket inventory failed");
  }
  const body: unknown = await response.json();
  const raw = body !== null && typeof body === "object" ? Reflect.get(body, "buckets") : undefined;
  if (!Array.isArray(raw)) throw new Error("open-compute R2 bucket inventory is invalid");
  const buckets = raw.map(item => {
    const id = item !== null && typeof item === "object" ? Reflect.get(item, "resourceId") : undefined;
    const name = item !== null && typeof item === "object" ? Reflect.get(item, "name") : undefined;
    if (typeof id !== "string" || typeof name !== "string" || name.length === 0) {
      throw new Error("open-compute R2 bucket inventory is invalid");
    }
    uuid(id, "open-compute R2 bucket");
    return { id, name };
  });
  if (new Set(buckets.map(item => item.id)).size !== buckets.length) {
    throw new Error("open-compute R2 bucket inventory contains duplicate identities");
  }
  return buckets;
}

async function ensureOpenComputeR2Absent(
  name: string,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<void> {
  if ((await listOpenComputeR2(endpoint, accountId, token)).some(item => item.name === name)) {
    throw new Error("refusing to overwrite a pre-existing open-compute R2 bucket");
  }
}

async function createOpenComputeR2(
  name: string,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<string> {
  const headers: Record<string, string> = {
    "content-type": "application/json",
    "idempotency-key": `p3-cf-diff-r2-${randomBytes(8).toString("hex")}`,
  };
  if (token !== undefined) headers.authorization = `Bearer ${token}`;
  const response = await fetch(new URL(`/v1/accounts/${accountId}/r2/buckets`, endpoint), {
    method: "POST", headers, body: JSON.stringify({ name }), signal: AbortSignal.timeout(60_000),
  });
  if (response.status !== 200 && response.status !== 201) {
    await response.body?.cancel();
    throw new Error("open-compute R2 bucket creation failed");
  }
  const body: unknown = await response.json();
  const bucket = body !== null && typeof body === "object" ? Reflect.get(body, "bucket") : undefined;
  const id = bucket !== null && typeof bucket === "object" ? Reflect.get(bucket, "resourceId") : undefined;
  if (typeof id !== "string") throw new Error("open-compute R2 bucket creation response is invalid");
  uuid(id, "open-compute R2 bucket");
  const matches = (await listOpenComputeR2(endpoint, accountId, token))
    .filter(item => item.id === id || item.name === name);
  if (matches.length !== 1 || matches[0]!.id !== id || matches[0]!.name !== name) {
    throw new Error("open-compute R2 bucket creation could not be verified");
  }
  return id;
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

async function cleanupOpenCompute(
  name: string,
  knownWorkerId: string | undefined,
  knownRouteId: string | undefined,
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
    const route = await cleanupOpenComputeRoute(endpoint, accountId, workerId, knownRouteId, `${name}.p3-diff.invalid`, headers);
    if (!route.deleted) return { deleted: false, status: "route-cleanup-failed", route };
    const url = new URL(`/v1/accounts/${accountId}/workers/${workerId}`, endpoint);
    let restarted = false;
    let removed = false;
    for (let attempt = 0; attempt < 2; attempt++) {
      const response = await fetch(url, {
        method: "DELETE",
        headers: { ...headers, "idempotency-key": `p3-cf-diff-delete-${randomBytes(8).toString("hex")}` },
        signal: AbortSignal.timeout(60_000),
      });
      const responseText = await response.text();
      if (response.ok || response.status === 410) {
        removed = true;
        break;
      }
      if (attempt === 0 && response.status === 409 && platformErrorCode(responseText) === "DEPLOYMENT_REFERENCED") {
        await restartOpenComputeRuntime(endpoint, headers);
        restarted = true;
        continue;
      }
      return { deleted: false, status: response.status, errorCode: platformErrorCode(responseText), route, restarted };
    }
    if (!removed) return { deleted: false, status: "delete-failed", route, restarted };
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
      && Reflect.get(item, "deletedAtMs") === null
      && (Reflect.get(item, "id") === workerId || Reflect.get(item, "name") === name));
    return { deleted: !present, status: present ? "still-present" : "absent", route, restarted };
  } catch {
    return { deleted: false, status: "unavailable" };
  }
}

async function cleanupOpenComputeKv(
  name: string,
  knownId: string | undefined,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<JsonRecord> {
  try {
    const matches = (await listOpenComputeKv(endpoint, accountId, token))
      .filter(item => item.name === name || item.id === knownId);
    if (matches.length === 0) return { deleted: true, status: "already-absent" };
    if (matches.length !== 1 || matches[0]!.name !== name
        || (knownId !== undefined && matches[0]!.id !== knownId)) {
      return { deleted: false, status: "ambiguous-owned-namespace" };
    }
    const id = matches[0]!.id;
    const headers: Record<string, string> = {
      "idempotency-key": `p3-cf-diff-kv-delete-${randomBytes(8).toString("hex")}`,
    };
    if (token !== undefined) headers.authorization = `Bearer ${token}`;
    const response = await fetch(new URL(`/v1/accounts/${accountId}/kv/namespaces/${id}`, endpoint), {
      method: "DELETE", headers, signal: AbortSignal.timeout(60_000),
    });
    await response.body?.cancel();
    if (!response.ok && response.status !== 404 && response.status !== 410) {
      return { deleted: false, status: response.status };
    }
    const remaining = (await listOpenComputeKv(endpoint, accountId, token))
      .some(item => item.id === id || item.name === name);
    return { deleted: !remaining, status: remaining ? "still-present" : "absent", id };
  } catch {
    return { deleted: false, status: "unavailable" };
  }
}

async function cleanupOpenComputeD1(
  name: string,
  knownId: string | undefined,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<JsonRecord> {
  try {
    const matches = (await listOpenComputeD1(endpoint, accountId, token))
      .filter(item => item.name === name || item.id === knownId);
    if (matches.length === 0) return { deleted: true, status: "already-absent" };
    if (matches.length !== 1 || matches[0]!.name !== name
        || (knownId !== undefined && matches[0]!.id !== knownId)) {
      return { deleted: false, status: "ambiguous-owned-database" };
    }
    const id = matches[0]!.id;
    const headers: Record<string, string> = {
      "idempotency-key": `p3-cf-diff-d1-delete-${randomBytes(8).toString("hex")}`,
    };
    if (token !== undefined) headers.authorization = `Bearer ${token}`;
    const response = await fetch(new URL(`/v1/accounts/${accountId}/d1/databases/${id}`, endpoint), {
      method: "DELETE", headers, signal: AbortSignal.timeout(60_000),
    });
    await response.body?.cancel();
    if (!response.ok && response.status !== 404 && response.status !== 410) {
      return { deleted: false, status: response.status };
    }
    const remaining = (await listOpenComputeD1(endpoint, accountId, token))
      .some(item => item.id === id || item.name === name);
    return { deleted: !remaining, status: remaining ? "still-present" : "absent", id };
  } catch {
    return { deleted: false, status: "unavailable" };
  }
}

async function cleanupOpenComputeR2(
  name: string,
  knownId: string | undefined,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<JsonRecord> {
  try {
    const matches = (await listOpenComputeR2(endpoint, accountId, token))
      .filter(item => item.name === name || item.id === knownId);
    if (matches.length === 0) return { deleted: true, status: "already-absent" };
    if (matches.length !== 1 || matches[0]!.name !== name
        || (knownId !== undefined && matches[0]!.id !== knownId)) {
      return { deleted: false, status: "ambiguous-owned-bucket" };
    }
    const id = matches[0]!.id;
    const headers: Record<string, string> = {
      "idempotency-key": `p3-cf-diff-r2-delete-${randomBytes(8).toString("hex")}`,
    };
    if (token !== undefined) headers.authorization = `Bearer ${token}`;
    const response = await fetch(new URL(`/v1/accounts/${accountId}/r2/buckets/${id}?force=true`, endpoint), {
      method: "DELETE", headers, signal: AbortSignal.timeout(120_000),
    });
    await response.body?.cancel();
    if (!response.ok && response.status !== 404 && response.status !== 410) {
      return { deleted: false, status: response.status };
    }
    const remaining = (await listOpenComputeR2(endpoint, accountId, token))
      .some(item => item.id === id || item.name === name);
    return { deleted: !remaining, status: remaining ? "still-present" : "absent", id };
  } catch {
    return { deleted: false, status: "unavailable" };
  }
}

async function cleanupOpenComputeRoute(
  endpoint: URL,
  accountId: string,
  workerId: string,
  routeId: string | undefined,
  hostname: string,
  headers: Readonly<Record<string, string>>,
): Promise<JsonRecord> {
  const routesUrl = new URL(`/v1/accounts/${accountId}/workers/${workerId}/routes`, endpoint);
  const listed = await fetch(routesUrl, {
    headers,
    signal: AbortSignal.timeout(30_000),
  });
  if (!listed.ok) {
    await listed.body?.cancel();
    return listed.status === 404 || listed.status === 410
      ? { deleted: true, status: listed.status }
      : { deleted: false, status: listed.status };
  }
  const body = await listed.json() as unknown;
  const routes = body !== null && typeof body === "object" ? Reflect.get(body, "routes") : undefined;
  if (!Array.isArray(routes)) return { deleted: false, status: "invalid-list" };
  const owned = routes.filter(item => item !== null && typeof item === "object"
    && (Reflect.get(item, "id") === routeId || Reflect.get(item, "hostnameAscii") === hostname));
  const ids = owned.map(item => Reflect.get(item, "id"));
  if (ids.some(id => typeof id !== "string" || id.length === 0) || new Set(ids).size !== ids.length) {
    return { deleted: false, status: "invalid-route-identity" };
  }
  for (const id of ids as string[]) {
    const response = await fetch(new URL(`${routesUrl.pathname}/${id}`, endpoint), {
      method: "DELETE",
      headers: { ...headers, "idempotency-key": `p3-cf-diff-route-delete-${randomBytes(8).toString("hex")}` },
      signal: AbortSignal.timeout(30_000),
    });
    await response.body?.cancel();
    if (!response.ok && response.status !== 404 && response.status !== 410) {
      return { deleted: false, status: response.status, routeId: id };
    }
  }
  const verify = await fetch(routesUrl, { headers, signal: AbortSignal.timeout(30_000) });
  if (!verify.ok) {
    await verify.body?.cancel();
    return verify.status === 404 || verify.status === 410
      ? { deleted: true, status: verify.status }
      : { deleted: false, status: verify.status };
  }
  const verified = await verify.json() as unknown;
  const remaining = verified !== null && typeof verified === "object" ? Reflect.get(verified, "routes") : undefined;
  if (!Array.isArray(remaining)) return { deleted: false, status: "invalid-verify-list" };
  const ownedRemaining = remaining.some(item => item !== null && typeof item === "object"
    && (Reflect.get(item, "id") === routeId || Reflect.get(item, "hostnameAscii") === hostname));
  return { deleted: !ownedRemaining, status: ownedRemaining ? "still-present" : "absent" };
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
