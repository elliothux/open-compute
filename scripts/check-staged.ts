import { spawnSync } from "node:child_process";

type Mode = "check" | "format" | "lint";

const mode = (process.argv[2] ?? "check") as Mode;
if (!(["check", "format", "lint"] as const).includes(mode)) {
  throw new Error(`unknown staged-check mode: ${mode}`);
}

function run(command: string, arguments_: string[], input?: Uint8Array) {
  const result = spawnSync(command, arguments_, {
    cwd: process.cwd(),
    input,
    stdio: input ? ["pipe", "inherit", "inherit"] : "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}

const merge = spawnSync("git", ["rev-parse", "-q", "--verify", "MERGE_HEAD"], {
  stdio: "ignore",
});
if (merge.status === 0) process.exit(0);

const inventory = spawnSync(
  "git",
  ["diff", "--cached", "--name-only", "--diff-filter=ACMR", "-z"],
  { encoding: "buffer" },
);
if (inventory.error) throw inventory.error;
if (inventory.status !== 0) process.exit(inventory.status ?? 1);

const paths = inventory.stdout.toString("utf8").split("\0").filter(Boolean);
const manifests = paths.filter((path) => path.endsWith("package.json"));
const prettierPaths = paths.filter((path) =>
  /(?:^|\/)(?:package\.json|[^/]+\.(?:css|html|json|jsonc|md|mdx|mjs|ts|tsx|yaml|yml))$/.test(
    path,
  ),
);
const isTestPath = (path: string) =>
  path.startsWith("test/") ||
  path.includes("/tests/") ||
  /\.(?:test|spec)\.[^/]+$/.test(path);
const lintPaths = paths.filter(
  (path) => /\.(?:js|jsx|mjs|cjs|ts|tsx)$/.test(path) && !isTestPath(path),
);

function format() {
  if (manifests.length > 0)
    run("bun", ["x", "sort-package-json", ...manifests]);
  if (prettierPaths.length > 0)
    run("bun", [
      "x",
      "prettier",
      "--write",
      "--ignore-unknown",
      ...prettierPaths,
    ]);
}

function lint() {
  if (lintPaths.length > 0)
    run("bun", [
      "x",
      "oxlint",
      "--disable-nested-config",
      "--deny-warnings",
      ...lintPaths,
    ]);
}

if (mode === "format" || mode === "check") format();
if (mode === "lint" || mode === "check") lint();
if (mode === "check") run("git", ["update-index", "--again"]);
