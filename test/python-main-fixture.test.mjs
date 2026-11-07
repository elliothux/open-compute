import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import test from "node:test";
import { promisify } from "node:util";
import { repository, sha256 } from "../scripts/workerd-archive.ts";
import {
  captureEnvironment,
  describePythonUpload,
} from "./conformance/applications/prepare-python-main.ts";

async function upload(change = () => {}) {
  // Parser fixture only; this is never admitted as an SDK-generated Gate input.
  const form = new FormData();
  form.append(
    "metadata",
    JSON.stringify({
      main_module: "main.py",
      compatibility_date: "2026-09-08",
      compatibility_flags: ["python_workers"],
    }),
  );
  for (const [name, mime, bytes] of [
    ["main.py", "text/x-python", "pass\n"],
    ["greeting/__init__.py", "text/x-python", "message = 'hello'\n"],
    [
      "greeting/message.json",
      "application/octet-stream",
      '{"message":"hello"}',
    ],
    [
      "python_modules/workers/workers.mjs",
      "application/javascript+module",
      "export {};",
    ],
  ])
    form.append(name, new File([bytes], name, { type: mime }));
  change(form);
  const request = new Request("http://127.0.0.1/upload", {
    method: "POST",
    body: form,
  });
  return {
    contentType: request.headers.get("content-type"),
    body: new Uint8Array(await request.arrayBuffer()),
  };
}

test("Python fixture capture records original multipart and module bytes", async () => {
  const input = await upload();
  const result = await describePythonUpload(input.contentType, input.body);
  assert.equal(result.sha256, sha256(input.body));
  assert.equal(result.size, input.body.byteLength);
  assert.equal(result.contentType, input.contentType);
  assert.equal(result.modules.length, 4);
  assert.deepEqual(
    result.modules.find((m) => m.name === "main.py"),
    {
      name: "main.py",
      uploadName: "main.py",
      mime: "text/x-python",
      size: 5,
      sha256: sha256(Buffer.from("pass\n")),
    },
  );
});

test("Python fixture capture rejects incomplete, ambiguous and untyped inputs", async () => {
  for (const change of [
    (form) => form.delete("metadata"),
    (form) =>
      form.append(
        "./main.py",
        new File(["pass"], "./main.py", { type: "text/x-python" }),
      ),
    (form) =>
      form.append(
        "./../escape.py",
        new File(["pass"], "./../escape.py", { type: "text/x-python" }),
      ),
    (form) => form.delete("main.py"),
    (form) => form.delete("greeting/message.json"),
    (form) => form.delete("python_modules/workers/workers.mjs"),
    (form) =>
      form.set(
        "python_modules/workers/workers.mjs",
        new File(["export {};"], "python_modules/workers/workers.mjs", {
          type: "text/plain",
        }),
      ),
    (form) =>
      form.append(
        "main.py",
        new File(["pass"], "main.py", { type: "text/x-python" }),
      ),
    (form) =>
      form.set(
        "main.py",
        new File(["pass"], "main.py", { type: "text/plain" }),
      ),
    (form) =>
      form.append(
        "../escape.py",
        new File(["pass"], "../escape.py", { type: "text/x-python" }),
      ),
    (form) =>
      form.set(
        "metadata",
        '{"main_module":"main.py","compatibility_date":"2026-09-08","compatibility_flags":[]}',
      ),
  ]) {
    const input = await upload(change);
    await assert.rejects(describePythonUpload(input.contentType, input.body));
  }
  await assert.rejects(
    describePythonUpload("application/json", new Uint8Array()),
  );
});

test("Python capture uses cf with offline inputs and loopback-only API credentials", () => {
  const env = captureEnvironment("http://127.0.0.1:12345");
  assert.equal(env.CLOUDFLARE_API_BASE_URL, "http://127.0.0.1:12345/client/v4");
  assert.equal(env.UV_PYTHON_DOWNLOADS, "never");
  assert.equal(env.UV_OFFLINE, "true");
  assert.equal(env.CF_SEND_TELEMETRY, "false");
  assert.equal(env.NODE_DISABLE_COMPILE_CACHE, "1");
  assert.equal(env.CF_API_BASE_URL, undefined);
  assert.equal(env.CLOUDFLARE_API_KEY, undefined);
  for (const origin of [
    "https://api.cloudflare.com",
    "http://localhost:12345",
    "http://127.0.0.1:12345/client/v4",
    "http://token@127.0.0.1:12345",
  ])
    assert.throws(() => captureEnvironment(origin));
});

test("framework captures require their own package data and preserve the Python main", async () => {
  for (const [fixture, data] of [
    ["django", "app_urls.py"],
    ["flask", "templates/page.html"],
    ["fastapi", undefined],
    ["services", undefined],
    ["queues", undefined],
    ["workflows", undefined],
    [
      "runtime",
      [
        "cache_cases.py",
        "image_cases.py",
        "ai_cases.py",
        "vector_cases.py",
        "artifact_cases.py",
        "search_cases.py",
        "http_client_cases.py",
      ],
    ],
    ["durableobjects", undefined],
    ["durableobjects-retired", undefined],
  ]) {
    const input = await upload((form) => {
      form.delete("greeting/__init__.py");
      form.delete("greeting/message.json");
      for (const file of data ? (Array.isArray(data) ? data : [data]) : [])
        form.append(
          file,
          new File(["fixture data"], file, {
            type: file.endsWith(".py")
              ? "text/x-python"
              : "application/octet-stream",
          }),
        );
    });
    const result = await describePythonUpload(
      input.contentType,
      input.body,
      fixture,
    );
    assert.equal(result.metadata.main_module, "main.py");
    assert.equal(
      result.modules.length,
      2 + (Array.isArray(data) ? data.length : data ? 1 : 0),
    );
    if (data) {
      const missing = await upload((form) => {
        form.delete("greeting/__init__.py");
        form.delete("greeting/message.json");
      });
      await assert.rejects(
        describePythonUpload(missing.contentType, missing.body, fixture),
      );
    }
  }
});

