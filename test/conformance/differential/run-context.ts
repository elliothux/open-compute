import { randomBytes } from "node:crypto";
import { mkdir, stat, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { command } from "../adapters/command.ts";
import { WRANGLER_VERSION } from "../adapters/runtime-contract.ts";
import type { JsonRecord, PortableFixture } from "../adapters/types.ts";
import {
  verifyOpenComputeAccount,
  verifyWranglerAccount,
} from "./cloudflare-resources.ts";
import { processEnv } from "./environment.ts";
import { sourceIdentity } from "./evidence.ts";
import { resourceCounts } from "./owned-resources.ts";

export interface DifferentialContext {
  readonly root: string;
  readonly accountId: string;
  readonly accountAlias: string;
  readonly token?: string;
  readonly wrangler: string;
  readonly endpoint: URL;
  readonly openComputeInternalAccount: string;
  readonly adminToken: string;
  readonly openComputeAccount: string;
  readonly cloudflareEnv: Readonly<Record<string, string>>;
  readonly openComputeEnv: Readonly<Record<string, string>>;
  readonly prefix: string;
  readonly revision: string;
  readonly workingTreeSha256: string;
  readonly directory: string;
  readonly journalPath: string;
}

function required(name: string): string {
  const value = process.env[name];
  if (value === undefined || value.length === 0)
    throw new Error(`${name} is required`);
  return value;
}

function safeAlias(value: string): string {
  if (!/^[a-z0-9][a-z0-9._-]{0,63}$/.test(value))
    throw new Error("Cloudflare account alias is invalid");
  return value;
}

function uuid(value: string, label: string): string {
  if (
    !/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(
      value,
    )
  ) {
    throw new Error(`${label} must be a UUIDv7`);
  }
  return value;
}

async function executable(name: string): Promise<string> {
  const path = required(name);
  if (!path.startsWith("/") || !(await stat(path)).isFile())
    throw new Error(`${name} must name an absolute regular file`);
  return path;
}

function validatedEndpoint(): URL {
  const endpoint = new URL(required("OPEN_COMPUTE_ENDPOINT"));
  if (
    endpoint.pathname !== "/" ||
    endpoint.search ||
    endpoint.hash ||
    (endpoint.protocol !== "https:" &&
      !(
        endpoint.protocol === "http:" &&
        ["127.0.0.1", "localhost", "[::1]"].includes(endpoint.hostname)
      ))
  ) {
    throw new Error(
      "open-compute endpoint must be HTTPS or loopback HTTP origin",
    );
  }
  return endpoint;
}

export function sanitizedError(
  error: unknown,
  secrets: readonly (string | undefined)[],
): string {
  let message =
    error instanceof Error ? error.message : "differential fixture failed";
  for (const secret of secrets) {
    if (secret !== undefined && secret.length > 0)
      message = message.replaceAll(secret, "[REDACTED]");
  }
  return message.slice(0, 2048);
}

export async function prepareRun(
  root: string,
  selected: readonly PortableFixture[],
): Promise<DifferentialContext> {
  if (required("OPEN_COMPUTE_CF_MUTATION_ACK") !== "p3-cf-diff")
    throw new Error("Cloudflare mutation acknowledgement is missing");
  const accountId = required("OPEN_COMPUTE_CF_ACCOUNT_ID");
  if (!/^[0-9a-f]{32}$/.test(accountId))
    throw new Error("Cloudflare account ID is invalid");
  const accountAlias = safeAlias(required("OPEN_COMPUTE_CF_ACCOUNT_ALIAS"));
  const token = process.env.CLOUDFLARE_API_TOKEN;
  const wrangler = await executable("OPEN_COMPUTE_CF_WRANGLER");
  const endpoint = validatedEndpoint();
  const openComputeInternalAccount = uuid(
    required("OPEN_COMPUTE_ACCOUNT_ID"),
    "open-compute internal data-plane account",
  );
  const adminToken = required("OPEN_COMPUTE_ADMIN_TOKEN");
  const openComputeApiBase = new URL("/client/v4", endpoint);
  const openComputeAccount = await verifyOpenComputeAccount(
    openComputeApiBase,
    adminToken,
  );
  const cloudflareEnv = processEnv({
    CLOUDFLARE_ACCOUNT_ID: accountId,
    ...(token === undefined ? {} : { CLOUDFLARE_API_TOKEN: token }),
  });
  const openComputeEnv = processEnv({
    CLOUDFLARE_ACCOUNT_ID: openComputeAccount,
    CLOUDFLARE_API_BASE_URL: openComputeApiBase.href,
    CLOUDFLARE_API_TOKEN: adminToken,
  });
  const version = await command(wrangler, ["--version"], {
    cwd: root,
    env: cloudflareEnv,
    timeout: 20_000,
  });
  const escapedWranglerVersion = WRANGLER_VERSION.replaceAll(".", "\\.");
  if (
    !new RegExp(`(?:^|\\s)${escapedWranglerVersion}(?:\\s|$)`).test(
      `${version.stdout}\n${version.stderr}`,
    )
  ) {
    throw new Error("Wrangler version differs from baseline");
  }
  await verifyWranglerAccount(wrangler, accountId, cloudflareEnv);
  await verifyWranglerAccount(wrangler, openComputeAccount, openComputeEnv);
  const prefix = `oc-p34-${Date.now().toString(36)}-${randomBytes(4).toString("hex")}`;
  const revision = (
    await command("git", ["rev-parse", "HEAD"], {
      cwd: root,
      env: processEnv({}),
      timeout: 20_000,
    })
  ).stdout.trim();
  const workingTreeSha256 = await sourceIdentity();
  const counts = resourceCounts(selected);
  const plan: JsonRecord = {
    schemaVersion: 1,
    phase: "preflight",
    revision,
    workingTreeSha256,
    accountAlias,
    prefix,
    fixtures: selected.length,
    mutationScope: `one uniquely named Worker per selected fixture and provider, ${counts.kv} uniquely named KV namespaces, ${counts.d1} uniquely named D1 databases, ${counts.r2} uniquely named R2 buckets, ${counts.queues} uniquely named Queues, ${counts.durableObjects} Worker-owned Durable Object namespaces, and ${counts.workflows} uniquely named Workflows per provider`,
    cleanup: [
      "fixed Wrangler delete --name of each exact Worker without dependency override",
      "exact owned KV namespace, D1 database, R2 bucket, Queue, Worker-owned Durable Object namespace, and Workflow deletion through the official v4 API followed by provider inventory absence verification",
    ],
  };
  process.stdout.write(`${JSON.stringify(plan)}\n`);
  const runRoot = join(root, ".temp/gate-run");
  await mkdir(runRoot, { recursive: true });
  const directory = join(runRoot, prefix);
  await mkdir(directory, { recursive: false });
  await writeFile(
    join(directory, "plan.json"),
    `${JSON.stringify(plan, null, 2)}\n`,
    { mode: 0o600 },
  );
  return {
    root,
    accountId,
    accountAlias,
    ...(token === undefined ? {} : { token }),
    wrangler,
    endpoint,
    openComputeInternalAccount,
    adminToken,
    openComputeAccount,
    cloudflareEnv,
    openComputeEnv,
    prefix,
    revision,
    workingTreeSha256,
    directory,
    journalPath: join(directory, "ownership.jsonl"),
  };
}
