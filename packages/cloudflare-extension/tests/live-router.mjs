import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";

const baseURL = process.env.OPEN_COMPUTE_V4_BASE_URL;
const apiToken = process.env.OPEN_COMPUTE_V4_TOKEN;
const accountID = process.env.OPEN_COMPUTE_V4_ACCOUNT_ID;
const sdkEntry = process.env.OPEN_COMPUTE_CLOUDFLARE_SDK_ENTRY;
assert.ok(baseURL, "OPEN_COMPUTE_V4_BASE_URL is required");
assert.ok(apiToken, "OPEN_COMPUTE_V4_TOKEN is required");
assert.ok(accountID, "OPEN_COMPUTE_V4_ACCOUNT_ID is required");
assert.ok(sdkEntry, "OPEN_COMPUTE_CLOUDFLARE_SDK_ENTRY is required");

const { default: Cloudflare } = await import(pathToFileURL(sdkEntry).href);
const client = new Cloudflare({ apiToken, baseURL, maxRetries: 0 });

for (const [name, contract] of [
  ["identity", identityContract],
  ["workers", workersContract],
  ["kv", kvContract],
  ["d1", d1Contract],
  ["r2", r2Contract],
  ["vectorize", vectorizeContract],
  ["ai-search", aiSearchContract],
  ["queues", queuesContract],
  ["workflows", workflowsContract],
]) {
  try {
    await contract();
  } catch (error) {
    console.error(`official SDK ${name} contract failed`);
    throw error;
  }
}

async function identityContract() {
  const accounts = await client.accounts.list();
  assert.equal(accounts.result.length, 1);
  assert.equal(accounts.result_info.count, 1);
  assert.equal(accounts.result[0].id, accountID);
  assert.equal((await client.accounts.get({ account_id: accountID })).id, accountID);
  assert.match((await client.user.get()).id, /^[0-9a-f]{32}$/);
  assert.equal((await client.user.tokens.verify()).status, "active");
}

async function workersContract() {
  const workerName = "sdk-worker";
  const uploadedWorkerName = "sdk-uploaded-worker";
  const scripts = await client.workers.scripts.list({ account_id: accountID });
  assert.ok(scripts.result.some((script) => script.id === workerName));
  const versions = await client.workers.scripts.versions.list(workerName, {
    account_id: accountID, page: 1, per_page: 1,
  });
  assert.equal(versions.result.items.length, 1);
  const versionID = versions.result.items[0].id;
  assert.equal((await client.workers.scripts.versions.get(versionID, {
    account_id: accountID, script_name: workerName,
  })).id, versionID);
  const deployments = await client.workers.scripts.deployments.list(workerName, {
    account_id: accountID,
  });
  assert.equal(deployments.deployments.length, 1);
  assert.equal(deployments.deployments[0].versions[0].version_id, versionID);
  const secrets = await client.workers.scripts.secrets.list(workerName, {
    account_id: accountID,
  });
  assert.deepEqual(secrets.result.map((secret) => secret.name), ["SDK_SECRET"]);
  assert.equal((await client.workers.scripts.secrets.get("SDK_SECRET", {
    account_id: accountID, script_name: workerName,
  })).name, "SDK_SECRET");

  const upload = new FormData();
  upload.append("metadata", JSON.stringify({
    main_module: "index.js",
    compatibility_date: "2026-08-30",
  }));
  upload.append("index.js", new File(
    ["export default { fetch() { return new Response('sdk'); } };"],
    "index.js",
    { type: "application/javascript+module" },
  ));
  const uploadEnvelope = await client.put(
    `/accounts/${accountID}/workers/scripts/${uploadedWorkerName}`,
    { body: upload },
  );
  assert.equal(uploadEnvelope.success, true);
  assert.match(uploadEnvelope.result.id, /^[0-9a-f-]{36}$/);
  const uploadedVersions = await client.workers.scripts.versions.list(uploadedWorkerName, {
    account_id: accountID, page: 1, per_page: 10,
  });
  assert.equal(uploadedVersions.result.items.length, 1);
  const uploadedDeployments = await client.workers.scripts.deployments.list(uploadedWorkerName, {
    account_id: accountID,
  });
  assert.equal(uploadedDeployments.deployments.length, 1);
  assert.equal(
    uploadedDeployments.deployments[0].versions[0].version_id,
    uploadedVersions.result.items[0].id,
  );
  const mutatedSecret = await client.workers.scripts.secrets.update(uploadedWorkerName, {
    account_id: accountID,
    name: "SDK_MUTATED_SECRET",
    text: "official-sdk-secret-value",
    type: "secret_text",
  });
  assert.equal(mutatedSecret.name, "SDK_MUTATED_SECRET");
  const mutatedSecrets = await client.workers.scripts.secrets.list(uploadedWorkerName, {
    account_id: accountID,
  });
  assert.deepEqual(mutatedSecrets.result.map((secret) => secret.name), ["SDK_MUTATED_SECRET"]);
  await client.workers.scripts.secrets.delete("SDK_MUTATED_SECRET", {
    account_id: accountID,
    script_name: uploadedWorkerName,
  });
  assert.equal((await client.workers.scripts.secrets.list(uploadedWorkerName, {
    account_id: accountID,
  })).result.length, 0);

  await expectAPIError(() => client.workers.scripts.update(workerName, {
    account_id: accountID,
    metadata: { main_module: "index.js", compatibility_date: "2026-08-30" },
    files: [new File(
      ["export default { fetch() { return new Response('sdk'); } };"],
      "index.js",
      { type: "application/javascript+module" },
    )],
  }), [400]);
  await expectAPIError(() => client.workers.scripts.versions.get(
    "00000000-0000-7000-8000-000000000000",
    { account_id: accountID, script_name: workerName },
  ), [404]);
}

