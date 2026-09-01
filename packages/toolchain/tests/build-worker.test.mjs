import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";
import { compileWorker } from "../src/build-worker.ts";

const require = createRequire(resolve(import.meta.dirname, "../package.json"));
const nodeTypes = dirname(require.resolve("@types/node/package.json"));

const defaultConfig = {
  compilerOptions: {
    target: "ES2024", module: "Preserve", moduleResolution: "Bundler",
    lib: ["ES2024", "DOM"], types: [], strict: true,
    isolatedModules: true, verbatimModuleSyntax: true, noEmit: true,
  },
  include: ["*.ts"],
};

async function project(t, files, config = defaultConfig) {
  const root = await mkdtemp(join(tmpdir(), "open-compute-worker-build-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  for (const [name, source] of Object.entries({
    "package.json": JSON.stringify({ type: "module" }),
    "tsconfig.json": JSON.stringify(config),
    ...files,
  })) {
    await mkdir(dirname(join(root, name)), { recursive: true });
    await writeFile(join(root, name), source);
  }
  return { project: root, entry: "index.ts", tsconfig: "tsconfig.json" };
}

test("bundles TS dependencies and lazy imports while preserving Worker named exports", async t => {
  const options = await project(t, {
    "index.ts": `import { answer } from "./values.js";
      export class Counter { read(): number { return answer; } }
      export default { async fetch(): Promise<Response> {
        const { suffix } = await import("./lazy.js");
        return new Response(String(answer) + suffix);
      } };`,
    "values.ts": "export const answer: number = 42;",
    "lazy.ts": 'export const suffix: string = "!";',
  });
  const compiled = await compileWorker(options);
  assert.equal(compiled.mainModule, "worker.js");
  assert.ok(compiled.modules.length > 1);
  const outputDirectory = join(options.project, "built");
  for (const module of compiled.modules) {
    assert.equal(module.type, "esModule");
    const outputPath = join(outputDirectory, module.name);
    await mkdir(dirname(outputPath), { recursive: true });
    await writeFile(outputPath, module.bytes);
  }
  const worker = await import(pathToFileURL(join(outputDirectory, compiled.mainModule)).href);
  assert.equal(new worker.Counter().read(), 42);
  assert.equal(await (await worker.default.fetch()).text(), "42!");
  assert.deepEqual(await compileWorker(options), compiled);
});

test("does not execute Worker code while compiling", async t => {
  const options = await project(t, {
    "index.ts": 'throw new Error("tenant code executed during build"); export default {};',
  });
  const compiled = await compileWorker(options);
  assert.ok(compiled.modules.length > 0);
});

test("type checking and bundling choose the same workerd package export", async t => {
  const options = await project(t, {
    "index.ts": 'import { kind } from "conditional"; const value: "workerd" = kind; export default value;',
    "node_modules/conditional/package.json": JSON.stringify({
      name: "conditional", type: "module", exports: {
        workerd: { types: "./worker.d.ts", default: "./worker.js" },
        browser: { types: "./browser.d.ts", default: "./browser.js" },
        default: "./browser.js",
      },
    }),
    "node_modules/conditional/worker.d.ts": 'export const kind: "workerd";',
    "node_modules/conditional/worker.js": 'export const kind = "workerd";',
    "node_modules/conditional/browser.d.ts": 'export const kind: "browser";',
    "node_modules/conditional/browser.js": 'export const kind = "browser";',
  });
  const compiled = await compileWorker(options);
  const code = new TextDecoder().decode(compiled.modules[0].bytes);
  const worker = await import(`data:text/javascript;base64,${Buffer.from(code).toString("base64")}`);
  assert.equal(worker.default, "workerd");
});

test("type errors, excluded entries, and disabled checks cannot bypass validation", async t => {
  for (const [source, config] of [
    ['export const value: number = "wrong";', defaultConfig],
    ['export const value: number = "wrong";', { ...defaultConfig, include: ["other.ts"] }],
    ['export const value: number = "wrong";', {
      ...defaultConfig, compilerOptions: { ...defaultConfig.compilerOptions, noCheck: true },
    }],
    ["export const value: string = null;", {
      ...defaultConfig, compilerOptions: { ...defaultConfig.compilerOptions, strictNullChecks: false },
    }],
  ]) {
    const options = await project(t, { "index.ts": source, "other.ts": "export {};" }, config);
    await assert.rejects(compileWorker(options), /TypeScript validation|entry.*type.check/);
  }
});

