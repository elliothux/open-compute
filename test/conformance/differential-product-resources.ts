import { resolve } from "node:path";

import { command, commandStatus, type CommandResult, type JsonRecord } from "./adapters.ts";

const ROOT = resolve(import.meta.dirname, "../..");

function output(result: CommandResult): string {
  return `${result.stdout}\n${result.stderr}`.replaceAll(/\u001b\[[0-9;]*m/g, "");
}

function queueMissing(result: CommandResult, name: string): boolean {
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

/** Refuse to reuse a Queue name that the differential run does not own. */
export async function ensureQueueAbsent(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  const result = await queueInfo(name, config, wrangler, environment);
  if (result.status === 0) throw new Error("refusing to overwrite a pre-existing Queue");
  if (!queueMissing(result, name)) throw new Error("Queue absence could not be verified");
}

/** Create and verify one exact, uniquely named Queue through fixed Wrangler. */
export async function createQueue(
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
    throw new Error("Queue creation could not be verified");
  }
}

/** Delete only the exact Queue name provisioned by this run and prove absence. */
export async function cleanupQueue(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<JsonRecord> {
  try {
    const before = await queueInfo(name, config, wrangler, environment);
    if (queueMissing(before, name)) return { deleted: true, status: "already-absent" };
    if (before.status !== 0) return { deleted: false, status: "verification-failed" };
    const removed = await commandStatus(wrangler, ["queues", "delete", name, "--config", config], {
      cwd: ROOT,
      env: environment,
      timeout: 120_000,
    });
    const after = await queueInfo(name, config, wrangler, environment);
    const deleted = queueMissing(after, name);
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

function exactTableName(outputText: string, name: string): boolean {
  const escaped = name.replaceAll(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`(?:^|[\\s│])${escaped}(?:$|[\\s│])`, "m").test(outputText);
}

async function workflowListed(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<boolean> {
  for (let page = 1; page <= 100; page++) {
    const result = await commandStatus(wrangler, [
      "workflows", "list", "--page", String(page), "--per-page", "100", "--config", config,
    ], { cwd: ROOT, env: environment, timeout: 60_000 });
    if (result.status !== 0) throw new Error("Workflow inventory failed");
    const text = output(result);
    if (exactTableName(text, name)) return true;
    if (text.includes("There are no deployed Workflows in this account")
        || text.includes(`No Workflows found on page ${page}.`)) return false;
  }
  throw new Error("Workflow inventory exceeded the bounded page audit");
}

/** Refuse to reuse a Workflow name that the differential run does not own. */
export async function ensureWorkflowAbsent(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  if (await workflowListed(name, config, wrangler, environment)) {
    throw new Error("refusing to overwrite a pre-existing Workflow");
  }
}

/** Verify that deployment created the exact uniquely named Workflow. */
export async function verifyWorkflowCreated(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  if (!await workflowListed(name, config, wrangler, environment)) {
    throw new Error("Workflow creation could not be verified");
  }
}

/** Delete one exact Workflow name and prove that it left inventory. */
export async function cleanupWorkflow(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<JsonRecord> {
  try {
    if (!await workflowListed(name, config, wrangler, environment)) {
      return { deleted: true, status: "already-absent" };
    }
    const removed = await commandStatus(wrangler, ["workflows", "delete", name, "--config", config], {
      cwd: ROOT,
      env: environment,
      timeout: 120_000,
    });
    const present = await workflowListed(name, config, wrangler, environment);
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
