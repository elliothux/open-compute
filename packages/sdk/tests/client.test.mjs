import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { APIError } from "cloudflare";
import { createOpenComputeClient } from "../src/index.ts";

const repoRoot = new URL("../../..", import.meta.url).pathname;

async function mockClient(overrides = {}) {
  const requests = [];
  const responses = overrides.responses ?? [];
  const client = createOpenComputeClient({
    apiToken: "test-token",
    baseURL: "https://compute.example/client/v4",
    maxRetries: 0,
    fetch: async (url, init) => {
      const request =
        url instanceof Request ? url.clone() : new Request(url, init);
      requests.push({ url: request.url, init, request });
      const response = responses.shift();
      if (response !== undefined) return response;
      return new Response(
        JSON.stringify({ success: true, result: {}, errors: [], messages: [] }),
        { status: 200, headers: { "content-type": "application/json" } },
      );
    },
    ...overrides.options,
  });
  return { client, requests };
}

test("surface report, combined OpenAPI, and extension authority agree", async () => {
  const surface = JSON.parse(
    await readFile(new URL("../surface.json", import.meta.url), "utf8"),
  );
  const packageJson = JSON.parse(
    await readFile(new URL("../package.json", import.meta.url), "utf8"),
  );
  const extension = JSON.parse(
    await readFile(
      new URL("../../../openapi/open-compute-extension.json", import.meta.url),
      "utf8",
    ),
  );
  const combined = JSON.parse(
    await readFile(
      new URL("../../../openapi/open-compute-sdk.json", import.meta.url),
      "utf8",
    ),
  );
  const vendorOperations = Object.entries(extension.paths).flatMap(
    ([path, methods]) =>
      Object.entries(methods)
        .filter(([method]) => method !== "parameters")
        .map(([method, operation]) => ({
          method: method.toUpperCase(),
          path,
          sdkMethod: operation["x-open-compute-sdk-method"],
        })),
  );
  assert.equal(surface.schemaVersion, 1);
  assert.equal(surface.package, "@open-compute/sdk");
  assert.equal(surface.packageVersion, packageJson.version);
  assert.equal(surface.operations.length, 141);
  assert.equal(surface.observedStandardOperations.length, 18);
  assert.equal(surface.excludedOperations.length, 1);
  const byNode = (list) =>
    [...list].sort((left, right) => left.node.localeCompare(right.node));
  assert.deepEqual(
    byNode(surface.openComputeOperations),
    byNode(
      vendorOperations.map(({ method, path, sdkMethod }) => ({
        source: "open_compute_extension",
        node: `openCompute.${sdkMethod}`,
        method,
        path,
      })),
    ),
  );
  assert.equal(
    combined["x-open-compute-sdk"].surfaceDigest,
    surface.surfaceDigest,
  );
  const combinedStandard = Object.entries(combined.paths).flatMap(
    ([path, methods]) =>
      Object.entries(methods)
        .filter(([method]) => method !== "parameters")
        .map(([method]) => `${method.toUpperCase()} ${path}`),
  );
  assert.deepEqual(
    combinedStandard.sort(),
    [
      ...surface.operations.map((operation) => operation.operation),
      ...surface.observedStandardOperations.map(
        ({ method, path }) => `${method} ${path}`,
      ),
      ...vendorOperations.map(({ method, path }) => `${method} ${path}`),
    ].sort(),
  );
});

test("runtime surface walk equals the generated surface graph", async () => {
  const surface = JSON.parse(
    await readFile(new URL("../surface.json", import.meta.url), "utf8"),
  );
  const { client } = await mockClient();
  const runtime = [];
  const walk = (node, path) => {
    for (const [name, value] of Object.entries(node)) {
      if (typeof value === "function") runtime.push(`${path}.${name}`);
      else if (value !== null && typeof value === "object")
        walk(value, `${path}.${name}`);
    }
  };
  walk(client, "");
  const expected = [
    ...surface.operations.map(
      (operation) => `.${operation.node}.${operation.officialMethod}`,
    ),
    ...surface.observedStandardOperations.map(
      (operation) => `.${operation.node}`,
    ),
    ...surface.openComputeOperations.map((operation) => `.${operation.node}`),
  ].sort();
  assert.deepEqual(runtime.sort(), expected);
});

