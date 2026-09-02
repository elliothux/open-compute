import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { generateEnvTypes, writeGeneratedTypes } from "../src/generate-types.ts";
import { loadProject } from "../src/project.ts";

async function fixture(t, config) {
  const directory = await mkdtemp(join(tmpdir(), "open-compute-types-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  await mkdir(join(directory, "src"), { recursive: true });
  await writeFile(join(directory, "src", "index.ts"), "export default {}; export class Admin {}");
  const filename = join(directory, "wrangler.jsonc");
  await writeFile(filename, JSON.stringify(config));
  return { directory, filename };
}

test("generates Env types from normalized Wrangler bindings", async t => {
  const { directory, filename } = await fixture(t, {
    name: "hello",
    main: "src/index.ts",
    compatibility_date: "2026-08-30",
    vars: { GREETING: "hello", COUNT: 42 },
    secrets: { required: ["TOKEN"] },
    kv_namespaces: [{ binding: "KV", id: "kv-id" }],
    services: [{ binding: "SELF", service: "hello" }, { binding: "ADMIN", service: "hello", entrypoint: "Admin" }],
    images: { binding: "IMAGES" },
    version_metadata: { binding: "VERSION" },
  });
  const output = join(directory, "worker-configuration.d.ts");
  const generated = generateEnvTypes(await loadProject(filename), output);
  assert.match(generated, /COUNT: 42;/);
  assert.match(generated, /TOKEN: string;/);
  assert.match(generated, /KV: KVNamespace;/);
  assert.match(generated, /SELF: Service<typeof import\("\.\/src\/index"\)\.default>;/);
  assert.match(generated, /ADMIN: Service<typeof import\("\.\/src\/index"\)\.Admin>;/);
  assert.match(generated, /IMAGES: ImagesBinding;/);
  assert.match(generated, /VERSION: WorkerVersionMetadata;/);
  assert.doesNotMatch(generated, /kv-id/);
});
test("fails closed on duplicate Env sources", async t => {
  const { directory, filename } = await fixture(t, {
    name: "hello",
    main: "src/index.ts",
    compatibility_date: "2026-08-30",
  });
  const base = await loadProject(filename);
  assert.throws(() => generateEnvTypes({ ...base, vars: { TOKEN: "public" }, secrets: ["TOKEN"] }, join(directory, "out.d.ts")), /duplicate Env property/);
});

test("atomically replaces only generated destinations", async t => {
  const { directory, filename } = await fixture(t, {
    name: "hello",
    main: "src/index.ts",
    compatibility_date: "2026-08-30",
  });
  const output = join(directory, "worker-configuration.d.ts");
  const generated = generateEnvTypes(await loadProject(filename), output);
  await writeGeneratedTypes(output, generated);
  await writeGeneratedTypes(output, generated);
  assert.equal(await readFile(output, "utf8"), generated);
  const linked = join(directory, "linked.d.ts");
  await symlink(output, linked);
  await assert.rejects(writeGeneratedTypes(linked, generated), /symbolic link/);
  const handmade = join(directory, "handmade.d.ts");
  await writeFile(handmade, "interface Env {}\n");
  await assert.rejects(writeGeneratedTypes(handmade, generated), /not generated/);
});
