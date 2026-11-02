import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, relative } from "node:path";
import { baseline, json, record, ROOT, sha256, string } from "./context.ts";

export function typesAstEnv(): NodeJS.ProcessEnv {
  const tmp = join(ROOT, ".temp/bun-tmp");
  const transpile = join(ROOT, ".temp/bun-transpile");
  mkdirSync(tmp, { recursive: true });
  mkdirSync(transpile, { recursive: true });
  return {
    ...process.env,
    TMPDIR: tmp,
    BUN_RUNTIME_TRANSPILER_CACHE_PATH: transpile,
  };
}

export function fingerprintFile(path: string): {
  sha256: string;
  statements: number;
  lines: number;
} {
  const output = execFileSync(
    process.execPath,
    [join(ROOT, "test/conformance/types-ast.ts"), "fingerprint", path],
    {
      cwd: ROOT,
      encoding: "utf8",
      env: typesAstEnv(),
      timeout: 60_000,
      maxBuffer: 1024 * 1024,
    },
  );
  const value = record(JSON.parse(output), `fingerprint ${path}`);
  if (typeof value.statements !== "number" || typeof value.lines !== "number") {
    throw new Error(`fingerprint ${path} is malformed`);
  }
  return {
    sha256: string(value.sha256, "fingerprint.sha256"),
    statements: value.statements,
    lines: value.lines,
  };
}

export async function publicTypesSurface(): Promise<void> {
  const lock = record(
    json("packages/runtime/workerd.lock.json"),
    "workerd lock",
  );
  const lockTypes = record(lock.workersTypes, "lock.workersTypes");
  const baselineTypes = record(
    baseline().workersTypes,
    "baseline.workersTypes",
  );
  const workersTypesRoot = dirname(
    createRequire(join(ROOT, "packages/workers-types/package.json")).resolve(
      "@cloudflare/workers-types/package.json",
    ),
  );
  const packageJson = record(
    json(relative(ROOT, join(workersTypesRoot, "package.json"))),
    "workers-types package",
  );
  if (
    packageJson.version !== lockTypes.version ||
    packageJson.version !== "5.20260930.2"
  ) {
    throw new Error("installed @cloudflare/workers-types version drift");
  }
  const installedPath = join(workersTypesRoot, "index.d.ts");
  const workerdRoot = join(ROOT, "third_party/workerd");
  const installed = readFileSync(installedPath);
  const indexSha256 = string(
    baselineTypes.indexSha256,
    "workersTypes.indexSha256",
  );
  if (sha256(installed) !== indexSha256) {
    throw new Error("workers-types index digest drift");
  }
  const installedAst = fingerprintFile(installedPath);
  if (existsSync(join(workerdRoot, ".git"))) {
    // The npm declaration snapshot has its own immutable upstream identity. The fork's
    // runtime compatibility is qualified by product cases, not byte equality with newer types.
    const revision = string(lockTypes.gitHead, "lock.workersTypes.gitHead");
    const snapshot = execFileSync(
      "git",
      ["show", `${revision}:types/generated-snapshot/index.d.ts`],
      {
        cwd: workerdRoot,
        timeout: 10_000,
        maxBuffer: 8 * 1024 * 1024,
      },
    );
    if (sha256(snapshot) !== indexSha256) {
      throw new Error("workers-types index digest drift");
    }
    if (!installed.equals(snapshot)) {
      throw new Error(
        "npm workers-types and workerd generated snapshot are not byte-identical",
      );
    }
  }
  if (
    installedAst.sha256 !==
      string(lockTypes.astSha256, "lock.workersTypes.astSha256") ||
    installedAst.sha256 !==
      string(baselineTypes.astSha256, "baseline.workersTypes.astSha256")
  ) {
    throw new Error("workers-types AST digest drift");
  }
  if (installedAst.lines !== 18494 || installedAst.statements < 100) {
    throw new Error("upstream stable declaration is incomplete");
  }
  execFileSync(
    process.execPath,
    [
      join(ROOT, "test/conformance/types-ast.ts"),
      "thin-bridge",
      join(ROOT, "packages/workers-types/index.d.ts"),
    ],
    {
      cwd: ROOT,
      encoding: "utf8",
      env: typesAstEnv(),
      timeout: 60_000,
    },
  );
  const example = readFileSync(
    join(ROOT, "examples/hello-worker/tsconfig.json"),
    "utf8",
  );
  if (
    !example.includes(".cloudflare/types") ||
    example.includes("workers-types/experimental")
  ) {
    throw new Error("example does not consume the pinned stable type surface");
  }
  const fixtures = readFileSync(
    join(ROOT, "test/conformance/fixtures/tsconfig.json"),
    "utf8",
  );
  if (
    !fixtures.includes("@open-compute/workers-types") ||
    fixtures.includes("workers-types/experimental")
  ) {
    throw new Error(
      "tenant fixtures do not consume the pinned stable type surface",
    );
  }
}

export function compileFixtures(): void {
  execFileSync(
    join(ROOT, "node_modules/.bin/tsc"),
    [
      "--project",
      join(ROOT, "test/conformance/fixtures/tsconfig.json"),
      "--noEmit",
      "--pretty",
      "false",
    ],
    {
      cwd: ROOT,
      encoding: "utf8",
      timeout: 120_000,
      maxBuffer: 4 * 1024 * 1024,
    },
  );
}

export function conformanceSelfTests(): void {
  try {
    execFileSync(
      "node",
      [
        "--test",
        // p6-contract.test.mjs builds the Rust vendor schemas with Cargo, which
        // does not fit this 120 s target; CI's test:js:ci runs it after Cargo.
        join(ROOT, "test/conformance/adapters.test.mjs"),
        join(ROOT, "test/conformance/case-evidence.test.mjs"),
        join(ROOT, "test/conformance/inventory.test.mjs"),
        join(ROOT, "test/conformance/p23-cf-wire.test.mjs"),
      ],
      {
        cwd: ROOT,
        encoding: "utf8",
        env: typesAstEnv(),
        timeout: 120_000,
        maxBuffer: 4 * 1024 * 1024,
      },
    );
  } catch (error) {
    const failure = error as {
      message?: unknown;
      stderr?: unknown;
      stdout?: unknown;
    };
    throw new Error(
      [failure.message, failure.stderr, failure.stdout]
        .filter(Boolean)
        .join("\n"),
    );
  }
}
