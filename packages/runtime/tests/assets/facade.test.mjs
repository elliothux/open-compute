import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime } from "../compiled-runtime.mjs";

test("asset facade preserves fetch URL, method, and headers across the RPC boundary", async () => {
  const calls = [];
  const { AssetsBinding } = await importRuntime("assets/facade.ts");
  const binding = new AssetsBinding({
    async fetchAsset(request) {
      calls.push(request);
      return new Response("asset");
    },
  });
  const response = await binding.fetch("https://assets.example.test/static.txt", {
    method: "HEAD",
    headers: { "if-none-match": '"digest"' },
  });
  assert.equal(await response.text(), "asset");
  assert.deepEqual(calls, [{
    url: "https://assets.example.test/static.txt",
    method: "HEAD",
    headers: [["if-none-match", '"digest"']],
  }]);
  assert.throws(() => new AssetsBinding({}), /ASSET_BINDING_UNAVAILABLE/);
});