async function kvContract() {
  const first = await client.kv.namespaces.create({ account_id: accountID, title: "sdk-kv-a" });
  await client.kv.namespaces.create({ account_id: accountID, title: "sdk-kv-b" });
  const page = await client.kv.namespaces.list({ account_id: accountID, page: 1, per_page: 1 });
  assert.equal(page.result.length, 1);
  assert.equal(page.result_info.total_count, 2);
  assert.equal((await page.getNextPage()).result.length, 1);
  const bytes = new Uint8Array([0, 255, 1, 128, 65]);
  await client.kv.namespaces.values.update("raw-key", {
    account_id: accountID,
    namespace_id: first.id,
    value: new File([bytes], "value", { type: "application/octet-stream" }),
  });
  const response = await client.kv.namespaces.values.get("raw-key", {
    account_id: accountID, namespace_id: first.id,
  });
  assert.deepEqual(new Uint8Array(await response.arrayBuffer()), bytes);
  const keys = await client.kv.namespaces.keys.list(first.id, { account_id: accountID, limit: 1 });
  assert.equal(keys.result[0].name, "raw-key");
  await expectAPIError(() => client.kv.namespaces.get(
    "00000000000000000000000000000000", { account_id: accountID },
  ), [404]);
}

async function d1Contract() {
  const first = await client.d1.database.create({ account_id: accountID, name: "sdk-d1-a" });
  await client.d1.database.create({ account_id: accountID, name: "sdk-d1-b" });
  const page = await client.d1.database.list({ account_id: accountID, page: 1, per_page: 1 });
  assert.equal(page.result.length, 1);
  assert.equal(page.result_info.total_count, 2);
  assert.equal((await page.getNextPage()).result.length, 1);
  assert.equal((await client.d1.database.get(first.uuid, { account_id: accountID })).uuid, first.uuid);
  const query = await client.d1.database.query(first.uuid, {
    account_id: accountID, sql: "SELECT ? AS value", params: ["sdk"],
  });
  assert.equal(query.result.length, 1);
  assert.equal(query.result[0].success, true);
  await expectAPIError(() => client.d1.database.get(
    "00000000-0000-7000-8000-000000000000", { account_id: accountID },
  ), [404]);
}

