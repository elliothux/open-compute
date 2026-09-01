import { randomBytes } from "node:crypto";
import { resolve } from "node:path";

import { command, commandStatus, type CommandResult, type JsonRecord } from "./adapters.ts";

const ROOT = resolve(import.meta.dirname, "../..");

interface OpenComputeQueue {
  readonly id: string;
  readonly name: string;
  readonly lifecycleGeneration: number;
  readonly state: "creating" | "ready" | "deleting";
}

interface OpenComputeDurableObjectNamespace {
  readonly id: string;
  readonly name: string;
  readonly workerId: string;
  readonly className: string;
}

interface OpenComputeWorkflowDefinition {
  readonly id: string;
  readonly name: string;
  readonly state: "creating" | "ready" | "deleting";
}

function headers(token: string | undefined, json = false): Record<string, string> {
  return {
    ...(json ? { "content-type": "application/json" } : {}),
    ...(token === undefined ? {} : { authorization: `Bearer ${token}` }),
  };
}

function uuid(value: unknown, label: string): string {
  if (typeof value !== "string"
      || !/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(value)) {
    throw new Error(`${label} must be a UUIDv7`);
  }
  return value;
}

function output(result: CommandResult): string {
  return `${result.stdout}\n${result.stderr}`.replaceAll(/\u001b\[[0-9;]*m/g, "");
}

function cloudflareQueueMissing(result: CommandResult, name: string): boolean {
  return result.status !== 0 && output(result).includes(`Queue "${name}" does not exist.`);
}

async function queueInfo(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<CommandResult> {
  return commandStatus(wrangler, ["queues", "info", name, "--config", config], {
    cwd: ROOT,
    env: environment,
    timeout: 60_000,
  });
}

/** Refuse to reuse a Cloudflare Queue name that the differential run does not own. */
export async function ensureCloudflareQueueAbsent(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  const result = await queueInfo(name, config, wrangler, environment);
  if (result.status === 0) throw new Error("refusing to overwrite a pre-existing Cloudflare Queue");
  if (!cloudflareQueueMissing(result, name)) throw new Error("Cloudflare Queue absence could not be verified");
}

/** Create and verify one exact, uniquely named Cloudflare Queue. */
export async function createCloudflareQueue(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  await command(wrangler, ["queues", "create", name, "--config", config], {
    cwd: ROOT,
    env: environment,
    timeout: 120_000,
  });
  if ((await queueInfo(name, config, wrangler, environment)).status !== 0) {
    throw new Error("Cloudflare Queue creation could not be verified");
  }
}

/** Delete only the exact Queue name provisioned by this run and prove absence. */
export async function cleanupCloudflareQueue(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<JsonRecord> {
  try {
    const before = await queueInfo(name, config, wrangler, environment);
    if (cloudflareQueueMissing(before, name)) return { deleted: true, status: "already-absent" };
    if (before.status !== 0) return { deleted: false, status: "verification-failed" };
    const removed = await commandStatus(wrangler, ["queues", "delete", name, "--config", config], {
      cwd: ROOT,
      env: environment,
      timeout: 120_000,
    });
    const after = await queueInfo(name, config, wrangler, environment);
    const deleted = cloudflareQueueMissing(after, name);
    return {
      deleted,
      status: deleted ? (removed.status === 0 ? "absent" : "absent-after-delete-error")
        : (removed.status === 0 ? "still-present" : "delete-failed"),
      name,
    };
  } catch {
    return { deleted: false, status: "verification-failed" };
  }
}

async function listOpenComputeQueues(
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<OpenComputeQueue[]> {
  const response = await fetch(new URL(`/v1/accounts/${accountId}/queues`, endpoint), {
    headers: headers(token),
    signal: AbortSignal.timeout(30_000),
  });
  if (!response.ok) {
    await response.body?.cancel();
    throw new Error("open-compute Queue inventory failed");
  }
  const body: unknown = await response.json();
  const rows = body !== null && typeof body === "object" ? Reflect.get(body, "queues") : undefined;
  if (!Array.isArray(rows)) throw new Error("open-compute Queue inventory is invalid");
  const queues = rows.map(row => {
    if (row === null || typeof row !== "object") throw new Error("open-compute Queue inventory is invalid");
    const id = uuid(Reflect.get(row, "id"), "open-compute Queue");
    const name = Reflect.get(row, "name");
    const lifecycleGeneration = Reflect.get(row, "lifecycleGeneration");
    const state = Reflect.get(row, "state");
    if (typeof name !== "string" || name.length === 0
        || typeof lifecycleGeneration !== "number" || !Number.isSafeInteger(lifecycleGeneration)
        || lifecycleGeneration < 1
        || !["creating", "ready", "deleting", "tombstoned"].includes(String(state))) {
      throw new Error("open-compute Queue inventory is invalid");
    }
    return { id, name, lifecycleGeneration, state };
  }).filter((queue): queue is OpenComputeQueue => queue.state !== "tombstoned");
  if (new Set(queues.map(queue => queue.id)).size !== queues.length) {
    throw new Error("open-compute Queue inventory contains duplicate identities");
  }
  return queues;
}

/** Refuse to reuse an open-compute Queue name that this run does not own. */
export async function ensureOpenComputeQueueAbsent(
  name: string,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<void> {
  if ((await listOpenComputeQueues(endpoint, accountId, token)).some(queue => queue.name === name)) {
    throw new Error("refusing to overwrite a pre-existing open-compute Queue");
  }
}

/** Create and verify one exact open-compute Queue. */
export async function createOpenComputeQueue(
  name: string,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<string> {
  const response = await fetch(new URL(`/v1/accounts/${accountId}/queues`, endpoint), {
    method: "POST",
    headers: {
      ...headers(token, true),
      "idempotency-key": `p3-cf-diff-queue-${randomBytes(8).toString("hex")}`,
    },
    body: JSON.stringify({ name }),
    signal: AbortSignal.timeout(60_000),
  });
  if (response.status !== 200 && response.status !== 201) {
    await response.body?.cancel();
    throw new Error("open-compute Queue creation failed");
  }
  const body: unknown = await response.json();
  const queue = body !== null && typeof body === "object" ? Reflect.get(body, "queue") : undefined;
  const id = uuid(queue !== null && typeof queue === "object" ? Reflect.get(queue, "id") : undefined,
    "open-compute Queue");
  const matches = (await listOpenComputeQueues(endpoint, accountId, token))
    .filter(item => item.id === id || item.name === name);
  if (matches.length !== 1 || matches[0]!.id !== id || matches[0]!.name !== name) {
    throw new Error("open-compute Queue creation could not be verified");
  }
  return id;
}

/** Force-purge and delete only the exact Queue identity owned by this run. */
export async function cleanupOpenComputeQueue(
  name: string,
  knownId: string | undefined,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<JsonRecord> {
  try {
    const matches = (await listOpenComputeQueues(endpoint, accountId, token))
      .filter(item => item.name === name || item.id === knownId);
    if (matches.length === 0) return { deleted: true, status: "already-absent" };
    if (matches.length !== 1 || matches[0]!.name !== name
        || (knownId !== undefined && matches[0]!.id !== knownId)) {
      return { deleted: false, status: "ambiguous-owned-queue" };
    }
    const queue = matches[0]!;
    const url = new URL(`/v1/accounts/${accountId}/queues/${queue.id}`, endpoint);
    url.searchParams.set("force", "true");
    const response = await fetch(url, {
      method: "DELETE",
      headers: {
        ...headers(token),
        "idempotency-key": `p3-cf-diff-queue-delete-${randomBytes(8).toString("hex")}`,
        "x-open-compute-expected-lifecycle-generation": String(queue.lifecycleGeneration),
      },
      signal: AbortSignal.timeout(60_000),
    });
    await response.body?.cancel();
    if (!response.ok && response.status !== 404 && response.status !== 410) {
      return { deleted: false, status: response.status };
    }
    const remaining = (await listOpenComputeQueues(endpoint, accountId, token))
      .some(item => item.id === queue.id || item.name === name);
    return { deleted: !remaining, status: remaining ? "still-present" : "absent", id: queue.id };
  } catch {
    return { deleted: false, status: "verification-failed" };
  }
}

async function listOpenComputeDurableObjectNamespaces(
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<OpenComputeDurableObjectNamespace[]> {
  const response = await fetch(new URL(`/v1/accounts/${accountId}/durable-objects/namespaces`, endpoint), {
    headers: headers(token),
    signal: AbortSignal.timeout(30_000),
  });
  if (!response.ok) {
    await response.body?.cancel();
    throw new Error("open-compute Durable Object namespace inventory failed");
  }
  const body: unknown = await response.json();
  const rows = body !== null && typeof body === "object" ? Reflect.get(body, "namespaces") : undefined;
  if (!Array.isArray(rows)) throw new Error("open-compute Durable Object namespace inventory is invalid");
  const namespaces = rows.map(row => {
    if (row === null || typeof row !== "object") {
      throw new Error("open-compute Durable Object namespace inventory is invalid");
    }
    const id = uuid(Reflect.get(row, "resourceId"), "open-compute Durable Object namespace");
    const workerId = uuid(Reflect.get(row, "ownerWorkerId"), "open-compute Durable Object owner Worker");
    const name = Reflect.get(row, "name");
    const className = Reflect.get(row, "className");
    if (typeof name !== "string" || name.length === 0 || typeof className !== "string" || className.length === 0) {
      throw new Error("open-compute Durable Object namespace inventory is invalid");
    }
    return { id, name, workerId, className };
  });
  if (new Set(namespaces.map(namespace => namespace.id)).size !== namespaces.length) {
    throw new Error("open-compute Durable Object namespace inventory contains duplicate identities");
  }
  return namespaces;
}

/** Refuse to reuse an open-compute Durable Object namespace name. */
export async function ensureOpenComputeDurableObjectNamespaceAbsent(
  name: string,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<void> {
  if ((await listOpenComputeDurableObjectNamespaces(endpoint, accountId, token))
    .some(namespace => namespace.name === name)) {
    throw new Error("refusing to overwrite a pre-existing open-compute Durable Object namespace");
  }
}

/** Create and verify the Durable Object namespace owned by the bootstrapped Worker. */
export async function createOpenComputeDurableObjectNamespace(
  name: string,
  workerId: string,
  className: string,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<string> {
  uuid(workerId, "open-compute Durable Object owner Worker");
  const response = await fetch(new URL(`/v1/accounts/${accountId}/durable-objects/namespaces`, endpoint), {
    method: "POST",
    headers: {
      ...headers(token, true),
      "idempotency-key": `p3-cf-diff-do-${randomBytes(8).toString("hex")}`,
    },
    body: JSON.stringify({ name, workerId, className }),
    signal: AbortSignal.timeout(60_000),
  });
  if (response.status !== 200 && response.status !== 201) {
    await response.body?.cancel();
    throw new Error("open-compute Durable Object namespace creation failed");
  }
  const body: unknown = await response.json();
  const id = uuid(body !== null && typeof body === "object" ? Reflect.get(body, "resourceId") : undefined,
    "open-compute Durable Object namespace");
  const matches = (await listOpenComputeDurableObjectNamespaces(endpoint, accountId, token))
    .filter(item => item.id === id || item.name === name);
  if (matches.length !== 1 || matches[0]!.id !== id || matches[0]!.name !== name
      || matches[0]!.workerId !== workerId || matches[0]!.className !== className) {
    throw new Error("open-compute Durable Object namespace creation could not be verified");
  }
  return id;
}

/** Delete the exact Durable Object namespace and all objects created by its portable fixture. */
export async function cleanupOpenComputeDurableObjectNamespace(
  name: string,
  knownId: string | undefined,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<JsonRecord> {
  try {
    const matches = (await listOpenComputeDurableObjectNamespaces(endpoint, accountId, token))
      .filter(item => item.name === name || item.id === knownId);
    if (matches.length === 0) return { deleted: true, status: "already-absent" };
    if (matches.length !== 1 || matches[0]!.name !== name
        || (knownId !== undefined && matches[0]!.id !== knownId)) {
      return { deleted: false, status: "ambiguous-owned-namespace" };
    }
    const namespace = matches[0]!;
    const url = new URL(`/v1/accounts/${accountId}/durable-objects/namespaces/${namespace.id}`, endpoint);
    url.searchParams.set("force", "true");
    const response = await fetch(url, {
      method: "DELETE",
      headers: headers(token),
      signal: AbortSignal.timeout(120_000),
    });
    await response.body?.cancel();
    if (!response.ok && response.status !== 404 && response.status !== 410) {
      return { deleted: false, status: response.status };
    }
    const remaining = (await listOpenComputeDurableObjectNamespaces(endpoint, accountId, token))
      .some(item => item.id === namespace.id || item.name === name);
    return { deleted: !remaining, status: remaining ? "still-present" : "absent", id: namespace.id };
  } catch {
    return { deleted: false, status: "verification-failed" };
  }
}

function exactTableName(outputText: string, name: string): boolean {
  const escaped = name.replaceAll(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`(?:^|[\\s│])${escaped}(?:$|[\\s│])`, "m").test(outputText);
}

async function cloudflareWorkflowListed(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<boolean> {
  for (let page = 1; page <= 100; page++) {
    const result = await commandStatus(wrangler, [
      "workflows", "list", "--page", String(page), "--per-page", "100", "--config", config,
    ], { cwd: ROOT, env: environment, timeout: 60_000 });
    if (result.status !== 0) throw new Error("Cloudflare Workflow inventory failed");
    const text = output(result);
    if (exactTableName(text, name)) return true;
    if (text.includes("There are no deployed Workflows in this account")
        || text.includes(`No Workflows found on page ${page}.`)) return false;
  }
  throw new Error("Cloudflare Workflow inventory exceeded the bounded page audit");
}

/** Refuse to reuse a Cloudflare Workflow name that this run does not own. */
export async function ensureCloudflareWorkflowAbsent(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  if (await cloudflareWorkflowListed(name, config, wrangler, environment)) {
    throw new Error("refusing to overwrite a pre-existing Cloudflare Workflow");
  }
}

/** Verify that deployment created the exact uniquely named Cloudflare Workflow. */
export async function verifyCloudflareWorkflowCreated(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  if (!await cloudflareWorkflowListed(name, config, wrangler, environment)) {
    throw new Error("Cloudflare Workflow creation could not be verified");
  }
}

/** Delete one exact Cloudflare Workflow name and prove that it left inventory. */
export async function cleanupCloudflareWorkflow(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<JsonRecord> {
  try {
    if (!await cloudflareWorkflowListed(name, config, wrangler, environment)) {
      return { deleted: true, status: "already-absent" };
    }
    const removed = await commandStatus(wrangler, ["workflows", "delete", name, "--config", config], {
      cwd: ROOT,
      env: environment,
      timeout: 120_000,
    });
    const present = await cloudflareWorkflowListed(name, config, wrangler, environment);
    return {
      deleted: !present,
      status: present ? (removed.status === 0 ? "still-present" : "delete-failed")
        : (removed.status === 0 ? "absent" : "absent-after-delete-error"),
      name,
    };
  } catch {
    return { deleted: false, status: "verification-failed" };
  }
}

async function listOpenComputeWorkflows(
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<OpenComputeWorkflowDefinition[]> {
  const response = await fetch(new URL(`/v1/accounts/${accountId}/workflows?limit=1000`, endpoint), {
    headers: headers(token),
    signal: AbortSignal.timeout(30_000),
  });
  if (!response.ok) {
    await response.body?.cancel();
    throw new Error("open-compute Workflow inventory failed");
  }
  const rows: unknown = await response.json();
  if (!Array.isArray(rows)) throw new Error("open-compute Workflow inventory is invalid");
  const workflows = rows.map(row => {
    if (row === null || typeof row !== "object") throw new Error("open-compute Workflow inventory is invalid");
    const id = uuid(Reflect.get(row, "id"), "open-compute Workflow");
    const name = Reflect.get(row, "name");
    const state = Reflect.get(row, "state");
    if (typeof name !== "string" || name.length === 0
        || !["creating", "ready", "deleting", "tombstoned"].includes(String(state))) {
      throw new Error("open-compute Workflow inventory is invalid");
    }
    return { id, name, state };
  }).filter((workflow): workflow is OpenComputeWorkflowDefinition => workflow.state !== "tombstoned");
  if (new Set(workflows.map(workflow => workflow.id)).size !== workflows.length) {
    throw new Error("open-compute Workflow inventory contains duplicate identities");
  }
  return workflows;
}

/** Refuse to reuse an open-compute Workflow definition name. */
export async function ensureOpenComputeWorkflowAbsent(
  name: string,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<void> {
  if ((await listOpenComputeWorkflows(endpoint, accountId, token)).some(workflow => workflow.name === name)) {
    throw new Error("refusing to overwrite a pre-existing open-compute Workflow");
  }
}

/** Create and verify one exact open-compute Workflow definition. */
export async function createOpenComputeWorkflow(
  name: string,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<string> {
  const response = await fetch(new URL(`/v1/accounts/${accountId}/workflows`, endpoint), {
    method: "POST",
    headers: headers(token, true),
    body: JSON.stringify({ name }),
    signal: AbortSignal.timeout(60_000),
  });
  if (response.status !== 201) {
    await response.body?.cancel();
    throw new Error("open-compute Workflow creation failed");
  }
  const body: unknown = await response.json();
  const id = uuid(body !== null && typeof body === "object" ? Reflect.get(body, "id") : undefined,
    "open-compute Workflow");
  const matches = (await listOpenComputeWorkflows(endpoint, accountId, token))
    .filter(workflow => workflow.id === id || workflow.name === name);
  if (matches.length !== 1 || matches[0]!.id !== id || matches[0]!.name !== name) {
    throw new Error("open-compute Workflow creation could not be verified");
  }
  return id;
}

/** Stage and prove one ready Workflow version against the final immutable deployment. */
export async function activateOpenComputeWorkflowVersion(
  workflowId: string,
  deploymentId: string,
  className: string,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<void> {
  uuid(workflowId, "open-compute Workflow");
  uuid(deploymentId, "open-compute Workflow deployment");
  const versions = new URL(`/v1/accounts/${accountId}/workflows/${workflowId}/versions`, endpoint);
  const response = await fetch(versions, {
    method: "POST",
    headers: headers(token, true),
    body: JSON.stringify({ deploymentId, className }),
    signal: AbortSignal.timeout(120_000),
  });
  if (response.status !== 201 && response.status !== 202) {
    await response.body?.cancel();
    throw new Error("open-compute Workflow version creation failed");
  }
  await response.body?.cancel();
  for (let attempt = 0; attempt < 60; attempt++) {
    const reconcile = await fetch(new URL("/v1/operator/workflows/reconcile", endpoint), {
      method: "POST",
      headers: headers(token),
      signal: AbortSignal.timeout(60_000),
    });
    await reconcile.body?.cancel();
    if (!reconcile.ok) throw new Error("open-compute Workflow reconciliation failed");
    const listed = await fetch(versions, { headers: headers(token), signal: AbortSignal.timeout(30_000) });
    if (!listed.ok) {
      await listed.body?.cancel();
      throw new Error("open-compute Workflow version inventory failed");
    }
    const rows: unknown = await listed.json();
    if (!Array.isArray(rows)) throw new Error("open-compute Workflow version inventory is invalid");
    const matched = rows.find(row => {
      if (row === null || typeof row !== "object") return false;
      const target = Reflect.get(row, "target");
      return target !== null && typeof target === "object"
        && Reflect.get(target, "deploymentId") === deploymentId;
    });
    if (matched !== undefined && matched !== null && typeof matched === "object") {
      const state = Reflect.get(matched, "state");
      if (state === "ready") return;
      if (state === "failed") throw new Error("open-compute Workflow version validation failed");
    }
    await new Promise(resolveDelay => setTimeout(resolveDelay, 100));
  }
  throw new Error("open-compute Workflow version did not become ready");
}

/** Delete one exact open-compute Workflow definition and prove absence. */
export async function cleanupOpenComputeWorkflow(
  name: string,
  knownId: string | undefined,
  endpoint: URL,
  accountId: string,
  token: string | undefined,
): Promise<JsonRecord> {
  try {
    const matches = (await listOpenComputeWorkflows(endpoint, accountId, token))
      .filter(workflow => workflow.name === name || workflow.id === knownId);
    if (matches.length === 0) return { deleted: true, status: "already-absent" };
    if (matches.length !== 1 || matches[0]!.name !== name
        || (knownId !== undefined && matches[0]!.id !== knownId)) {
      return { deleted: false, status: "ambiguous-owned-workflow" };
    }
    const workflow = matches[0]!;
    const response = await fetch(new URL(`/v1/accounts/${accountId}/workflows/${workflow.id}`, endpoint), {
      method: "DELETE",
      headers: headers(token),
      signal: AbortSignal.timeout(60_000),
    });
    await response.body?.cancel();
    if (!response.ok && response.status !== 404 && response.status !== 410) {
      return { deleted: false, status: response.status };
    }
    const remaining = (await listOpenComputeWorkflows(endpoint, accountId, token))
      .some(item => item.id === workflow.id || item.name === name);
    return { deleted: !remaining, status: remaining ? "still-present" : "absent", id: workflow.id };
  } catch {
    return { deleted: false, status: "verification-failed" };
  }
}
