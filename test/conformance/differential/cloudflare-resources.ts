import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { command, commandStatus } from "../adapters/command.ts";
import {
  cloudflareTransientFailure,
  cloudflareWorkerMissing,
} from "../adapters/transport.ts";
import type { CommandResult, JsonRecord } from "../adapters/types.ts";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");

export async function verifyOpenComputeAccount(
  apiBase: URL,
  token: string,
): Promise<string> {
  const response = await fetch(
    new URL(`${apiBase.href.replace(/\/$/, "")}/accounts`),
    {
      headers: { authorization: `Bearer ${token}` },
      signal: AbortSignal.timeout(30_000),
    },
  );
  if (!response.ok) {
    await response.body?.cancel();
    throw new Error("open-compute account verification failed");
  }
  const envelope: unknown = await response.json();
  if (
    envelope === null ||
    typeof envelope !== "object" ||
    Reflect.get(envelope, "success") !== true ||
    !Array.isArray(Reflect.get(envelope, "errors")) ||
    !Array.isArray(Reflect.get(envelope, "messages")) ||
    !Array.isArray(Reflect.get(envelope, "result"))
  ) {
    throw new Error("open-compute account envelope is invalid");
  }
  const accounts = Reflect.get(envelope, "result") as unknown[];
  if (
    accounts.length !== 1 ||
    accounts[0] === null ||
    typeof accounts[0] !== "object"
  ) {
    throw new Error("open-compute account selection is ambiguous");
  }
  const accountId = Reflect.get(accounts[0], "id");
  if (typeof accountId !== "string" || !/^[0-9a-f]{32}$/.test(accountId)) {
    throw new Error("open-compute account identity is invalid");
  }
  return accountId;
}

export async function verifyWranglerAccount(
  wrangler: string,
  accountId: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  const result = await readOnlyWrangler(
    wrangler,
    ["whoami", "--json"],
    environment,
  );
  if (result.status !== 0)
    throw new Error("Wrangler account verification failed");
  const identity: unknown = JSON.parse(result.stdout);
  if (
    identity === null ||
    typeof identity !== "object" ||
    !Array.isArray(Reflect.get(identity, "accounts"))
  ) {
    throw new Error("Wrangler identity response is invalid");
  }
  const accounts = Reflect.get(identity, "accounts") as unknown[];
  if (
    !accounts.some(
      (account) =>
        account !== null &&
        typeof account === "object" &&
        Reflect.get(account, "id") === accountId,
    )
  ) {
    throw new Error(
      "Wrangler is not authenticated for the explicitly selected Cloudflare account",
    );
  }
}

export async function ensureCloudflareAbsent(
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
  if (result.status === 0)
    throw new Error("refusing to overwrite a pre-existing Cloudflare Worker");
  if (!cloudflareWorkerMissing(`${result.stdout}\n${result.stderr}`)) {
    throw new Error(
      "could not prove the unique Cloudflare Worker name was unused",
    );
  }
}

interface CloudflareKvNamespace {
  readonly id: string;
  readonly title: string;
}

