import assert from "node:assert/strict";
import Cloudflare from "cloudflare";

const baseURL = process.env.OPEN_COMPUTE_V4_BASE_URL;
const apiToken = process.env.OPEN_COMPUTE_V4_TOKEN;
const accountID = process.env.OPEN_COMPUTE_V4_ACCOUNT_ID;
const publicURL = process.env.OPEN_COMPUTE_P7_PUBLIC_URL;
const secret = process.env.OPEN_COMPUTE_P7_SECRET;
assert.ok(baseURL && apiToken && accountID && publicURL && secret);

const client = new Cloudflare({ apiToken, baseURL, maxRetries: 0 });
const prepared = await client.workers.observability.telemetry.liveTail({
  account_id: accountID,
  scriptId: "p6-wrangler-resource-gate",
  filterCombination: "and",
  filters: [{
    key: "$workers.preview.slug",
    type: "string",
    operation: "is_null",
  }],
});
const socket = new WebSocket(prepared.wsUrl);
await new Promise((resolve, reject) => {
  const timer = setTimeout(() => reject(new Error("Live Tail WebSocket open timed out")), 5_000);
  socket.addEventListener("open", () => {
    clearTimeout(timer);
    resolve();
  }, { once: true });
  socket.addEventListener("error", () => reject(new Error("Live Tail WebSocket failed")), {
    once: true,
  });
});

assert.deepEqual(await client.workers.observability.telemetry.liveTailHeartbeat({
  account_id: accountID,
  scriptId: "p6-wrangler-resource-gate",
}), {});

const eventPromise = new Promise((resolve, reject) => {
  const timer = setTimeout(() => reject(new Error("Live Tail event timed out")), 5_000);
  socket.addEventListener("message", ({ data }) => {
    const event = JSON.parse(String(data));
    if (JSON.stringify(event.source).includes("p7-tail-event")) {
      clearTimeout(timer);
      resolve(event);
    }
  });
});
const response = await fetch(publicURL, {
  headers: {
    authorization: `Bearer ${secret}`,
    "x-api-key": secret,
  },
});
assert.equal(response.status, 200);
await response.arrayBuffer();
const event = await eventPromise;
assert.deepEqual(Object.keys(event).sort(), ["$metadata", "$workers", "dataset", "source", "timestamp"]);
assert.equal(event.dataset, "");
assert.equal(event.$workers.scriptName, "p6-wrangler-resource-gate");
assert.equal(event.$workers.eventType, "fetch");
assert.equal(event.$metadata.type, "cf-worker-log");
assert.equal(event.$metadata.service, "p6-wrangler-resource-gate");
assert.ok(!JSON.stringify(event).includes(secret));
socket.close(1000);

const now = Date.now();
const timeframe = { from: now - 120_000, to: now + 60_000 };
const keys = await client.workers.observability.telemetry.keys({
  account_id: accountID,
  datasets: ["cloudflare-workers"],
  from: timeframe.from,
  to: timeframe.to,
});
assert.ok(keys.result.some(({ key, type }) => key === "$metadata.service" && type === "string"));
const values = await client.workers.observability.telemetry.values({
  account_id: accountID,
  datasets: ["cloudflare-workers"],
  key: "$metadata.service",
  timeframe,
  type: "string",
});
assert.ok(values.result.some(({ value }) => value === "p6-wrangler-resource-gate"));

const parameters = {
  datasets: ["cloudflare-workers"],
  filterCombination: "and",
  filters: [{
    key: "$metadata.message",
    type: "string",
    operation: "MATCH_REGEX",
    value: "p7-tail-event.*invoice",
  }],
};
const first = await client.workers.observability.telemetry.query({
  account_id: accountID,
  queryId: "p7-dashboard-events",
  timeframe,
  parameters,
  view: "events",
  limit: 1,
});
assert.equal(first.events.count, 1);
assert.equal(first.events.events[0].dataset, "cloudflare-workers");
assert.ok(!JSON.stringify(first).includes(secret));
const cursor = first.events.events[0].$metadata.id;
assert.equal(typeof cursor, "string");
const second = await client.workers.observability.telemetry.query({
  account_id: accountID,
  queryId: "p7-dashboard-events",
  timeframe,
  parameters,
  view: "events",
  limit: 1,
  offset: cursor,
});
assert.equal(second.events.count, 1);
assert.notEqual(second.events.events[0].$metadata.id, cursor);
const invocations = await client.workers.observability.telemetry.query({
  account_id: accountID,
  queryId: "p7-dashboard-invocations",
  timeframe,
  parameters,
  view: "invocations",
  limit: 20,
});
assert.ok(Object.keys(invocations.invocations).length > 0);
assert.ok(!JSON.stringify(invocations).includes(secret));

await assert.rejects(
  client.workers.observability.telemetry.query({
    account_id: accountID,
    queryId: "p7-unsupported-traces",
    timeframe,
    parameters: { datasets: ["cloudflare-workers"], filters: [] },
    view: "traces",
  }),
  (error) => error?.status === 501,
);
