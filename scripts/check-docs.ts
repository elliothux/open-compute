import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const websiteRoot = join(repositoryRoot, "apps/website");
const docsRoot = join(websiteRoot, "src/content/docs/docs");

function filesBelow(directory: string): string[] {
  return readdirSync(directory)
    .flatMap((name) => {
      const path = join(directory, name);
      return statSync(path).isDirectory() ? filesBelow(path) : [path];
    })
    .sort();
}

const markdownFiles = filesBelow(docsRoot).filter((path) =>
  /\.mdx?$/.test(path),
);
const relativeFiles = markdownFiles.map((path) => relative(docsRoot, path));
const sourceFiles = new Set(relativeFiles);
const englishFiles = relativeFiles.filter((path) => !path.startsWith("zh/"));
const chineseFiles = new Set(
  relativeFiles
    .filter((path) => path.startsWith("zh/"))
    .map((path) => path.slice("zh/".length)),
);

const failures: string[] = [];
const fail = (message: string) => failures.push(message);

for (const path of englishFiles) {
  if (!chineseFiles.has(path)) fail(`missing Chinese page for ${path}`);
}
for (const path of chineseFiles) {
  if (!englishFiles.includes(path)) fail(`missing English page for zh/${path}`);
}

const requiredPages = [
  "index.md",
  "get-started.mdx",
  "develop/index.mdx",
  "operate/index.md",
  "cli/index.md",
  "products/index.md",
  "reference/index.md",
  "project/index.md",
];
for (const path of requiredPages) {
  if (!sourceFiles.has(path)) fail(`missing required page ${path}`);
  if (!sourceFiles.has(`zh/${path}`)) fail(`missing required page zh/${path}`);
}

function sourceForDocsUrl(url: string): string | undefined {
  let pathname: string;
  try {
    pathname = url.startsWith("http")
      ? new URL(url).pathname
      : new URL(url, "https://open-compute.dev").pathname;
  } catch {
    return undefined;
  }
  if (!pathname.startsWith("/docs/")) return undefined;

  const route = pathname.slice("/docs/".length).replace(/^\/+|\/+$/g, "");
  if (route === "") return "index.md";
  if (route === "zh") return "zh/index.md";
  for (const source of [
    `${route}.md`,
    `${route}.mdx`,
    `${route}/index.md`,
    `${route}/index.mdx`,
  ]) {
    if (sourceFiles.has(source)) return source;
  }
  return `${route}.md`;
}

const readmeFiles = [
  join(repositoryRoot, "README.md"),
  join(repositoryRoot, "README.zh.md"),
  join(websiteRoot, "README.md"),
];
const llmsFile = join(websiteRoot, "public/llms.txt");
const markdownLinkFiles = [...readmeFiles, ...markdownFiles];
const linkPattern =
  /\]\((\/docs\/[^)#?\s]*|https:\/\/open-compute\.dev\/docs\/[^)#?\s]*)/g;
for (const path of markdownLinkFiles) {
  const content = readFileSync(path, "utf8");
  for (const match of content.matchAll(linkPattern)) {
    const url = match[1];
    if (!url) continue;
    const source = sourceForDocsUrl(url);
    if (source && !sourceFiles.has(source)) {
      fail(`${relative(docsRoot, path)} links to missing ${url}`);
    }
  }
}

const siteLinkFiles = [
  join(websiteRoot, "src/components/hero-section/index.tsx"),
  join(websiteRoot, "src/components/site-footer/index.tsx"),
  join(websiteRoot, "src/components/site-header/index.tsx"),
  join(websiteRoot, "src/pages/404.astro"),
];
for (const path of siteLinkFiles) {
  const content = readFileSync(path, "utf8");
  for (const match of content.matchAll(/href="(\/docs\/[^"#?]*)"/g)) {
    const url = match[1];
    if (!url) continue;
    const source = sourceForDocsUrl(url);
    if (source && !sourceFiles.has(source)) {
      fail(`${relative(repositoryRoot, path)} links to missing ${url}`);
    }
  }
}

const navigation = readFileSync(
  join(websiteRoot, "src/docs-topics.ts"),
  "utf8",
);
for (const match of navigation.matchAll(/route:\s*"([^"]*)"/g)) {
  const route = match[1] ?? "";
  for (const prefix of ["/docs", "/docs/zh"]) {
    const source = sourceForDocsUrl(`${prefix}${route}/`);
    if (source && !sourceFiles.has(source))
      fail(`navigation links to missing ${prefix}${route}/`);
  }
}

const publicTextFiles = [...readmeFiles, ...markdownFiles, llmsFile];
const stalePatterns: [RegExp, string][] = [
  [/llms-(?:small|full)\.txt/, "retired generated LLM document"],
  [/\bbun run oc\b/, "bun run oc"],
  [/\boc (?:build|types|deploy|run)\b/, "retired oc command"],
  [/target\/debug\/ocd/, "source-build quickstart"],
  [/\btarget use\b/, "removed target use command"],
  [/\btarget add\s+\S+\s+--instance\b/, "removed target --instance form"],
  [/curl -fsSL -o install\.sh/, "multi-step public installer command"],
];
for (const path of publicTextFiles) {
  const content = readFileSync(path, "utf8");
  for (const [pattern, label] of stalePatterns) {
    if (pattern.test(content))
      fail(`${relative(repositoryRoot, path)} contains ${label}`);
  }
}

for (const path of [...readmeFiles.slice(0, 2), ...markdownFiles]) {
  const content = readFileSync(path, "utf8");
  if (/\bbun\s+(?:add|install|init|run)\b/.test(content)) {
    if (!/\bnpm\b/.test(content) || !/\bpnpm\b/.test(content)) {
      fail(
        `${relative(repositoryRoot, path)} has Bun user commands without npm and pnpm alternatives`,
      );
    }
  }
}

const installCommand =
  "curl -fsSL https://open-compute.dev/install.sh | sudo sh";
for (const path of [
  ...readmeFiles.slice(0, 2),
  join(docsRoot, "get-started.mdx"),
  join(docsRoot, "zh/get-started.mdx"),
  llmsFile,
]) {
  const content = readFileSync(path, "utf8");
  if (!content.includes(installCommand)) {
    fail(`${relative(repositoryRoot, path)} does not use the public installer`);
  }
}

const installerEndpoint = readFileSync(
  join(websiteRoot, "src/pages/install.sh.ts"),
  "utf8",
);
if (!installerEndpoint.includes('scripts/install.sh?raw"')) {
  fail("website installer endpoint does not serve scripts/install.sh");
}

const rootPackage = JSON.parse(
  readFileSync(join(repositoryRoot, "package.json"), "utf8"),
) as { catalog?: { wrangler?: string } };
const wranglerVersion = rootPackage.catalog?.wrangler;
if (!wranglerVersion) {
  fail("package.json does not declare catalog.wrangler");
} else {
  for (const path of [
    join(docsRoot, "get-started.mdx"),
    join(docsRoot, "zh/get-started.mdx"),
  ]) {
    if (!readFileSync(path, "utf8").includes(`wrangler@${wranglerVersion}`)) {
      fail(
        `${relative(repositoryRoot, path)} does not use Wrangler ${wranglerVersion}`,
      );
    }
  }
}

if (failures.length > 0) {
  for (const failure of failures) console.error(`docs check: ${failure}`);
  process.exit(1);
}

console.log(
  `docs check: ${englishFiles.length} English and ${chineseFiles.size} Chinese pages`,
);