test("runtime capture requires each maintained binding module", async () => {
  const modules = [
    "cache_cases.py",
    "image_cases.py",
    "ai_cases.py",
    "vector_cases.py",
    "artifact_cases.py",
    "search_cases.py",
  ];
  for (const missing of modules) {
    const input = await upload((form) => {
      for (const present of modules.filter((name) => name !== missing))
        form.append(
          present,
          new File(["fixture data"], present, { type: "text/x-python" }),
        );
    });
    await assert.rejects(
      describePythonUpload(input.contentType, input.body, "runtime"),
    );
  }
});

test("Python capture retains an SDK JavaScript module without renaming it", async () => {
  const input = await upload((form) => {
    form.delete("python_modules/workers/workers.mjs");
    form.append(
      "python_modules/workers/workflows.js",
      new File(["export {};"], "python_modules/workers/workflows.js", {
        type: "application/javascript+module",
      }),
    );
  });
  const result = await describePythonUpload(input.contentType, input.body);
  assert.ok(
    result.modules.some(
      (m) => m.name === "python_modules/workers/workflows.js",
    ),
  );
});

test("Python capture records cf wire names and canonical module identity", async () => {
  const input = await upload((form) => {
    const name = "greeting/message.json";
    const bytes = form.get(name);
    form.delete(name);
    form.append(
      `./${name}`,
      new File([bytes], `./${name}`, { type: "application/octet-stream" }),
    );
  });
  const result = await describePythonUpload(input.contentType, input.body);
  const data = result.modules.find((m) => m.name === "greeting/message.json");
  assert.equal(data.uploadName, "./greeting/message.json");
  assert.equal(data.sha256, sha256(Buffer.from('{"message":"hello"}')));
});

test("cf captures a prebuilt Python protocol fixture through its real version uploader", async () => {
  // Synthetic protocol input only: this does not qualify SDK bundling or Python execution.
  const base = join(repository, ".temp/python-main-capture-test/failed");
  await mkdir(base, { recursive: true });
  const root = await mkdtemp(join(base, "cf-"));
  const project = join(root, "project");
  const output = join(project, ".cloudflare/output/v0");
  const worker = join(output, "workers/default");
  const bundle = join(worker, "bundle");
  const input = await upload();
  const parts = await new Request("http://127.0.0.1", {
    method: "POST",
    headers: { "content-type": input.contentType },
    body: Buffer.from(input.body),
  }).formData();
  const modules = {};
  for (const [name, part] of parts) {
    if (name === "metadata") continue;
    const path = join(bundle, name);
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, Buffer.from(await part.arrayBuffer()));
    modules[name] = {
      type:
        part.type === "text/x-python"
          ? "python"
          : part.type === "application/octet-stream"
            ? "data"
            : "esm",
    };
  }
  await writeFile(
    join(output, "config.json"),
    JSON.stringify({ buildContext: { isPreview: false } }),
  );
  await writeFile(
    join(worker, "worker.config.json"),
    JSON.stringify({
      name: "python-main-fixture",
      compatibilityDate: "2026-09-08",
      compatibilityFlags: ["python_workers"],
      env: {
        REVISION: { type: "text", value: "first" },
        TOKEN: { type: "secret" },
        KV: { type: "kv", id: "11111111111111111111111111111111" },
        DB: {
          type: "d1",
          id: "019c0000-0000-7000-8000-000000000006",
          name: "python-main-db",
        },
        BUCKET: { type: "r2", name: "python-main-bucket" },
      },
      manifest: { type: "complete", mainModule: "main.py", modules },
    }),
  );
  const result = join(root, "result");
  await promisify(execFile)(
    process.execPath,
    [
      join(repository, "test/conformance/applications/prepare-python-main.ts"),
      project,
      result,
    ],
    { env: { ...process.env, NODE_DISABLE_COMPILE_CACHE: "1" } },
  );
  const recorded = JSON.parse(
    await readFile(join(result, "manifest.json"), "utf8"),
  );
  assert.equal(recorded.producer, "cf workers versions create --prebuilt");
  assert.equal(recorded.cf, "1.0.0-beta.21");
  assert.equal(recorded.metadata.main_module, "main.py");
  assert.equal(recorded.modules.length, 4);
  const bindings = Object.fromEntries(
    recorded.metadata.bindings.map((binding) => [binding.name, binding]),
  );
  assert.equal(bindings.TOKEN.type, "secret_text");
  assert.equal(bindings.TOKEN.text, "python-capture-fixture-token");
  assert.equal(bindings.REVISION.text, "first");
  assert.equal(bindings.KV.type, "kv_namespace");
  assert.equal(bindings.DB.type, "d1");
  assert.equal(bindings.BUCKET.type, "r2_bucket");
  const multipart = await readFile(join(result, "upload.multipart"));
  assert.equal(sha256(multipart), recorded.sha256);
  assert.equal(
    recorded.modules.find((m) => m.name === "greeting/message.json").uploadName,
    "./greeting/message.json",
  );
  await rm(root, { recursive: true }); // Failure preserves the owned input and diagnostics.
});
