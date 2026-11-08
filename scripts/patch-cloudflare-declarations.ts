import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import specification from "./cloudflare-declaration-patches.json" with { type: "json" };

// Regenerate declaration-only Bun patch inputs from the pinned official packages.
const digest = (value: string) =>
  createHash("sha256").update(value).digest("hex");
const output = new Map<string, string>();

function checked(value: string, expected: string): string {
  assert.equal(digest(value), expected, "Official declaration input changed");
  return value;
}

function pipeline(ambient: string): string {
  const start = ambient.indexOf('declare module "cloudflare:pipelines" {');
  assert.ok(start >= 0);
  const end = ambient.indexOf("\n}", start) + 2;
  const block = checked(
    ambient.slice(start, end),
    specification.pipelineBlockSha256,
  );
  const copyright = ambient.slice(
    ambient.lastIndexOf("// Copyright", start),
    start,
  );
  const body = block.slice(block.indexOf("\n") + 1, -2);
  const lines = body.split("\n");
  assert.ok(lines.every((line) => !line || line.startsWith("  ")));
  const inner = lines.map((line) => line.slice(2)).join("\n");
  assert.equal(
    inner
      .split("\n")
      .map((line) => (line ? `  ${line}` : ""))
      .join("\n"),
    body,
  );
  return `${copyright}import type { ExecutionContext } from "@cloudflare/workers-types/index.ts";\n\n${inner}\n`;
}

function config(version: "0.22.0", original: string): void {
  const start = original.indexOf("//#region src/schema.d.ts");
  assert.ok(start >= 0);
  const end = original.indexOf("//#endregion", start) + "//#endregion".length;
  const schema = checked(
    original.slice(start, end),
    specification.schemaHashes[version],
  );
  const exports = original.slice(original.lastIndexOf("export { "));
  const exportedNames = new Set(
    exports
      .split("{")[1]
      ?.split("}")[0]
      ?.split(",")
      .map(
        (name) =>
          name
            .trim()
            .replace(/^type /, "")
            .split(" as ")[0],
      ),
  );
  const names = [
    ...schema.matchAll(
      /^(?:declare (?:const|function)|type|interface) (\w+)/gm,
    ),
  ].map((match) => match[1]);
  const publicNames = names.filter((name) => exportedNames.has(name));
  assert.equal(names.length, 44);
  assert.equal(publicNames.length, 42);
  const changed = `${original.slice(0, start)}import { ${publicNames.join(", ")} } from "./schema.mjs";${original.slice(end)}`;
  assert.equal(changed.slice(changed.lastIndexOf("export { ")), exports);
  assert.ok(!changed.includes(schema));
  output.set(`config-${version}/dist/index.d.mts`, changed);
  output.set(
    `config-${version}/dist/schema.d.mts`,
    `import * as z from "zod";\nimport type { SendEmailBinding, VpcNetworkBinding } from "./public.mjs";\n\n${schema}\nexport { ${publicNames.join(", ")} };\n`,
  );
}

async function generate(source: string, destination: string): Promise<void> {
  const inputs = new Map<string, string>();
  for (const [path, expected] of Object.entries(specification.inputs)) {
    inputs.set(
      path,
      checked(await readFile(resolve(source, path), "utf8"), expected),
    );
  }
  const get = (path: string) => {
    const value = inputs.get(path);
    assert.notEqual(value, undefined);
    return value ?? "";
  };
  output.set(
    "workers-types/build-tools-pipelines.d.ts",
    pipeline(get("workers-types/index.d.ts")),
  );
  for (const version of ["0.22.0"] as const) {
    const prefix = `config-${version}`;
    config(version, get(`${prefix}/dist/index.d.mts`));
    // Add only the types export; retain all existing package metadata values.
    const metadata = get(`${prefix}/package.json`);
    const marker = '  "exports": {\n';
    assert.equal(metadata.split(marker).length, 2);
    // Match the official formatting used by Bun's committed patch.
    const parsed: unknown = JSON.parse(metadata);
    assert.ok(
      typeof parsed === "object" && parsed !== null && "exports" in parsed,
    );
    assert.ok(typeof parsed.exports === "object" && parsed.exports !== null);
    Object.assign(parsed.exports, {
      "./schema": { types: "./dist/schema.d.mts" },
    });
    output.set(
      `${prefix}/package.json`,
      JSON.stringify(parsed, null, 2).replace(
        /[^\x00-\x7f]/g,
        (character) =>
          `\\u${character.charCodeAt(0).toString(16).padStart(4, "0")}`,
      ),
    );
    const chunk = "public-DFY8wDEh.d.mts";
    output.set(
      `${prefix}/dist/${chunk}`,
      `import type { ${specification.workerGlobals.join(", ")} } from "@cloudflare/workers-types/index.ts";\n${get(`${prefix}/dist/${chunk}`).replace('from "cloudflare:pipelines"', 'from "@cloudflare/workers-types/build-tools-pipelines"')}`,
    );
  }
  output.set(
    "vite-plugin/dist/index.d.mts",
    get("vite-plugin/dist/index.d.mts")
      .replace(
        'import { InputWorkerConfig, ParsedInputWorkerConfig, WorkerConfig } from "@cloudflare/config";',
        'import { InputWorkerConfig, ParsedInputWorkerConfig } from "@cloudflare/config/schema";\nimport { WorkerConfig } from "@cloudflare/config/public";',
      )
      .replace(
        'from "@cloudflare/workers-utils"',
        'from "@cloudflare/workers-utils/compatibility-date"',
      ),
  );
  assert.deepEqual(
    [...output.keys()].sort(),
    Object.keys(specification.outputs).sort(),
  );
  for (const [path, value] of output) {
    checked(
      value,
      specification.outputs[path as keyof typeof specification.outputs],
    );
  }
  // Validate every input and output before writing any generated file.
  for (const [path, value] of output) {
    const target = resolve(destination, path);
    await mkdir(resolve(target, ".."), { recursive: true });
    await writeFile(target, value);
  }
  console.log(
    JSON.stringify({ generatedFiles: output.size, sourceIntegrity: true }),
  );
}

if (process.argv[2] === "--self-check") {
  const original = "official declaration\n";
  assert.equal(checked(original, digest(original)), original);
  assert.throws(() => checked(`${original}tampered`, digest(original)));
  assert.throws(() =>
    pipeline('declare module "cloudflare:pipelines" {\n  changed\n}'),
  );
  console.log("Declaration integrity self-check passed");
} else {
  const [source, destination, extra] = process.argv.slice(2);
  assert.ok(
    source && destination && !extra,
    "Usage: bun scripts/patch-cloudflare-declarations.ts ORIGINALS OUTPUT",
  );
  assert.notEqual(resolve(source), resolve(destination));
  await generate(source, destination);
}
