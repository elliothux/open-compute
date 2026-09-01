import assert from "node:assert/strict";
import { createServer } from "node:http";
import test from "node:test";
import { deployWorker } from "../src/deploy-worker.ts";

const account = "01900000-0000-7000-8000-000000000001";
const worker = "01900000-0000-7000-8000-000000000002";
const deployment = "01900000-0000-7000-8000-000000000003";
const collection = `/v1/accounts/${account}/workers`;
const artifact = { mainModule: "worker.js", bytes: Buffer.from("canonical-test-artifact"), sha256: "a".repeat(64) };
const workerRecord = { id: worker, accountId: account, name: "hello", deletedAtMs: null };
const route = { kind: "platform_path", workerId: worker, accountId: account, pathPrefix: "/worker/hello/" };
const ready = { promoted: true, deployment: { id: deployment, state: "ready", workerId: worker } };
const project = {
  project: "/unused", main: "index.ts", tsconfig: "tsconfig.json", name: "hello",
  vars: { GREETING: "你好 🌍" },
  secrets: { TOKEN: { env: "WORKER_TOKEN" } }, bindings: {},
  services: { SELF: { service: "hello" } }, endpoint: "http://127.0.0.1:1",
  runtimeFeatures: {
    cache: { enabled: false, crossVersionCache: false, entrypoints: {} },
  },
};

async function platform(t, handler) {
  const requests = [];
  const server = createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const record = { path: request.url, method: request.method, headers: request.headers, body: Buffer.concat(chunks) };
    requests.push(record);
    const result = handler(record);
    response.writeHead(result.status ?? 200, { "content-type": "application/json", ...result.headers });
    response.end(result.raw ?? JSON.stringify(result.body));
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  t.after(() => new Promise(resolve => { server.close(resolve); server.closeAllConnections(); }));
  return { endpoint: `http://127.0.0.1:${server.address().port}`, requests };
}

function responses(request, existing = []) {
  if (request.path === "/v1/account") return { body: { accountId: account } };
  if (request.path === collection) return { body: request.method === "GET" ? { workers: existing } : { worker: workerRecord } };
  if (request.path.endsWith("/deployments")) return { body: ready };
  if (request.path.endsWith("/routes")) return { body: { routes: [route] } };
  throw new Error(`unexpected test request ${request.path}`);
}

const options = { localOnly: true, token: "admin-secret", env: { WORKER_TOKEN: "tenant-secret-中文" } };

test("uses the authoritative account and route and sends secrets only in authenticated metadata", async t => {
  const server = await platform(t, request => responses(request));
  const result = await deployWorker(project, artifact, { ...options, endpoint: server.endpoint });
  assert.deepEqual(result, { workerId: worker, deploymentId: deployment, url: `${server.endpoint}/worker/hello/`, sha256: artifact.sha256 });
  assert.deepEqual(server.requests.map(item => item.method), ["GET", "GET", "POST", "POST", "GET"]);
  const posts = server.requests.filter(item => item.method === "POST");
  assert.notEqual(posts[0].headers["idempotency-key"], posts[1].headers["idempotency-key"]);
  for (const request of server.requests) {
    assert.equal(request.headers.authorization, "Bearer admin-secret");
    assert.doesNotMatch(request.path, /secret/);
    assert.doesNotMatch(request.body.toString(), /secret/);
  }
  const sent = posts[1];
  assert.deepEqual(sent.body, artifact.bytes);
  assert.match(sent.headers["x-open-compute-deployment-metadata"], /^[\x20-\x7e]+$/);
  const metadata = JSON.parse(sent.headers["x-open-compute-deployment-metadata"]);
  assert.deepEqual(metadata.vars, project.vars);
  assert.deepEqual(metadata.secrets, { TOKEN: options.env.WORKER_TOKEN });
  assert.deepEqual(metadata.services, { SELF: { targetWorkerId: worker } });
  assert.deepEqual(metadata.cache, {
    enabled: false, crossVersionCache: false, entrypoints: {},
  });
  assert.equal(metadata.promote, true);
});

test("redeployment reuses an existing Worker and explicit account without creating another", async t => {
  const server = await platform(t, request => responses(request, [workerRecord]));
  await deployWorker(project, artifact, { ...options, endpoint: server.endpoint, accountId: account });
  assert.equal(server.requests.length, 3);
  assert.deepEqual(server.requests.map(item => item.method), ["GET", "POST", "GET"]);
});

test("invalid destinations and missing secrets fail before any network mutation", async () => {
  for (const endpoint of ["http://example.invalid", "https://user:password@example.invalid", "https://example.invalid/path", "https://example.invalid/?token=x"]) {
    await assert.rejects(deployWorker(project, artifact, { ...options, localOnly: false, endpoint }), /endpoint/);
  }
  await assert.rejects(deployWorker(project, artifact, { ...options, endpoint: "https://example.invalid" }), /local platform/);
  await assert.rejects(deployWorker(project, artifact, { ...options, env: {} }), /missing secret environment reference/);
  await assert.rejects(deployWorker(project, artifact, { ...options, token: "not\na-token" }), /authentication token/);
});

test("redirects and failed responses never echo secrets or trigger retries", async t => {
  for (const response of [
    { status: 307, headers: { location: "http://127.0.0.1:1/private" }, body: "tenant-secret" },
    { status: 500, body: "tenant-secret" },
    { raw: "tenant-secret" },
    { raw: "x".repeat(1024 * 1024 + 1) },
  ]) {
    const server = await platform(t, () => response);
    await assert.rejects(deployWorker(project, artifact, { ...options, endpoint: server.endpoint }), error => {
      assert.doesNotMatch(error.message, /tenant-secret|admin-secret/);
      return true;
    });
    assert.equal(server.requests.length, 1);
  }
});

test("does not report success for unpromoted deployments or untrusted default routes", async t => {
  for (const override of [
    request => request.path.endsWith("/deployments") ? { body: { ...ready, promoted: false } } : undefined,
    request => request.path.endsWith("/routes") ? { body: { routes: [{ ...route, pathPrefix: "//example.invalid/" }] } } : undefined,
    request => request.path.endsWith("/routes") ? { body: { routes: [{ ...route, workerId: deployment }] } } : undefined,
  ]) {
    const server = await platform(t, request => override(request) ?? responses(request));
    await assert.rejects(deployWorker(project, artifact, { ...options, endpoint: server.endpoint }), /promoted|route/);
  }
});