async function listCloudflareKv(
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<CloudflareKvNamespace[]> {
  const result = await readOnlyWrangler(
    wrangler,
    ["kv", "namespace", "list", "--config", config],
    environment,
  );
  if (result.status !== 0)
    throw new Error("Cloudflare KV namespace inventory failed");
  const parsed: unknown = JSON.parse(result.stdout);
  if (!Array.isArray(parsed))
    throw new Error("Cloudflare KV namespace inventory is invalid");
  const namespaces = parsed.map((item) => {
    if (item === null || typeof item !== "object")
      throw new Error("Cloudflare KV namespace inventory is invalid");
    const id = Reflect.get(item, "id");
    const title = Reflect.get(item, "title");
    if (
      typeof id !== "string" ||
      !/^[0-9a-f]{32}$/.test(id) ||
      typeof title !== "string" ||
      title.length === 0
    ) {
      throw new Error("Cloudflare KV namespace inventory is invalid");
    }
    return { id, title };
  });
  if (new Set(namespaces.map((item) => item.id)).size !== namespaces.length) {
    throw new Error(
      "Cloudflare KV namespace inventory contains duplicate identities",
    );
  }
  return namespaces;
}

export async function ensureCloudflareKvAbsent(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  if (
    (await listCloudflareKv(config, wrangler, environment)).some(
      (item) => item.title === name,
    )
  ) {
    throw new Error(
      "refusing to overwrite a pre-existing Cloudflare KV namespace",
    );
  }
}

export async function createCloudflareKv(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<string> {
  const created = await command(
    wrangler,
    ["kv", "namespace", "create", name, "--config", config],
    {
      cwd: ROOT,
      env: environment,
      timeout: 120_000,
    },
  );
  const ids = [
    ...`${created.stdout}\n${created.stderr}`.matchAll(
      /"id"\s*:\s*"([0-9a-f]{32})"/g,
    ),
  ]
    .map((match) => match[1]!)
    .filter((id, index, values) => values.indexOf(id) === index);
  if (ids.length !== 1)
    throw new Error(
      "Wrangler did not report one unambiguous KV namespace identity",
    );
  const matches = (
    await listCloudflareKv(config, wrangler, environment)
  ).filter((item) => item.id === ids[0] || item.title === name);
  if (
    matches.length !== 1 ||
    matches[0]!.id !== ids[0] ||
    matches[0]!.title !== name
  ) {
    throw new Error("Cloudflare KV namespace creation could not be verified");
  }
  return ids[0]!;
}

interface CloudflareD1Database {
  readonly id: string;
  readonly name: string;
}

function validD1Id(value: unknown): value is string {
  return (
    typeof value === "string" &&
    /^(?:[0-9a-f]{32}|[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12})$/.test(value)
  );
}

async function listCloudflareD1(
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<CloudflareD1Database[]> {
  const result = await readOnlyWrangler(
    wrangler,
    ["d1", "list", "--config", config, "--json"],
    environment,
  );
  if (result.status !== 0)
    throw new Error("Cloudflare D1 database inventory failed");
  const parsed: unknown = JSON.parse(result.stdout);
  if (!Array.isArray(parsed))
    throw new Error("Cloudflare D1 database inventory is invalid");
  const databases = parsed.map((item) => {
    if (item === null || typeof item !== "object")
      throw new Error("Cloudflare D1 database inventory is invalid");
    const id = Reflect.get(item, "uuid");
    const name = Reflect.get(item, "name");
    if (!validD1Id(id) || typeof name !== "string" || name.length === 0) {
      throw new Error("Cloudflare D1 database inventory is invalid");
    }
    return { id, name };
  });
  if (new Set(databases.map((item) => item.id)).size !== databases.length) {
    throw new Error(
      "Cloudflare D1 database inventory contains duplicate identities",
    );
  }
  return databases;
}

export async function ensureCloudflareD1Absent(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  if (
    (await listCloudflareD1(config, wrangler, environment)).some(
      (item) => item.name === name,
    )
  ) {
    throw new Error(
      "refusing to overwrite a pre-existing Cloudflare D1 database",
    );
  }
}

export async function createCloudflareD1(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<string> {
  const created = await command(
    wrangler,
    ["d1", "create", name, "--config", config],
    {
      cwd: ROOT,
      env: environment,
      timeout: 120_000,
    },
  );
  const ids = [
    ...`${created.stdout}\n${created.stderr}`.matchAll(
      /"database_id"\s*:\s*"((?:[0-9a-f]{32}|[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}))"/g,
    ),
  ]
    .map((match) => match[1]!)
    .filter((id, index, values) => values.indexOf(id) === index);
  if (ids.length !== 1)
    throw new Error(
      "Wrangler did not report one unambiguous D1 database identity",
    );
  const matches = (
    await listCloudflareD1(config, wrangler, environment)
  ).filter((item) => item.id === ids[0] || item.name === name);
  if (
    matches.length !== 1 ||
    matches[0]!.id !== ids[0] ||
    matches[0]!.name !== name
  ) {
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
  const result = await readOnlyWrangler(
    wrangler,
    ["r2", "bucket", "list", "--config", config],
    environment,
  );
  if (result.status !== 0)
    throw new Error("Cloudflare R2 bucket inventory failed");
  const plain = result.stdout.replaceAll(/\u001b\[[0-9;]*m/g, "");
  const names = [...plain.matchAll(/^name:\s+(\S+)\s*$/gm)].map((match) =>
    cloudflareR2Name(match[1]!),
  );
  if (new Set(names).size !== names.length)
    throw new Error("Cloudflare R2 bucket inventory contains duplicate names");
  return names;
}

export async function ensureCloudflareR2Absent(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  cloudflareR2Name(name);
  if ((await listCloudflareR2(config, wrangler, environment)).includes(name)) {
    throw new Error(
      "refusing to overwrite a pre-existing Cloudflare R2 bucket",
    );
  }
}

export async function createCloudflareR2(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<void> {
  await command(
    wrangler,
    ["r2", "bucket", "create", name, "--config", config],
    {
      cwd: ROOT,
      env: environment,
      timeout: 120_000,
    },
  );
  const matches = (
    await listCloudflareR2(config, wrangler, environment)
  ).filter((item) => item === name);
  if (matches.length !== 1)
    throw new Error("Cloudflare R2 bucket creation could not be verified");
}

export async function cleanupCloudflare(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<JsonRecord> {
  const removed = await commandStatus(
    wrangler,
    ["delete", "--name", name, "--config", config],
    {
      cwd: ROOT,
      env: environment,
      timeout: 120_000,
    },
  );
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
        ? {
            deleted: true,
            status:
              removed.status === 0 ? "absent" : "absent-after-delete-error",
          }
        : { deleted: false, status: "verification-failed" };
    }
    if (Date.now() >= deadline) {
      return {
        deleted: false,
        status: removed.status === 0 ? "still-present" : "delete-failed",
      };
    }
    await new Promise((resolve) => setTimeout(resolve, delayMs));
    delayMs = Math.min(delayMs * 2, 2_000);
  }
}

export async function cleanupCloudflareKv(
  name: string,
  knownId: string | undefined,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<JsonRecord> {
  try {
    const matches = (
      await listCloudflareKv(config, wrangler, environment)
    ).filter((item) => item.title === name || item.id === knownId);
    if (matches.length === 0)
      return { deleted: true, status: "already-absent" };
    if (
      matches.length !== 1 ||
      matches[0]!.title !== name ||
      (knownId !== undefined && matches[0]!.id !== knownId)
    ) {
      return { deleted: false, status: "ambiguous-owned-namespace" };
    }
    const id = matches[0]!.id;
    const removed = await commandStatus(
      wrangler,
      [
        "kv",
        "namespace",
        "delete",
        "--namespace-id",
        id,
        "--skip-confirmation",
        "--config",
        config,
      ],
      { cwd: ROOT, env: environment, timeout: 120_000 },
    );
    if (removed.status !== 0)
      return { deleted: false, status: "delete-failed" };
    const remaining = (
      await listCloudflareKv(config, wrangler, environment)
    ).some((item) => item.id === id || item.title === name);
    return {
      deleted: !remaining,
      status: remaining ? "still-present" : "absent",
      id,
    };
  } catch {
    return { deleted: false, status: "verification-failed" };
  }
}

export async function cleanupCloudflareD1(
  name: string,
  knownId: string | undefined,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<JsonRecord> {
  try {
    const matches = (
      await listCloudflareD1(config, wrangler, environment)
    ).filter((item) => item.name === name || item.id === knownId);
    if (matches.length === 0)
      return { deleted: true, status: "already-absent" };
    if (
      matches.length !== 1 ||
      matches[0]!.name !== name ||
      (knownId !== undefined && matches[0]!.id !== knownId)
    ) {
      return { deleted: false, status: "ambiguous-owned-database" };
    }
    const id = matches[0]!.id;
    const removed = await commandStatus(
      wrangler,
      ["d1", "delete", id, "--skip-confirmation", "--config", config],
      { cwd: ROOT, env: environment, timeout: 120_000 },
    );
    const remaining = (
      await listCloudflareD1(config, wrangler, environment)
    ).some((item) => item.id === id || item.name === name);
    return {
      deleted: !remaining,
      status: remaining
        ? removed.status === 0
          ? "still-present"
          : "delete-failed"
        : removed.status === 0
          ? "absent"
          : "absent-after-delete-error",
      id,
    };
  } catch {
    return { deleted: false, status: "verification-failed" };
  }
}

export async function cleanupCloudflareR2(
  name: string,
  config: string,
  wrangler: string,
  environment: Readonly<Record<string, string>>,
): Promise<JsonRecord> {
  try {
    const matches = (
      await listCloudflareR2(config, wrangler, environment)
    ).filter((item) => item === name);
    if (matches.length === 0)
      return { deleted: true, status: "already-absent" };
    if (matches.length !== 1)
      return { deleted: false, status: "ambiguous-owned-bucket" };
    const removed = await commandStatus(
      wrangler,
      ["r2", "bucket", "delete", name, "--config", config],
      { cwd: ROOT, env: environment, timeout: 120_000 },
    );
    const remaining = (
      await listCloudflareR2(config, wrangler, environment)
    ).includes(name);
    return {
      deleted: !remaining,
      status: remaining
        ? removed.status === 0
          ? "still-present"
          : "delete-failed"
        : removed.status === 0
          ? "absent"
          : "absent-after-delete-error",
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
    result = await commandStatus(wrangler, args, {
      cwd: ROOT,
      env: environment,
      timeout: 60_000,
    });
    const output = `${result.stdout}\n${result.stderr}`;
    if (
      !cloudflareTransientFailure(output) &&
      !output.includes("[code: 10000]")
    )
      return result;
    if (attempt < 2)
      await new Promise((resolveDelay) =>
        setTimeout(resolveDelay, 250 * 2 ** attempt),
      );
  }
  return result;
}
