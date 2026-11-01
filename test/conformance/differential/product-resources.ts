import { dirname } from "node:path";
import { command, commandStatus } from "../adapters/command.ts";
import type { JsonRecord } from "../adapters/types.ts";

async function listed(
  kind: "queues" | "workflows",
  name: string,
  config: string,
  cf: string,
  environment: Readonly<Record<string, string>>,
): Promise<JsonRecord | undefined> {
  for (let page = 1; page <= 100; page++) {
    // cf queues list takes no paging flags and returns the whole inventory.
    const args =
      kind === "queues"
        ? [kind, "list"]
        : [kind, "list", "--page", String(page), "--per-page", "100"];
    const result = await command(cf, args, {
      cwd: dirname(config),
      env: environment,
      timeout: 60_000,
    });
    const rows: unknown = JSON.parse(result.stdout);
    if (!Array.isArray(rows)) throw new Error(`${kind} inventory is invalid`);
    for (const row of rows) {
      if (row === null || typeof row !== "object" || Array.isArray(row))
        throw new Error(`${kind} inventory row is invalid`);
      if (Reflect.get(row, kind === "queues" ? "queue_name" : "name") === name)
        return row as JsonRecord;
    }
    if (kind === "queues" || rows.length < 100) return undefined;
  }
  throw new Error(`${kind} inventory exceeded the bounded page audit`);
}

export async function ensureQueueAbsent(
  name: string,
  config: string,
  cf: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  if (await listed("queues", name, config, cf, environment))
    throw new Error("refusing to overwrite a pre-existing Queue");
}
export async function createQueue(
  name: string,
  config: string,
  cf: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  await command(cf, ["queues", "create", "--queue-name", name], {
    cwd: dirname(config),
    env: environment,
    timeout: 120_000,
  });
  if (!(await listed("queues", name, config, cf, environment)))
    throw new Error("Queue creation could not be verified");
}
export async function cleanupQueue(
  name: string,
  config: string,
  cf: string,
  environment: Readonly<Record<string, string>>,
): Promise<JsonRecord> {
  try {
    const row = await listed("queues", name, config, cf, environment);
    if (!row) return { deleted: true, status: "already-absent" };
    if (typeof row.queue_id !== "string" || !row.queue_id)
      throw new Error("Queue identity is invalid");
    const removed = await commandStatus(
      cf,
      ["queues", "delete", row.queue_id, "--force"],
      { cwd: dirname(config), env: environment, timeout: 120_000 },
    );
    const present = await listed("queues", name, config, cf, environment);
    return {
      deleted: !present,
      status: present
        ? "still-present"
        : removed.status === 0
          ? "absent"
          : "absent-after-delete-error",
      name,
    };
  } catch {
    return { deleted: false, status: "verification-failed" };
  }
}
export async function ensureWorkflowAbsent(
  name: string,
  config: string,
  cf: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  if (await listed("workflows", name, config, cf, environment))
    throw new Error("refusing to overwrite a pre-existing Workflow");
}
export async function verifyWorkflowCreated(
  name: string,
  config: string,
  cf: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  if (!(await listed("workflows", name, config, cf, environment)))
    throw new Error("Workflow creation could not be verified");
}
export async function cleanupWorkflow(
  name: string,
  config: string,
  cf: string,
  environment: Readonly<Record<string, string>>,
): Promise<JsonRecord> {
  try {
    if (!(await listed("workflows", name, config, cf, environment)))
      return { deleted: true, status: "already-absent" };
    const removed = await commandStatus(
      cf,
      ["workflows", "delete", name, "--force"],
      { cwd: dirname(config), env: environment, timeout: 120_000 },
    );
    const present = await listed("workflows", name, config, cf, environment);
    return {
      deleted: !present,
      status: present
        ? "still-present"
        : removed.status === 0
          ? "absent"
          : "absent-after-delete-error",
      name,
    };
  } catch {
    return { deleted: false, status: "verification-failed" };
  }
}
