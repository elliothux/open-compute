import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime } from "../compiled-runtime.mjs";

const { ImagesBinding } = await importRuntime("images/facade.ts");

test("Images binding preserves official synchronous chaining and result response shape", async () => {
  const calls = [];
  const binding = new ImagesBinding({
    async input(input) { calls.push(["input", input]); return "session"; },
    async info() { return { format: "png", fileSize: 4, width: 2, height: 2 }; },
    async transform(session, options) { calls.push(["transform", session, options]); },
    async draw(session, input, options) { calls.push(["draw", session, input, options]); },
    async output(session, options) {
      calls.push(["output", session, options]);
      return new Response("image", { headers: { "content-type": "image/webp" } });
    },
  });
  const source = new ReadableStream({ start(controller) { controller.close(); } });
  const overlay = new ReadableStream({ start(controller) { controller.close(); } });
  const result = await binding.input(source)
    .transform({ width: 20, fit: "cover", flip: "hv" })
    .draw(overlay, { left: 2, opacity: 0.5, composite: "normal" })
    .output({ format: "image/webp", anim: false });
  assert.equal(result.contentType(), "image/webp");
  const response = result.response({ headers: { "cache-control": "max-age=60" } });
  assert.equal(response.headers.get("cache-control"), "max-age=60");
  assert.equal(await response.text(), "image");
  assert.deepEqual(calls.map(call => call[0]), ["input", "transform", "draw", "output"]);
  assert.equal(calls[1][2].flip, "both");
  assert.equal(calls[2][3].blend, "normal");
  assert.equal(calls[3][2].format, "webp");
});

test("Images facade rejects unsupported public options before transport execution", async () => {
  let calls = 0;
  const binding = new ImagesBinding({
    async input() { calls += 1; return "session"; },
    async info() { return {}; },
    async transform() { calls += 1; },
    async draw() { calls += 1; },
    async output() { calls += 1; return new Response(); },
  });
  const source = () => new ReadableStream({ start(controller) { controller.close(); } });
  assert.throws(() => binding.input(source()).transform({ sharpen: 1 }), /IMAGE_OPTION_UNSUPPORTED/);
  assert.throws(() => binding.input(source(), { encoding: "base64" }), /IMAGE_OPTION_UNSUPPORTED/);
  assert.throws(() => binding.input(source()).draw(source(), { right: 1 }), /IMAGE_OPTION_UNSUPPORTED/);
  await assert.rejects(binding.input(source()).output({ format: "image/gif" }), /IMAGE_OPTION_UNSUPPORTED/);
  assert.equal(calls, 3, "only input session creation starts before chain validation");
});

test("Images facade rejects malformed info and output protocol values", async () => {
  const source = () => new ReadableStream({ start(controller) { controller.close(); } });
  const malformedInfo = new ImagesBinding({
    async input() { return "session"; },
    async info() { return { format: "png", fileSize: -1, width: 2, height: 2 }; },
    async transform() {}, async draw() {}, async output() { return new Response("image"); },
  });
  await assert.rejects(malformedInfo.info(source()), /IMAGE_PROTOCOL_ERROR/);

  const wrongOutput = new ImagesBinding({
    async input() { return "session"; },
    async info() { return { format: "png", fileSize: 1, width: 1, height: 1 }; },
    async transform() {}, async draw() {},
    async output() { return new Response("image", { headers: { "content-type": "image/png" } }); },
  });
  await assert.rejects(
    wrongOutput.input(source()).output({ format: "image/webp" }),
    /IMAGE_PROTOCOL_ERROR/,
  );
});
