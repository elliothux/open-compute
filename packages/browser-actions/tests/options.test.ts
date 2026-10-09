import assert from "node:assert/strict";
import test from "node:test";
import { cached, cacheKey, remember } from "../src/cache.js";
import { actionRequest, defined } from "../src/options.js";

const source = {
  sessionId: "019a1700-0000-7000-8000-000000000001",
  maxResultBytes: 4096,
};
test("action schemas reject mixed sources, unknown options and unsupported action fields", () => {
  for (const action of [
    "content",
    "screenshot",
    "pdf",
    "links",
    "snapshot",
    "markdown",
    "accessibilityTree",
  ]) {
    assert.equal(
      actionRequest.safeParse({
        ...source,
        action,
        options: { html: "<title>bounded</title>" },
      }).success,
      true,
    );
  }
  assert.equal(
    actionRequest.safeParse({
      ...source,
      action: "scrape",
      options: { html: "<p>text</p>", elements: [{ selector: "p" }] },
    }).success,
    true,
  );
  for (const options of [
    {},
    { html: "x", url: "https://example.com" },
    { url: "file:///etc/passwd" },
    { html: "x", browser: "kitesurf" },
    { html: "x", pdfOptions: {} },
    {
      html: "x",
      rejectResourceTypes: ["script"],
      allowResourceTypes: ["script"],
    },
    { html: "x", cacheTTL: 86401 },
    { html: "x", rejectRequestPattern: ["["] },
  ]) {
    assert.equal(
      actionRequest.safeParse({ ...source, action: "content", options })
        .success,
      false,
    );
  }
  assert.equal(
    actionRequest.safeParse({
      ...source,
      action: "snapshot",
      options: { html: "x", formats: ["content", "content"] },
    }).success,
    false,
  );
  const custom = {
    model: "openai/gpt-4o",
    authorization: "Bearer request-key",
  };
  const extraction = { html: "<h1>text</h1>", prompt: "extract" };
  assert.equal(
    actionRequest.safeParse({
      ...source,
      action: "json",
      options: {
        ...extraction,
        custom_ai: [{ model: "workers-ai/@cf/meta/model" }],
      },
    }).success,
    true,
  );
  assert.equal(
    actionRequest.safeParse({
      ...source,
      action: "json",
      options: {
        ...extraction,
        custom_ai: [{ model: "openai/gpt-4o" }],
      },
    }).success,
    false,
  );

  for (const authorization of [
    "request-key",
    "Bearer request-key",
    "bearer request-key",
  ]) {
    assert.equal(
      actionRequest.safeParse({
        ...source,
        action: "json",
        options: {
          ...extraction,
          custom_ai: [{ ...custom, authorization }],
        },
      }).success,
      true,
    );
  }
  for (const custom_ai of [
    [],
    null,
    Array.from({ length: 4 }, () => custom),
    [{ ...custom, endpoint: "https://other.example/chat" }],
    ...["tenant", "/model", "openai/", "openai/a b"].map((model) => [
      { ...custom, model },
    ]),
    ...[
      "",
      "Bearer ",
      "Bearer a b",
      "key\r\nheader: injected",
      "x".repeat(4097),
    ].map((authorization) => [{ ...custom, authorization }]),
  ]) {
    assert.equal(
      actionRequest.safeParse({
        ...source,
        action: "json",
        options: {
          ...extraction,
          custom_ai,
        },
      }).success,
      false,
    );
  }
  assert.equal(
    actionRequest.safeParse({
      ...source,
      action: "content",
      options: {
        html: "x",
        custom_ai: [custom],
      },
    }).success,
    false,
  );
  assert.equal(
    actionRequest.safeParse({
      ...source,
      action: "scrape",
      options: { html: "x" },
    }).success,
    false,
  );
  assert.equal(
    actionRequest.safeParse({
      ...source,
      action: "json",
      options: { html: "x" },
    }).success,
    false,
  );
  assert.equal(
    actionRequest.safeParse({
      ...source,
      action: "json",
      options: { html: "<p>text</p>", prompt: "Extract text" },
    }).success,
    true,
  );
  assert.equal(
    actionRequest.safeParse({
      ...source,
      action: "json",
      options: {
        html: "<p>text</p>",
        response_format: { type: "json_object" },
      },
    }).success,
    true,
  );
  assert.equal(
    actionRequest.safeParse({
      ...source,
      action: "json",
      options: {
        html: "x",
        prompt: "extract",
        custom_ai: [{ model: "tenant", authorization: "tenant-secret" }],
      },
    }).success,
    false,
  );
  assert.deepEqual(
    defined({
      outer: { present: 1, absent: undefined },
      items: [{ missing: undefined }],
    }),
    { outer: { present: 1 }, items: [{}] },
  );
});

test("cache isolates hashed options, disables zero TTL and enforces current result bounds", async () => {
  const extraction = {
    html: "same page",
    custom_ai: [{ model: "openai/gpt-4o", authorization: "first-request-key" }],
  };
  const first = await cacheKey("json", extraction);
  const second = await cacheKey("json", {
    ...extraction,
    custom_ai: [
      { ...extraction.custom_ai[0], authorization: "second-request-key" },
    ],
  });
  assert.notEqual(first, second);
  assert.match(first, /^[0-9a-f]{64}$/);
  assert(!first.includes("first-request-key"));
  const key = await cacheKey("content", { html: "secret webpage" });
  assert.match(key, /^[0-9a-f]{64}$/);
  assert.notEqual(key, await cacheKey("content", { html: "other" }));
  const bytes = new TextEncoder().encode("cached").buffer;
  remember(key, bytes, new Headers({ "content-type": "text/plain" }), 0, 32);
  assert.equal(cached(key, 32), undefined);
  remember(key, bytes, new Headers({ "content-type": "text/plain" }), 5, 32);
  assert.equal(cached(key, 2), undefined);
  const hit = cached(key, 32);
  assert.equal(await hit?.text(), "cached");
  assert.equal(hit?.headers.get("x-browser-ms-used"), "0");
  const oversized = await cacheKey("content", "oversized");
  remember(oversized, new ArrayBuffer(33), new Headers(), 5, 32);
  assert.equal(cached(oversized, 64), undefined);
});
