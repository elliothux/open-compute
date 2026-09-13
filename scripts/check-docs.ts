import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const websiteRoot = join(repositoryRoot, "apps/website");
const docsContentRoot = join(websiteRoot, "src/content/docs");
const englishDocsRoot = join(docsContentRoot, "docs");
const chineseDocsRoot = join(docsContentRoot, "zh/docs");

function filesBelow(directory: string): string[] {
  return readdirSync(directory)
    .flatMap((name) => {
      const path = join(directory, name);
      return statSync(path).isDirectory() ? filesBelow(path) : [path];
    })
    .sort();
}

const englishFiles = filesBelow(englishDocsRoot)
  .filter((path) => /\.mdx?$/.test(path))
  .map((path) => relative(englishDocsRoot, path));
const chineseFiles = new Set(
  filesBelow(chineseDocsRoot)
    .filter((path) => /\.mdx?$/.test(path))
    .map((path) => relative(chineseDocsRoot, path)),
);
const markdownFiles = [
  ...englishFiles.map((path) => join(englishDocsRoot, path)),
  ...[...chineseFiles].map((path) => join(chineseDocsRoot, path)),
];

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
  if (!englishFiles.includes(path)) fail(`missing required page ${path}`);
  if (!chineseFiles.has(path)) fail(`missing required page zh/${path}`);
}

interface DocsSource {
  language: "en" | "zh";
  relativePath: string;
}

function sourceForDocsUrl(url: string): DocsSource | undefined {
  let pathname: string;
  try {
    pathname = url.startsWith("http")
      ? new URL(url).pathname
      : new URL(url, "https://open-compute.dev").pathname;
  } catch {
    return undefined;
  }
  const language = pathname.startsWith("/zh/docs/") ? "zh" : "en";
  const prefix = language === "zh" ? "/zh/docs/" : "/docs/";
  if (!pathname.startsWith(prefix)) return undefined;

  const route = pathname.slice(prefix.length).replace(/^\/+|\/+$/g, "");
  const available = language === "zh" ? chineseFiles : new Set(englishFiles);
  if (route === "") return { language, relativePath: "index.md" };
  for (const source of [
    `${route}.md`,
    `${route}.mdx`,
    `${route}/index.md`,
    `${route}/index.mdx`,
  ]) {
    if (available.has(source)) return { language, relativePath: source };
  }
  return { language, relativePath: `${route}.md` };
}

function sourceExists(source: DocsSource): boolean {
  return source.language === "zh"
    ? chineseFiles.has(source.relativePath)
    : englishFiles.includes(source.relativePath);
}

const readmeFiles = [
  join(repositoryRoot, "README.md"),
  join(repositoryRoot, "README.zh.md"),
  join(websiteRoot, "README.md"),
];
const llmsFile = join(websiteRoot, "public/llms.txt");
const markdownLinkFiles = [...readmeFiles, ...markdownFiles];
const linkPattern =
  /\]\((\/(?:zh\/)?docs\/[^)#?\s]*|https:\/\/open-compute\.dev\/(?:zh\/)?docs\/[^)#?\s]*)/g;
for (const path of markdownLinkFiles) {
  const content = readFileSync(path, "utf8");
  for (const match of content.matchAll(linkPattern)) {
    const url = match[1];
    if (!url) continue;
    const source = sourceForDocsUrl(url);
    if (source && !sourceExists(source)) {
      fail(`${relative(docsContentRoot, path)} links to missing ${url}`);
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
  for (const match of content.matchAll(/href="(\/(?:zh\/)?docs\/[^"#?]*)"/g)) {
    const url = match[1];
    if (!url) continue;
    const source = sourceForDocsUrl(url);
    if (source && !sourceExists(source)) {
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
  for (const prefix of ["/docs", "/zh/docs"]) {
    const source = sourceForDocsUrl(`${prefix}${route}/`);
    if (source && !sourceExists(source))
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
  join(englishDocsRoot, "get-started.mdx"),
  join(chineseDocsRoot, "get-started.mdx"),
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
    join(englishDocsRoot, "get-started.mdx"),
    join(chineseDocsRoot, "get-started.mdx"),
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