async function r2Contract() {
  const bucket = await client.r2.buckets.create({ account_id: accountID, name: "sdk-r2" });
  assert.equal(bucket.name, "sdk-r2");
  const bytes = new Uint8Array([82, 50, 0, 255, 10]);
  await client.r2.buckets.objects.upload("raw.bin", bytes, {
    account_id: accountID, bucket_name: bucket.name,
  });
  const response = await client.r2.buckets.objects.get("raw.bin", {
    account_id: accountID, bucket_name: bucket.name,
  });
  assert.deepEqual(new Uint8Array(await response.arrayBuffer()), bytes);
  assert.ok((await client.r2.buckets.list({ account_id: accountID })).buckets.length >= 1);
  await expectAPIError(() => client.r2.buckets.objects.list(bucket.name, {
    account_id: accountID, per_page: 1,
  }), [501]);
  await expectAPIError(() => client.r2.buckets.get("sdk-r2-missing", {
    account_id: accountID,
  }), [404]);
}

async function vectorizeContract() {
  const index = await client.vectorize.indexes.create({
    account_id: accountID,
    name: "sdk-vectorize",
    config: { dimensions: 2, metric: "cosine" },
    description: "official SDK live-router Gate",
  });
  assert.equal(index.name, "sdk-vectorize");
  assert.equal((await client.vectorize.indexes.get(index.name, {
    account_id: accountID,
  })).name, index.name);
  assert.ok((await client.vectorize.indexes.list({ account_id: accountID })).result.length >= 1);
  await expectAPIError(() => client.vectorize.indexes.get("sdk-vectorize-missing", {
    account_id: accountID,
  }), [404]);
}

async function aiSearchContract() {
  await client.aiSearch.namespaces.create({
    account_id: accountID, name: "sdk-search-a", description: "first",
  });
  await client.aiSearch.namespaces.create({ account_id: accountID, name: "sdk-search-b" });
  const page = await client.aiSearch.namespaces.list({ account_id: accountID, page: 1, per_page: 1 });
  assert.equal(page.result.length, 1);
  assert.equal(page.result_info.total_count, 2);
  assert.equal((await page.getNextPage()).result.length, 1);
  assert.equal((await client.aiSearch.namespaces.read("sdk-search-a", {
    account_id: accountID,
  })).name, "sdk-search-a");
  await expectAPIError(() => client.aiSearch.namespaces.read("sdk-search-missing", {
    account_id: accountID,
  }), [404]);
}

async function queuesContract() {
  const queue = await client.queues.create({ account_id: accountID, queue_name: "sdk-queue" });
  assert.equal(queue.queue_name, "sdk-queue");
  assert.equal((await client.queues.get(queue.queue_id, {
    account_id: accountID,
  })).queue_id, queue.queue_id);
  assert.ok((await client.queues.list({ account_id: accountID })).result.length >= 1);
  await expectAPIError(() => client.queues.get(
    "00000000-0000-7000-8000-000000000000", { account_id: accountID },
  ), [404]);
}

async function workflowsContract() {
  const page = await client.workflows.list({ account_id: accountID, page: 1, per_page: 1 });
  assert.equal(page.result.length, 1);
  assert.equal(page.result[0].name, "sdk-workflow");
  assert.equal((await client.workflows.get("sdk-workflow", {
    account_id: accountID,
  })).name, "sdk-workflow");
  const versions = await client.workflows.versions.list("sdk-workflow", {
    account_id: accountID, page: 1, per_page: 1,
  });
  assert.equal(versions.result.length, 1);
  await expectAPIError(() => client.workflows.get("sdk-workflow-missing", {
    account_id: accountID,
  }), [404]);
}

async function expectAPIError(operation, statuses) {
  try {
    await operation();
    assert.fail("expected official SDK APIError");
  } catch (error) {
    assert.ok(error instanceof Cloudflare.APIError, String(error));
    assert.ok(statuses.includes(error.status), `unexpected status ${error.status}: ${error.message}`);
    assert.ok(Array.isArray(error.error?.errors), "Cloudflare error envelope is missing errors");
    assert.ok(error.error.errors.length > 0, "Cloudflare error envelope has no error item");
    assert.equal(typeof error.error.errors[0].code, "number");
    assert.equal(typeof error.error.errors[0].message, "string");
  }
}
