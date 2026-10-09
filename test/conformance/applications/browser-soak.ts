// Optional sustained local qualification; not scheduled by the workspace Gate.
import assert from "node:assert/strict";
import Cloudflare from "cloudflare";

const [baseURL, account_id, durationInput = "600000"] = process.argv.slice(2);
const apiToken = process.env.OPEN_COMPUTE_BROWSER_SOAK_TOKEN;
assert(
  baseURL && account_id && apiToken,
  "Expected local API URL, account and explicit soak token",
);
const origin = new URL(baseURL);
assert(
  origin.protocol === "http:" &&
    ["127.0.0.1", "[::1]"].includes(origin.hostname),
);
assert(!origin.username && !origin.password && !origin.search && !origin.hash);
assert.equal(origin.pathname, "/client/v4");
const duration = Number(durationInput);
assert(
  Number.isSafeInteger(duration) && duration >= 30000 && duration <= 3600000,
);
const client = new Cloudflare({
  baseURL,
  apiToken,
  maxRetries: 0,
  timeout: 30000,
});
const input = {
  account_id,
  html: "<html><body><h1>Browser soak</h1></body></html>",
  cacheTTL: 0,
};
const started = performance.now();
let calls = 0;
let batches = 0;
let lastReport = started;
while (performance.now() - started < duration) {
  const content = await Promise.all([
    client.browserRendering.content.create(input),
    client.browserRendering.content.create(input),
  ]);
  for (const value of content) assert.match(value, /<h1>Browser soak<\/h1>/);
  calls += content.length;
  batches++;
  if (batches % 10 === 0) {
    const response = await client.browserRendering.screenshot
      .create({
        ...input,
        viewport: { width: 320, height: 240 },
      })
      .asResponse();
    assert.match(response.headers.get("content-type") ?? "", /^image\/png/);
    assert.equal(
      Buffer.from(await response.arrayBuffer())
        .subarray(0, 8)
        .toString("hex"),
      "89504e470d0a1a0a",
    );
    calls++;
  }
  const settleDeadline = performance.now() + 10000;
  while (
    (await client.browserRendering.devtools.session.list({ account_id }))
      .length !== 0
  ) {
    assert(
      performance.now() < settleDeadline,
      "Completed actions leaked sessions",
    );
  }
  if (performance.now() - lastReport >= 30000) {
    console.log(
      JSON.stringify({
        elapsed_ms: Math.round(performance.now() - started),
        calls,
      }),
    );
    lastReport = performance.now();
  }
}
assert(calls >= 2);
console.log(
  JSON.stringify({
    status: "passed",
    elapsed_ms: Math.round(performance.now() - started),
    calls,
    active_sessions: 0,
  }),
);
