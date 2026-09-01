import { execFile } from "node:child_process";
import { mkdir, mkdtemp, symlink, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { promisify } from "node:util";
import test from "node:test";

const execFileAsync = promisify(execFile);
const require = createRequire(resolve(import.meta.dirname, "../package.json"));
const compiler = resolve(dirname(require.resolve("typescript/package.json")), "bin/tsc");
const typesPackage = resolve(import.meta.dirname, "../../types");
const cloudflareTypes = resolve(typesPackage, "node_modules/@cloudflare/workers-types");
const fixture = resolve(import.meta.dirname, "../../../test/conformance/fixtures/r2/bucket-surface.ts");

test("strict compile fixture covers pinned R2 members and negative types", async t => {
  const directory = await mkdtemp(join(tmpdir(), "oc-r2-surface-"));
  t.after(() => import("node:fs/promises").then(fs => fs.rm(directory, { recursive: true, force: true })));
  const source = await import("node:fs/promises").then(fs => fs.readFile(fixture, "utf8"));
  await mkdir(join(directory, "node_modules/@cloudflare"), { recursive: true });
  await symlink(cloudflareTypes, join(directory, "node_modules/@cloudflare/workers-types"));
  await writeFile(join(directory, "surface.ts"), source);
  await writeFile(join(directory, "tsconfig.json"), JSON.stringify({
    compilerOptions: {
      target: "ES2024", module: "Preserve", moduleResolution: "Bundler",
      lib: ["ES2024"], types: ["@cloudflare/workers-types"], strict: true, noEmit: true, skipLibCheck: false,
    },
    include: ["surface.ts"],
  }));
  await execFileAsync(process.execPath, [compiler, "--project", "tsconfig.json", "--pretty", "false"], { cwd: directory });
});