test("Artifacts delegate paginates, streams binary responses, and preserves raw path segments", async () => {
  const { client, requests } = await mockClient({
    responses: [
      new Response(
        JSON.stringify({
          success: true,
          result: [],
          result_info: { cursor: "", per_page: 50, count: 0 },
          errors: [],
          messages: [],
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      ),
      new Response("bytes", {
        status: 200,
        headers: { "content-type": "application/octet-stream" },
      }),
    ],
  });
  const page = await client.artifacts.namespaces.list({ account_id: "acc/1" });
  assert.deepEqual(page.result, []);
  const response = await client.artifacts.repositories.raw(
    "space",
    "repo",
    "main",
    "dir/a b.txt",
    { account_id: "acc/1" },
  );
  assert.equal(await response.text(), "bytes");
  assert.equal(
    requests[1].url,
    "https://compute.example/client/v4/accounts/acc%2F1/artifacts/namespaces/space/repos/repo/raw/main/dir/a%20b.txt",
  );
  assert.throws(() =>
    client.artifacts.repositories.raw("space", "repo", "main", "../secret", {
      account_id: "acc",
    }),
  );
});

test("standard and vendor methods issue official transport requests", async () => {
  const { client, requests } = await mockClient({
    responses: [
      new Response(
        JSON.stringify({
          success: true,
          result: [{ id: "v1", number: 1 }],
          errors: [],
          messages: [],
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      ),
      new Response(
        JSON.stringify({
          success: true,
          result: { state: "running" },
          errors: [],
          messages: [],
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      ),
    ],
  });
  const versions = await client.workers.scripts.versions.list("app", {
    account_id: "acc-1",
  });
  assert.equal(versions.result[0]?.number, 1);
  const status = await client.openCompute.system.status();
  assert.equal(status.state, "running");
  assert.deepEqual(
    requests.map(({ url, request }) => ({
      url,
      authorization: request.headers.get("authorization"),
    })),
    [
      {
        url: "https://compute.example/client/v4/accounts/acc-1/workers/scripts/app/versions",
        authorization: "Bearer test-token",
      },
      {
        url: "https://compute.example/client/v4/open-compute/system/status",
        authorization: "Bearer test-token",
      },
    ],
  );
});

test("vendor methods encode path segments and unwrap the v4 envelope", async () => {
  const { client, requests } = await mockClient({
    responses: [
      undefined,
      undefined,
      new Response(
        JSON.stringify({
          success: true,
          result: {
            id: "route",
            kind: "public_origin",
            url: "https://app.example.com/",
            scope: "public_network",
            created_on: "2026-01-01T00:00:00Z",
          },
          errors: [],
          messages: [],
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      ),
      new Response(
        JSON.stringify({
          success: true,
          result: { name: "app", url: "https://app.example.com/" },
          errors: [],
          messages: [],
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      ),
      new Response(
        JSON.stringify({
          success: true,
          result: null,
          errors: [],
          messages: [],
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      ),
    ],
  });
  await client.openCompute.backups.kv.create("acc/1", "ns");
  await client.openCompute.d1.migrations.apply("acc/1", "db", [
    { id: 1, name: "0001.sql", sha256: "a".repeat(64), sql: "SELECT 1" },
  ]);
  assert.equal(
    requests[0].url.endsWith(
      "/accounts/acc%2F1/open-compute/kv/namespaces/ns/backups",
    ),
    true,
  );
  assert.equal(requests[1].request.method, "PUT");
  assert.deepEqual(await requests[1].request.json(), [
    { id: 1, name: "0001.sql", sha256: "a".repeat(64), sql: "SELECT 1" },
  ]);
  const publicOrigin = await client.openCompute.workers.publicOrigin.set(
    "acc/1",
    "app",
    { name: "app" },
  );
  assert.equal(publicOrigin.url, "https://app.example.com/");
  const binding = await client.openCompute.workers.publicOrigin.get(
    "acc/1",
    "app",
  );
  assert.equal(binding?.name, "app");
  await client.openCompute.workers.publicOrigin.delete("acc/1", "app");
  assert.equal(requests[2].request.method, "PUT");
  assert.equal(requests[3].request.method, "GET");
  assert.equal(requests[4].request.method, "DELETE");
  assert.equal(
    requests[2].url,
    "https://compute.example/client/v4/accounts/acc%2F1/open-compute/workers/app/public-origin",
  );
  assert.deepEqual(await requests[2].request.json(), { name: "app" });
});

test("signature overrides preserve worker_loader metadata and asset File parts", async () => {
  const { client, requests } = await mockClient();
  const module = new File(["export default {}"], "index.js", {
    type: "application/javascript+module",
  });
  await client.workers.scripts.update("app", {
    account_id: "account",
    metadata: {
      main_module: "index.js",
      bindings: [{ type: "worker_loader", name: "LOADER" }],
    },
    files: [module],
  });
  const scriptRequest = requests.find(({ url }) =>
    url.endsWith("/workers/scripts/app"),
  ).request;
  assert.equal(scriptRequest.method, "PUT");
  assert.match(
    await scriptRequest.clone().text(),
    /Content-Type: application\/javascript\+module/,
  );
  const scriptForm = await scriptRequest.formData();
  assert.equal(scriptForm.get("metadata[main_module]"), "index.js");
  assert.equal(scriptForm.get("metadata[bindings][][type]"), "worker_loader");
  assert.equal(scriptForm.get("metadata[bindings][][name]"), "LOADER");
  await client.workers.scripts.update(
    "app-custom",
    {
      account_id: "account",
      metadata: { main_module: "index.js" },
      files: [module],
    },
    { headers: new Headers({ "X-Request-Label": "upload" }) },
  );
  const customRequest = requests.find(({ url }) =>
    url.endsWith("/workers/scripts/app-custom"),
  ).request;
  assert.equal(customRequest.headers.get("x-request-label"), "upload");
  assert.equal(
    (await customRequest.formData()).get("metadata[main_module]"),
    "index.js",
  );

  await client.workers.scripts.versions.create("app", {
    account_id: "account",
    metadata: {
      main_module: "index.js",
      bindings: [{ type: "worker_loader", name: "LOADER" }],
    },
    files: [module],
  });
  const versionRequest = requests.find(({ url }) =>
    url.endsWith("/workers/scripts/app/versions"),
  ).request;
  const versionForm = await versionRequest.formData();
  assert.equal(versionForm.get("metadata[main_module]"), "index.js");
  assert.equal(versionForm.get("metadata[bindings][][type]"), "worker_loader");
  assert.equal(versionForm.get("metadata[bindings][][name]"), "LOADER");

  const html = new File(["PGgxPm9rPC9oMT4="], "index.html", {
    type: "text/html",
  });
  await client.workers.assets.upload.create({
    account_id: "account",
    base64: true,
    body: { "index.html": html, "plain.txt": "dGV4dA==" },
  });
  const assetsRequest = requests.find(({ url }) =>
    url.includes("/workers/assets/upload?base64=true"),
  ).request;
  const assetsForm = await assetsRequest.formData();
  const part = assetsForm.get("index.html");
  assert.ok(part instanceof File);
  assert.equal(part.name, "index.html");
  assert.match(part.type, /^text\/html(?:;charset=utf-8)?$/);
  assert.equal(await part.text(), "PGgxPm9rPC9oMT4=");
  assert.equal(assetsForm.get("plain.txt"), "dGV4dA==");
});

test("official APIError envelope is preserved", async () => {
  const { client } = await mockClient({
    responses: [
      new Response(
        JSON.stringify({
          success: false,
          result: null,
          errors: [{ code: 10000, message: "authentication error" }],
          messages: [],
        }),
        { status: 401, headers: { "content-type": "application/json" } },
      ),
    ],
  });
  await assert.rejects(
    () => client.openCompute.system.status(),
    (error) => {
      assert.ok(error instanceof APIError);
      assert.equal(error.status, 401);
      return true;
    },
  );
});

test("official retry behavior is preserved through the facade", async () => {
  const requests = [];
  const { client } = await mockClient({
    options: { maxRetries: 1 },
    // mockClient pushes every request; retry happens inside the transport.
  });
  const fetchCalls = [];
  const retryClient = createOpenComputeClient({
    apiToken: "test-token",
    baseURL: "https://compute.example/client/v4",
    maxRetries: 1,
    fetch: async (url, init) => {
      fetchCalls.push(String(url));
      if (fetchCalls.length === 1) return new Response("boom", { status: 500 });
      return new Response(
        JSON.stringify({ success: true, result: {}, errors: [], messages: [] }),
        { status: 200, headers: { "content-type": "application/json" } },
      );
    },
  });
  await retryClient.openCompute.system.status();
  assert.equal(fetchCalls.length, 2);
  assert.equal(requests.length, 0);
});

test("request timeout surfaces as the official APIConnectionTimeoutError", async () => {
  const { APIConnectionTimeoutError } = await import("cloudflare");
  const client = createOpenComputeClient({
    apiToken: "test-token",
    baseURL: "https://compute.example/client/v4",
    timeout: 50,
    maxRetries: 0,
    fetch: (url, init) =>
      new Promise((_resolve, reject) => {
        init?.signal?.addEventListener("abort", () =>
          reject(init.signal.reason),
        );
      }),
  });
  await assert.rejects(
    () => client.openCompute.system.status(),
    (error) => error instanceof APIConnectionTimeoutError,
  );
});

test("client construction validation matrix", () => {
  const valid = {
    apiToken: "test-token",
    baseURL: "https://compute.example/client/v4",
  };
  const fetch = async () => new Response("{}");
  assert.ok(createOpenComputeClient({ ...valid, fetch }));
  assert.ok(
    createOpenComputeClient({
      ...valid,
      baseURL: "http://127.0.0.1:18787/client/v4",
      fetch,
    }),
  );
  assert.ok(
    createOpenComputeClient({
      ...valid,
      baseURL: "http://[::1]:18787/client/v4",
      fetch,
    }),
  );
  const invalid = [
    { ...valid, apiToken: "" },
    { ...valid, apiToken: "  " },
    { ...valid, apiToken: "bad\n" },
    { ...valid, baseURL: "compute.example/client/v4" },
    { ...valid, baseURL: "https://compute.example/other/v4" },
    { ...valid, baseURL: "https://compute.example/client/v4?x=1" },
    { ...valid, baseURL: "https://user:pw@compute.example/client/v4" },
    { ...valid, baseURL: "https://compute.example/client/v4#frag" },
    { ...valid, baseURL: "http://compute.example/client/v4" },
    { ...valid, baseURL: "ftp://compute.example/client/v4" },
    { ...valid, defaultHeaders: { authorization: "Bearer other" }, fetch },
    {
      ...valid,
      defaultHeaders: { "X-Open-Compute-Internal": "1" },
      fetch,
    },
  ];
  for (const options of invalid) {
    assert.throws(
      () => createOpenComputeClient(options),
      (error) => error instanceof Error,
      JSON.stringify(options),
    );
  }
});

test(
  "unsupported surface stays unreachable in compiled consumers",
  { timeout: 300_000 },
  () => {
    const tsc =
      process.env.OPEN_COMPUTE_SDK_TSC ??
      join(repoRoot, "node_modules", ".bin", "tsc");
    const fixtures = [
      { file: "unsupported-top-level.ts", symbol: "aiGateway" },
      { file: "unsupported-sibling.ts", symbol: "search" },
      { file: "unsupported-leaf.ts", symbol: "bulkUpdate" },
      { file: "raw-generic-request.ts", symbol: "get" },
    ];
    for (const { file, symbol } of fixtures) {
      const result = spawnSync(
        tsc,
        [
          "--ignoreConfig",
          "--noEmit",
          "--strict",
          "--target",
          "es2024",
          "--module",
          "preserve",
          "--moduleResolution",
          "bundler",
          "--allowImportingTsExtensions",
          "--types",
          "node",
          `${repoRoot}packages/sdk/tests/negative/${file}`,
        ],
        { cwd: repoRoot, encoding: "utf8" },
      );
      assert.equal(
        result.error,
        undefined,
        `failed to spawn tsc for ${file}: ${result.error?.message ?? result.error}`,
      );
      assert.notEqual(
        result.status,
        0,
        `negative fixture ${file} unexpectedly compiled: ${result.stdout}`,
      );
      assert.match(`${result.stderr}${result.stdout}`, new RegExp(symbol));
    }
  },
);

test(
  "signature overrides compile without consumer casts",
  { timeout: 300_000 },
  () => {
    const tsc =
      process.env.OPEN_COMPUTE_SDK_TSC ??
      join(repoRoot, "node_modules", ".bin", "tsc");
    const result = spawnSync(
      tsc,
      [
        "--ignoreConfig",
        "--noEmit",
        "--strict",
        "--target",
        "es2024",
        "--module",
        "preserve",
        "--moduleResolution",
        "bundler",
        "--allowImportingTsExtensions",
        "--types",
        "node",
        `${repoRoot}packages/sdk/tests/positive/signature-overrides.ts`,
      ],
      { cwd: repoRoot, encoding: "utf8" },
    );
    assert.equal(result.status, 0, `${result.stderr}${result.stdout}`);
  },
);
