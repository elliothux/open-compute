import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime } from "../compiled-runtime.mjs";

const { AiBinding } = await importRuntime("ai/facade.ts");

function success(file, format = "markdown") {
  return {
    id: `id-${file.name}`, name: file.name, mimeType: file.mimeType || "text/plain",
    format, tokens: 2, data: format === "text" ? "hello" : "# hello",
  };
}

test("AI binding preserves direct and handle single/array overloads", async () => {
  const calls = [];
  const binding = new AiBinding({
    async transform(files, options) {
      calls.push({ files, options });
      return files.map(file => success(file, options.output?.format));
    },
    async supported() {
      return [
        { extension: ".pdf", mimeType: "application/pdf" },
        { extension: ".docx", mimeType: "application/vnd.openxmlformats-officedocument.wordprocessingml.document" },
      ];
    },
  });
  assert.equal(binding.aiGatewayLogId, null);
  const first = { name: "manual.pdf", blob: new Blob(["pdf"], { type: "application/pdf" }) };
  const second = { name: "notes.docx", blob: new Blob(["docx"], {
    type: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
  }) };
  assert.equal((await binding.toMarkdown(first)).name, "manual.pdf");
  const array = await binding.toMarkdown([first]);
  assert.ok(Array.isArray(array));
  assert.equal(array.length, 1);
  const converted = await binding.toMarkdown().transform([first, second], {
    conversionOptions: {
      output: { format: "text" },
      html: { hostname: "example.com/base/", cssSelector: "main, article.content" },
      pdf: { metadata: true },
    },
  });
  assert.deepEqual(converted.map(item => item.name), ["manual.pdf", "notes.docx"]);
  assert.equal(converted[0].format, "text");
  assert.deepEqual(calls[2].options, {
    output: { format: "text" },
    html: { hostname: "example.com/base/", cssSelector: "main, article.content" },
    pdf: { metadata: true },
  });
  assert.match(calls[0].files[0].dataBase64, /^[A-Za-z0-9+/]+=*$/);
  assert.deepEqual((await binding.toMarkdown().supported()).map(item => item.extension), [".docx", ".pdf"]);
});

test("AI binding validates documents, options, limits, and backend response fail closed", async () => {
  let calls = 0;
  const binding = new AiBinding({
    async transform(files) { calls += 1; return files.map(success); },
    async supported() { return []; },
  });
  const file = { name: "manual.pdf", blob: new Blob(["pdf"], { type: "application/pdf" }) };
  for (const options of [
    { gateway: { id: "gateway" } },
    { extraHeaders: { authorization: "secret" } },
    { conversionOptions: { image: { descriptionLanguage: "en" } } },
    { conversionOptions: { docx: { images: { convert: true } } } },
    { conversionOptions: { html: { images: { convert: true } } } },
    { conversionOptions: { pdf: { images: { convert: true } } } },
    { conversionOptions: { output: { format: "html" } } },
    { conversionOptions: { html: { hostname: "file:///etc/passwd" } } },
    { conversionOptions: { html: { cssSelector: "@import url(x)" } } },
  ]) await assert.rejects(binding.toMarkdown(file, options), /AI_OPTION_UNSUPPORTED/);
  await assert.rejects(binding.toMarkdown({ name: "bad\0name", blob: file.blob }), /AI_DOCUMENT_INVALID/);
  await assert.rejects(binding.toMarkdown({ name: "../manual.pdf", blob: file.blob }), /AI_DOCUMENT_INVALID/);
  await assert.rejects(binding.toMarkdown({ name: "manual.pdf", blob: new Blob(["pdf"]) }), /AI_DOCUMENT_INVALID/);
  await assert.rejects(binding.toMarkdown({ name: "empty", blob: new Blob([]) }), /AI_DOCUMENT_INVALID/);
  await assert.rejects(binding.toMarkdown({ name: "large", blob: new Blob([new Uint8Array(4 * 1024 * 1024 + 1)]) }), /AI_DOCUMENT_TOO_LARGE/);
  await assert.rejects(binding.toMarkdown(Array.from({ length: 17 }, () => file)), /AI_BATCH_TOO_LARGE/);
  assert.equal(calls, 0);

  const malformed = new AiBinding({
    async transform() { return [{ name: "manual.pdf", format: "markdown", data: "x" }]; },
    async supported() { return [{ extension: "pdf", mimeType: "application/pdf" }]; },
  });
  await assert.rejects(malformed.toMarkdown(file), /AI_PROTOCOL_ERROR/);
  await assert.rejects(malformed.toMarkdown().supported(), /AI_PROTOCOL_ERROR/);
  await assert.rejects(binding.run(), /AI_UNSUPPORTED/);
  await assert.rejects(binding.models(), /AI_UNSUPPORTED/);
  assert.throws(() => binding.gateway("gateway"), /AI_UNSUPPORTED/);
  assert.throws(() => binding.aiSearch(), /AI_UNSUPPORTED/);
  assert.throws(() => binding.autorag("instance"), /AI_UNSUPPORTED/);
});
