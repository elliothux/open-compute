import assert from "node:assert/strict";
import Cloudflare from "cloudflare";

const [baseURL, account_id] = process.argv.slice(2);
assert(baseURL && account_id, "Expected local API origin and account");
const client = new Cloudflare({
  baseURL,
  apiToken: "workflow-deployer",
  maxRetries: 0,
  timeout: 30_000,
});
const html =
  '<html><title>Actions</title><body><h1>rendered<span style="display:none">hidden</span></h1><a href="https://example.com/linked">link</a><button>Submit</button></body></html>';
const input = { account_id, html, cacheTTL: 0 };
assert.match(await client.browserRendering.content.create(input), /rendered/);
assert.match(
  await client.browserRendering.markdown.create(input),
  /# rendered/,
);
assert.deepEqual(await client.browserRendering.links.create(input), [
  "https://example.com/linked",
]);
const scrape = await client.browserRendering.scrape.create({
  ...input,
  elements: [{ selector: "h1" }],
});
// Generated Results is declared as an object; the endpoint documents an array of matches.
const results: unknown = scrape[0]?.results;
assert(Array.isArray(results));
const first: unknown = results[0];
assert(first && typeof first === "object" && "text" in first);
assert.equal(first.text, "rendered");
assert("html" in first);
assert.equal(first.html, 'rendered<span style="display:none">hidden</span>');
const snapshot = await client.browserRendering.snapshot.create({
  ...input,
  formats: ["content", "markdown", "accessibilityTree"],
});
assert.match(snapshot.content ?? "", /rendered/);
assert.match(snapshot.markdown ?? "", /^---\ntitle: "Actions"\n---\n/);
assert(snapshot.accessibilityTree);
assert(
  (await client.browserRendering.accessibilityTree.create(input))
    .accessibilityTree,
);
assert.deepEqual(
  await client.browserRendering.json.create({
    ...input,
    prompt: "Extract the heading",
  }),
  {
    heading: "rendered",
  },
);
const pdf = await client.browserRendering.pdf.create(input);
for (const [authorization, expected] of [
  ["Bearer custom-request-key", "custom"],
  ["Bearer rejected-request-key", "fallback"],
] as const) {
  assert.deepEqual(
    await client.browserRendering.json.create({
      ...input,
      prompt: "Extract the heading",
      custom_ai: [
        { model: "openai/gpt-4o", authorization },
        {
          model: "anthropic/claude-sonnet-4-20250514",
          authorization: "fallback-request-key",
        },
      ],
    }),
    { heading: expected },
  );
}
await assert.rejects(
  () =>
    client.browserRendering.json.create({
      ...input,
      prompt: "Extract the heading",
      custom_ai: [
        {
          model: "openai/gpt-4o",
          authorization: "Bearer rejected-request-key",
        },
      ],
    }),
  (error: unknown) =>
    error instanceof Cloudflare.APIError &&
    error.status === 503 &&
    !error.message.includes("rejected-request-key"),
);
assert.match(pdf.headers.get("content-type") ?? "", /^application\/pdf/);
assert.equal(
  Buffer.from(await pdf.arrayBuffer())
    .subarray(0, 5)
    .toString(),
  "%PDF-",
);
// The SDK screenshot response declaration conflicts with the documented raw image body.
const screenshot = await client.browserRendering.screenshot
  .create({ ...input, viewport: { width: 320, height: 240 } })
  .asResponse();
assert.match(screenshot.headers.get("content-type") ?? "", /^image\/png/);
assert.equal(
  Buffer.from(await screenshot.arrayBuffer())
    .subarray(0, 8)
    .toString("hex"),
  "89504e470d0a1a0a",
);
for (const [query, body, expected] of [
  ["?cacheTTL=-1", { html }, 400],
  ["?cacheTTL=86401", { html }, 400],
  ["?cacheTTL=0&cacheTTL=1", { html }, 400],
  ["", { html, cacheTTL: 0 }, 400],
  ["?browser=kitesurf", { html }, 501],
] as const) {
  const response = await fetch(
    `${baseURL}/accounts/${account_id}/browser-rendering/content${query}`,
    {
      method: "POST",
      headers: {
        authorization: "Bearer workflow-deployer",
        "content-type": "application/json",
      },
      body: JSON.stringify(body),
    },
  );
  assert.equal(response.status, expected);
}
