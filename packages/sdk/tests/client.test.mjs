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
  assert.equal(surface.operations.length, 145);
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

test("Worker settings edit preserves empty and nested bindings as one JSON multipart part", async () => {
  const { client, requests } = await mockClient();
  for (const bindings of [
    [],
    [{ type: "json", name: "CONFIG", json: { nested: [1, 2] } }],
    [{ type: "worker_loader", name: "LOADER" }],
    [
      {
        type: "service",
        name: "TARGET",
        service: "worker-b",
        entrypoint: "NamedEntrypoint",
        props: { tenant: "example" },
      },
    ],
  ]) {
    await client.workers.scripts.scriptAndVersionSettings.edit("test-worker", {
      account_id: "test-account",
      settings: {
        bindings,
        annotations: { "workers/message": "saved settings" },
      },
    });
  }
  assert.equal(requests.length, 4);
  for (const [index, request] of requests.entries()) {
    assert.equal(request.request.method, "PATCH");
    assert.equal(
      request.url,
      "https://compute.example/client/v4/accounts/test-account/workers/scripts/test-worker/settings",
    );
    const form = await request.request.formData();
    assert.deepEqual([...form.keys()], ["settings"]);
    const part = form.get("settings");
    assert.match(part.type, /^application\/json(?:;|$)/);
    assert.deepEqual(
      JSON.parse(await part.text()).bindings,
      index === 0
        ? []
        : index === 1
          ? [{ type: "json", name: "CONFIG", json: { nested: [1, 2] } }]
          : index === 2
            ? [{ type: "worker_loader", name: "LOADER" }]
            : [
                {
                  type: "service",
                  name: "TARGET",
                  service: "worker-b",
                  entrypoint: "NamedEntrypoint",
                  props: { tenant: "example" },
                },
              ],
    );
    assert.equal(
      JSON.parse(await part.text()).annotations["workers/message"],
      "saved settings",
    );
  }
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

test("queue message and beta version delete delegates preserve official wire", async () => {
  const { client, requests } = await mockClient();
  await client.queues.messages.push("queue/1", {
    account_id: "acc/1",
    body: { job: 42 },
    content_type: "json",
  });
  await client.queues.messages.bulkPush("queue/1", {
    account_id: "acc/1",
    messages: [{ body: "one", content_type: "text" }],
  });
  await client.workers.beta.workers.versions.delete("version/1", {
    account_id: "acc/1",
    worker_id: "worker/1",
  });
  assert.deepEqual(
    requests.map(({ url, request }) => [request.method, url]),
    [
      [
        "POST",
        "https://compute.example/client/v4/accounts/acc%2F1/queues/queue%2F1/messages",
      ],
      [
        "POST",
        "https://compute.example/client/v4/accounts/acc%2F1/queues/queue%2F1/messages/batch",
      ],
      [
        "DELETE",
        "https://compute.example/client/v4/accounts/acc%2F1/workers/workers/worker%2F1/versions/version%2F1",
      ],
    ],
  );
  assert.deepEqual(await requests[0].request.json(), {
    body: { job: 42 },
    content_type: "json",
  });
  assert.deepEqual(await requests[1].request.json(), {
    messages: [{ body: "one", content_type: "text" }],
  });
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

test("R2 usage unwraps current values and encodes the bucket name", async () => {
  const { client, requests } = await mockClient({
    responses: [
      new Response(
        JSON.stringify({
          success: true,
          result: { object_count: 2, size_bytes: null },
          errors: [],
          messages: [],
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      ),
    ],
  });
  const usage = await client.openCompute.r2.usage.get("acc/1", "bucket name");
  assert.deepEqual(usage, { object_count: 2, size_bytes: null });
  assert.equal(
    requests[0].url,
    "https://compute.example/client/v4/accounts/acc%2F1/open-compute/r2/buckets/bucket%20name/usage",
  );
  assert.equal(requests[0].request.method, "GET");
});

test("R2 multipart vendor upload sends the binary part unchanged", async () => {
  const { client, requests } = await mockClient({
    responses: [
      new Response(
        JSON.stringify({
          success: true,
          result: { partNumber: 1, etag: "etag" },
          errors: [],
          messages: [],
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      ),
    ],
  });
  const result = await client.openCompute.r2.multipart.uploadPart(
    "acc/1",
    "bucket",
    "upload",
    "1",
    "dir/file.txt",
    new Uint8Array([1, 2, 3]),
  );
  assert.deepEqual(result, { partNumber: 1, etag: "etag" });
  assert.equal(requests[0].request.method, "PUT");
  assert.equal(
    requests[0].url,
    "https://compute.example/client/v4/accounts/acc%2F1/open-compute/r2/buckets/bucket/multipart-uploads/upload/parts/1/dir%2Ffile.txt",
  );
  assert.deepEqual(
    [...new Uint8Array(await requests[0].request.arrayBuffer())],
    [1, 2, 3],
  );
});

test("AI Search upload keeps browser File folder paths in multipart filenames", async () => {
  const { client, requests } = await mockClient();
  for (const folder of ["docs", "other"]) {
    await client.aiSearch.namespaces.instances.items.upload("instance", {
      account_id: "account",
      name: "default",
      file: {
        file: new File([folder], `${folder}/same.txt`, {
          type: "text/plain",
        }),
        metadata: JSON.stringify({ folder }),
        wait_for_completion: false,
      },
    });
  }
  assert.equal(requests.length, 2);
  for (const [index, folder] of ["docs", "other"].entries()) {
    const request = requests[index].request;
    assert.equal(request.method, "POST");
    assert.equal(request.headers.get("authorization"), "Bearer test-token");
    assert.equal(
      request.url,
      "https://compute.example/client/v4/accounts/account/ai-search/namespaces/default/instances/instance/items",
    );
    const body = await request.text();
    assert.ok(body.includes(`filename="${folder}/same.txt"`));
    assert.ok(body.includes(`name="metadata"`));
    assert.ok(body.includes(`{\"folder\":\"${folder}\"}`));
    assert.ok(body.includes(`name="wait_for_completion"`));
  }
});

test("D1 rename uses the vendor PATCH contract", async () => {
  const { client, requests } = await mockClient({
    responses: [
      new Response(
        JSON.stringify({
          success: true,
          result: { id: "db/id", name: "renamed-db" },
          errors: [],
          messages: [],
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      ),
    ],
  });
  const renamed = await client.openCompute.d1.rename("acc/1", "db/id", {
    name: "renamed-db",
  });
  assert.deepEqual(renamed, { id: "db/id", name: "renamed-db" });
  assert.equal(requests[0].request.method, "PATCH");
  assert.equal(
    requests[0].url,
    "https://compute.example/client/v4/accounts/acc%2F1/open-compute/d1/databases/db%2Fid/name",
  );
  assert.deepEqual(await requests[0].request.json(), { name: "renamed-db" });
});

test("D1 retained checkpoints use the vendor GET contract", async () => {
  const { client, requests } = await mockClient({
    responses: [
      new Response(
        JSON.stringify({
          success: true,
          result: { checkpoints_ms: [100, 200] },
          errors: [],
          messages: [],
        }),
        { status: 200, headers: { "content-type": "application/json" } },
      ),
    ],
  });
  const result = await client.openCompute.d1.timeTravel.checkpoints(
    "acc/1",
    "db/id",
  );
  assert.deepEqual(result, { checkpoints_ms: [100, 200] });
  assert.equal(requests[0].request.method, "GET");
  assert.equal(
    requests[0].url,
    "https://compute.example/client/v4/accounts/acc%2F1/open-compute/d1/databases/db%2Fid/time-travel/checkpoints",
  );
});

test("worker uploads send one JSON metadata part and named module parts", async () => {
  const { client, requests } = await mockClient();
  const module = new File(["export default {}"], "index.js", {
    type: "application/javascript+module",
  });
  await client.workers.scripts.update("app", {
    account_id: "account",
    metadata: {
      main_module: "index.js",
      bindings: [
        { type: "worker_loader", name: "LOADER" },
        { type: "d1", name: "DB", database_id: "database-id" },
        {
          type: "service",
          name: "CATALOG",
          service: "catalog",
          props: { mode: "read", nested: [1, true] },
        },
        {
          type: "artifacts",
          name: "ARTIFACTS",
          namespace: "team",
        },
      ],
      migrations: {
        old_tag: "v0",
        new_tag: "v2",
        steps: [
          { new_classes: ["Counter"] },
          { renamed_classes: [{ from: "Counter", to: "Total" }] },
        ],
      },
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
  const scriptMetadata = JSON.parse(scriptForm.get("metadata"));
  assert.equal(scriptMetadata.main_module, "index.js");
  assert.deepEqual(scriptMetadata.bindings[1], {
    type: "d1",
    name: "DB",
    id: "database-id",
  });
  assert.deepEqual(scriptMetadata.bindings[2].props, {
    mode: "read",
    nested: [1, true],
  });
  assert.equal(scriptMetadata.bindings[3].type, "artifacts");
  assert.equal(scriptMetadata.migrations.steps.length, 2);
  assert.equal(await scriptForm.get("index.js").text(), "export default {}");
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
    JSON.parse((await customRequest.formData()).get("metadata")).main_module,
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
  assert.equal(JSON.parse(versionForm.get("metadata")).main_module, "index.js");
  assert.equal(
    JSON.parse(versionForm.get("metadata")).bindings[0].type,
    "worker_loader",
  );
  assert.equal(await versionForm.get("index.js").text(), "export default {}");

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
