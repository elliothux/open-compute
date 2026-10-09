import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime } from "../compiled-runtime.mjs";

const { BrowserRun } = await importRuntime("browser/facade.ts");

test("BrowserRun forwards Fetcher requests and the nine fixed quick actions as Responses", async () => {
  const calls = [];
  const browser = new BrowserRun({
    async fetch(input, init) {
      calls.push([input, init]);
      return new Response("result", {
        headers: { "content-type": "image/png" },
      });
    },
  });
  const request = new Request("https://browser/v1/sessions");
  const response = await browser.fetch(request);
  assert.equal(calls[0][0], request);
  assert.equal(response.headers.get("content-type"), "image/png");
  for (const action of [
    "screenshot",
    "pdf",
    "content",
    "scrape",
    "links",
    "snapshot",
    "markdown",
    "json",
    "accessibilityTree",
  ]) {
    const options = { html: "<h1>Test</h1>" };
    assert.equal(
      await (await browser.quickAction(action, options)).text(),
      "result",
    );
    const [url, init] = calls.at(-1);
    assert.equal(url, `https://browser/v1/${action}`);
    assert.equal(init.method, "POST");
    assert.deepEqual(JSON.parse(init.body), options);
  }
  assert.deepEqual(Object.keys(browser), []);
  assert.throws(() => browser.quickAction("crawl", {}), /BROWSER_UNSUPPORTED/);
  for (const invalid of [undefined, null, {}, { fetch: "invalid" }])
    assert.throws(() => new BrowserRun(invalid), /BROWSER_UNAVAILABLE/);
});