test("pinned @types/node compiles default node: builtins without a tenant flag", async t => {
  const options = await project(t, {
    "index.ts": `import { Buffer } from "node:buffer";
      import { createHash } from "node:crypto";
      import { join } from "node:path";
      import process from "node:process";
      import { kind } from "conditional";
      const value: "workerd" = kind;
      export default { async fetch(): Promise<Response> {
        return Response.json({
          buffer: Buffer.from("node").toString(),
          digest: createHash("sha256").update("open-compute").digest("hex"),
          path: join("a", "b"),
          envKeys: Object.keys(process.env),
          kind: value,
        });
      } };`,
    "node_modules/conditional/package.json": JSON.stringify({
      name: "conditional", type: "module", exports: {
        workerd: { types: "./worker.d.ts", default: "./worker.js" },
        node: { types: "./node.d.ts", default: "./node.js" },
        browser: { types: "./browser.d.ts", default: "./browser.js" },
        default: "./browser.js",
      },
    }),
    "node_modules/conditional/worker.d.ts": 'export const kind: "workerd";',
    "node_modules/conditional/worker.js": 'export const kind = "workerd";',
    "node_modules/conditional/node.d.ts": 'export const kind: "node";',
    "node_modules/conditional/node.js": 'export const kind = "node";',
    "node_modules/conditional/browser.d.ts": 'export const kind: "browser";',
    "node_modules/conditional/browser.js": 'export const kind = "browser";',
  }, {
    compilerOptions: {
      target: "ES2024", module: "Preserve", moduleResolution: "Bundler",
      lib: ["ES2024"], types: ["node"], strict: true,
      isolatedModules: true, verbatimModuleSyntax: true, noEmit: true,
    },
    include: ["*.ts"],
  });
  await mkdir(join(options.project, "node_modules/@types"), { recursive: true });
  await symlink(nodeTypes, join(options.project, "node_modules/@types/node"));
  const compiled = await compileWorker(options);
  const code = new TextDecoder().decode(compiled.modules[0].bytes);
  assert.match(code, /from\s*["']node:buffer["']/);
  assert.match(code, /from\s*["']node:crypto["']/);
  assert.match(code, /from\s*["']node:path["']/);
  assert.match(code, /from\s*["']node:process["']/);
  assert.match(code, /["']workerd["']/);
  assert.doesNotMatch(code, /["']node["']\s*;/);
  await writeFile(join(options.project, "index.ts"), `
    import { connect } from "cloudflare:sockets";
    import { connect as netConnect } from "node:net";
    import { connect as tlsConnect } from "node:tls";
    declare const service: { connect(address: string): unknown };
    export default { connect, netConnect, tlsConnect, bindingConnect: service.connect };
  `);
  await writeFile(join(options.project, "modules.d.ts"), 'declare module "cloudflare:sockets" { export function connect(address: string): unknown; }');
  const sockets = await compileWorker(options);
  const socketCode = new TextDecoder().decode(sockets.modules[0].bytes);
  for (const specifier of ["cloudflare:sockets", "node:net", "node:tls"]) {
    assert.match(socketCode, new RegExp(`from\\s*["']${specifier.replace(":", "\\:")}["']`));
  }
  for (const specifier of ["cloudflare:unknown", "https://example.invalid/module.js"]) {
    await writeFile(join(options.project, "index.ts"), `import { value } from ${JSON.stringify(specifier)}; export default { value };`);
    await writeFile(join(options.project, "modules.d.ts"), `declare module ${JSON.stringify(specifier)} { export const value: string; }`);
    await assert.rejects(compileWorker(options), /unsupported|remote module/);
  }
});

test("entry and configuration paths cannot escape the selected project", async t => {
  const options = await project(t, { "index.ts": "export default {};" });
  const outside = await project(t, { "index.ts": "export default {};" });
  await assert.rejects(compileWorker({ ...options, entry: outside.project + "/index.ts" }), /inside the project/);
  await assert.rejects(compileWorker({ ...options, tsconfig: outside.project + "/tsconfig.json" }), /inside the project/);
  await symlink(join(outside.project, "index.ts"), join(options.project, "linked.ts"));
  await assert.rejects(compileWorker({ ...options, entry: "linked.ts" }), /inside the project/);
  assert.equal(await readFile(join(outside.project, "index.ts"), "utf8"), "export default {};");
});
