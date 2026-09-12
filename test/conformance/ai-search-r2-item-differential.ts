import { randomBytes } from "node:crypto";
import { mkdir, rm, stat, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { command, commandStatus } from "./adapters/command.ts";
import type { JsonRecord } from "./adapters/types.ts";
import { processEnv } from "./differential/environment.ts";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const API = "https://api.cloudflare.com/client/v4";

function required(name: string): string {
  const value = process.env[name];
  if (value === undefined || value.length === 0)
    throw new Error(`${name} is required`);
  return value;
}

function record(value: unknown, label: string): JsonRecord {
  if (value === null || typeof value !== "object" || Array.isArray(value))
    throw new Error(`${label} is not an object`);
  return value as JsonRecord;
}

async function api(
  token: string,
  account: string,
  method: string,
  path: string,
  body?: JsonRecord,
): Promise<{ status: number; result?: unknown; errors?: unknown }> {
  const response = await fetch(`${API}/accounts/${account}${path}`, {
    method,
    headers: {
      authorization: `Bearer ${token}`,
      ...(body === undefined ? {} : { "content-type": "application/json" }),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    signal: AbortSignal.timeout(45_000),
  });
  const payload = record(await response.json(), "Cloudflare response");
  return {
    status: response.status,
    result: payload.result,
    errors: payload.errors,
  };
}

function expectSuccess(
  response: { status: number; result?: unknown; errors?: unknown },
  label: string,
): unknown {
  if (response.status < 200 || response.status >= 300)
    throw new Error(`${label} failed with HTTP ${response.status}`);
  return response.result;
}

async function main(): Promise<void> {
  if (required("OPEN_COMPUTE_CF_MUTATION_ACK") !== "issue-52-ai-search-r2")
    throw new Error("Cloudflare mutation acknowledgement is missing");
  const account = required("OPEN_COMPUTE_CF_ACCOUNT_ID");
  if (!/^[0-9a-f]{32}$/.test(account))
    throw new Error("Cloudflare account ID is invalid");
  const token = required("CLOUDFLARE_API_TOKEN");
  const sourceTokenId = required("OPEN_COMPUTE_CF_AI_SEARCH_TOKEN_ID");
  const wrangler = required("OPEN_COMPUTE_CF_WRANGLER");
  if (!wrangler.startsWith("/") || !(await stat(wrangler)).isFile())
    throw new Error("OPEN_COMPUTE_CF_WRANGLER must name an absolute file");

  const suffix = `${Date.now().toString(36)}-${randomBytes(4).toString("hex")}`;
  const namespace = `oc-i52-${suffix}`;
  const instance = `oc-i52-${suffix}`;
  const bucket = `oc-i52-${suffix}`;
  const seedKey = "docs/seed.md";
  const itemKey = "docs/content-address";
  const directory = join(ROOT, ".temp/ai-search-r2-differential", suffix);
  await mkdir(directory, { recursive: true });
  const seedPath = join(directory, "seed.md");
  const itemPath = join(directory, "content-address");
  await writeFile(seedPath, "# seed\n\ninitial reconcile marker", {
    mode: 0o600,
  });
  await writeFile(itemPath, "extensionless issue 52 marker", { mode: 0o600 });
  const environment = processEnv({
    CLOUDFLARE_ACCOUNT_ID: account,
    CLOUDFLARE_API_TOKEN: token,
  });
  let namespaceOwned = false;
  let instanceOwned = false;
  let bucketOwned = false;
  let seedOwned = false;
  let itemOwned = false;
  const cleanup: JsonRecord = {};
  try {
    expectSuccess(
      await api(token, account, "POST", "/ai-search/namespaces", {
        name: namespace,
        description: "issue 52 differential",
      }),
      "namespace create",
    );
    namespaceOwned = true;
    await command(wrangler, ["r2", "bucket", "create", bucket, "--remote"], {
      cwd: ROOT,
      env: environment,
      timeout: 120_000,
    });
    bucketOwned = true;
    await command(
      wrangler,
      [
        "r2",
        "object",
        "put",
        `${bucket}/${seedKey}`,
        "--file",
        seedPath,
        "--content-type",
        "text/markdown",
        "--remote",
      ],
      { cwd: ROOT, env: environment, timeout: 120_000 },
    );
    seedOwned = true;
    expectSuccess(
      await api(
        token,
        account,
        "POST",
        `/ai-search/namespaces/${namespace}/instances`,
        {
          id: instance,
          type: "r2",
          source: bucket,
          token_id: sourceTokenId,
          sync_interval: 900,
          source_params: {
            prefix: "docs/",
            include_items: ["docs/*"],
          },
          index_method: { vector: false, keyword: true },
        },
      ),
      "instance create",
    );
    instanceOwned = true;
    const itemsPath = `/ai-search/namespaces/${namespace}/instances/${instance}/items`;
    const deadline = Date.now() + 300_000;
    for (;;) {
      const listed = expectSuccess(
        await api(token, account, "GET", `${itemsPath}?key=${seedKey}`),
        "initial item list",
      );
      const items = Array.isArray(listed) ? listed : [];
      const status =
        items.length === 1 ? Reflect.get(items[0]!, "status") : undefined;
      if (status === "completed") break;
      if (["error", "skipped", "outdated"].includes(String(status)))
        throw new Error(`initial reconcile ended as ${String(status)}`);
      if (Date.now() >= deadline)
        throw new Error(
          "initial reconcile did not complete within five minutes",
        );
      await new Promise((resolveWait) => setTimeout(resolveWait, 2_000));
    }
    expectSuccess(
      await api(
        token,
        account,
        "PUT",
        `/ai-search/namespaces/${namespace}/instances/${instance}`,
        { paused: true },
      ),
      "pause instance",
    );
    await command(
      wrangler,
      [
        "r2",
        "object",
        "put",
        `${bucket}/${itemKey}`,
        "--file",
        itemPath,
        "--content-type",
        "text/plain",
        "--remote",
      ],
      { cwd: ROOT, env: environment, timeout: 120_000 },
    );
    itemOwned = true;
    const upsert = await api(token, account, "PUT", itemsPath, {
      key: itemKey,
      next_action: "INDEX",
      wait_for_completion: true,
    });
    const upsertResult =
      upsert.status >= 200 && upsert.status < 300
        ? record(upsert.result, "upsert result")
        : undefined;
    const search =
      upsertResult === undefined
        ? undefined
        : await api(
            token,
            account,
            "POST",
            `/ai-search/namespaces/${namespace}/instances/${instance}/search`,
            {
              query: "extensionless issue 52 marker",
              ai_search_options: {
                retrieval: { retrieval_type: "keyword" },
              },
            },
          );
    process.stdout.write(
      `${JSON.stringify(
        {
          schemaVersion: 1,
          operation: "paused-r2-undiscovered-key-put",
          upsertStatus: upsert.status,
          upsertErrors: upsert.errors,
          itemStatus: upsertResult?.status,
          sourceId: upsertResult?.source_id,
          queryStatus: search?.status,
          queryable: JSON.stringify(search?.result).includes(
            "extensionless issue 52 marker",
          ),
        },
        null,
        2,
      )}\n`,
    );
  } finally {
    const run = async (args: readonly string[]): Promise<boolean> =>
      (
        await commandStatus(wrangler, args, {
          cwd: ROOT,
          env: environment,
          timeout: 120_000,
        })
      ).status === 0;
    if (instanceOwned)
      cleanup.instance = (
        await api(
          token,
          account,
          "DELETE",
          `/ai-search/namespaces/${namespace}/instances/${instance}`,
        )
      ).status;
    if (namespaceOwned)
      cleanup.namespace = (
        await api(
          token,
          account,
          "DELETE",
          `/ai-search/namespaces/${namespace}`,
        )
      ).status;
    if (itemOwned)
      cleanup.item = await run([
        "r2",
        "object",
        "delete",
        `${bucket}/${itemKey}`,
        "--remote",
      ]);
    if (seedOwned)
      cleanup.seed = await run([
        "r2",
        "object",
        "delete",
        `${bucket}/${seedKey}`,
        "--remote",
      ]);
    if (bucketOwned)
      cleanup.bucket = await run([
        "r2",
        "bucket",
        "delete",
        bucket,
        "--remote",
      ]);
    await rm(directory, { recursive: true, force: true });
    if (
      Object.values(cleanup).some(
        (value) =>
          value !== true &&
          (typeof value !== "number" || value < 200 || value >= 300),
      )
    )
      throw new Error(
        `differential cleanup failed: ${JSON.stringify(cleanup)}`,
      );
  }
}

await main();
