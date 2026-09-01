import assert from "node:assert/strict";
import { opendir, readFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { chromium, type Browser, type Page } from "@playwright/test";

const CASES = [
  "vinext/app/ssr-status-headers",
  "vinext/app/streaming-ssr",
  "vinext/app/hydration-navigation",
  "vinext/app/rsc-flight",
  "vinext/app/server-actions",
  "vinext/app/routes-middleware",
  "vinext/app/metadata-assets",
  "vinext/pages/gssp",
  "vinext/pages/gsp-paths",
  "vinext/static/export-routing",
  "vinext/static/deployment-coherence",
  "vinext/bindings/env-context",
  "vinext/lifecycle/cold-warm",
  "vinext/security/server-only",
  "vinext/security/browser-context-isolation",
] as const;

type CaseId = typeof CASES[number];

interface Target {
  readonly base: URL;
  readonly browserBase: URL;
  readonly requestHost?: string;
  readonly browserHostMap?: string;
}

interface CaseResult {
  readonly id: CaseId;
  readonly status: "passed" | "failed";
  readonly durationMs: number;
  readonly error?: string;
}

function target(): Target {
  const value = process.env.P4_BASE_URL;
  if (value === undefined) throw new Error("P4_BASE_URL is required");
  const base = new URL(value);
  const browserBase = new URL(process.env.P4_BROWSER_URL ?? value);
  for (const url of [base, browserBase]) {
    if (!url.pathname.endsWith("/") || url.search || url.hash || url.username || url.password
        || (url.protocol !== "https:" && url.protocol !== "http:")) {
      throw new Error("P4 target must be an HTTP(S) base URL ending in /");
    }
  }
  const requestHost = process.env.P4_REQUEST_HOST;
  if (requestHost !== undefined && !/^[a-z0-9.-]+(?::[0-9]{1,5})?$/.test(requestHost)) {
    throw new Error("P4_REQUEST_HOST is invalid");
  }
  const browserHostMap = process.env.P4_BROWSER_HOST_MAP;
  if (browserHostMap !== undefined && !/^[a-z0-9.-]+=[0-9a-f:.]+$/.test(browserHostMap)) {
    throw new Error("P4_BROWSER_HOST_MAP is invalid");
  }
  return { base, browserBase, ...(requestHost === undefined ? {} : { requestHost }),
    ...(browserHostMap === undefined ? {} : { browserHostMap }) };
}

function url(base: URL, path: string): URL {
  if (!path.startsWith("/")) throw new Error("qualification path must be absolute");
  const result = new URL(path.slice(1), base);
  if (result.origin !== base.origin) throw new Error("qualification path escaped the target");
  return result;
}

async function request(targetValue: Target, path: string): Promise<Response> {
  return fetch(url(targetValue.base, path), {
    headers: targetValue.requestHost === undefined ? {} : { host: targetValue.requestHost },
    redirect: "manual",
    signal: AbortSignal.timeout(30_000),
  });
}

async function text(targetValue: Target, path: string, status: number): Promise<{ response: Response; body: string }> {
  const response = await request(targetValue, path);
  const body = await response.text();
  assert.equal(response.status, status, `${path}: ${body.slice(0, 200)}`);
  return { response, body };
}

async function launch(targetValue: Target): Promise<Browser> {
  const args = ["--no-proxy-server"];
  if (targetValue.browserHostMap !== undefined) {
    const [hostname, address] = targetValue.browserHostMap.split("=");
    args.push(`--host-resolver-rules=MAP ${hostname} ${address}`);
  }
  return chromium.launch({ headless: true, args });
}

function browserErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on("console", message => { if (message.type() === "error") errors.push(message.text()); });
  page.on("pageerror", error => errors.push(error.message));
  return errors;
}

const implementations: Record<CaseId, (targetValue: Target) => Promise<void>> = {
  "vinext/app/ssr-status-headers": async targetValue => {
    const { response, body } = await text(targetValue, "/", 200);
    assert.match(response.headers.get("content-type") ?? "", /^text\/html/);
    assert.equal(response.headers.get("x-p4-proxy"), "/");
    assert.match(body, /app-router:ssr/);
  },
  "vinext/app/streaming-ssr": async targetValue => {
    const response = await request(targetValue, "/stream");
    assert.equal(response.status, 200);
    assert(response.body !== null);
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let body = "";
    for (;;) {
      const part = await reader.read();
      if (part.done) break;
      body += decoder.decode(part.value, { stream: true });
    }
    body += decoder.decode();
    assert(body.indexOf("stream:fallback") >= 0);
    assert(body.indexOf("stream:resolved") > body.indexOf("stream:fallback"));
  },
  "vinext/app/hydration-navigation": async targetValue => {
    const browser = await launch(targetValue);
    try {
      const page = await browser.newPage();
      const errors = browserErrors(page);
      const response = await page.goto(targetValue.browserBase.href, { waitUntil: "networkidle" });
      assert.equal(response?.status(), 200);
      assert.equal(await page.getByTestId("counter").textContent(), "count:0");
      await page.getByTestId("counter").click();
      assert.equal(await page.getByTestId("counter").textContent(), "count:1");
      assert.deepEqual(errors, []);
    } finally { await browser.close(); }
  },
  "vinext/app/rsc-flight": async targetValue => {
    const browser = await launch(targetValue);
    try {
      const page = await browser.newPage();
      await page.goto(targetValue.browserBase.href, { waitUntil: "networkidle" });
      const flight = page.waitForResponse(response => {
        const headers = response.request().headers();
        return headers.rsc === "1" && new URL(response.url()).pathname === "/navigation";
      });
      await page.getByRole("link", { name: "navigation" }).click();
      await page.getByTestId("navigation-marker").waitFor();
      const response = await flight;
      assert.equal(response.status(), 200);
      assert.match(await response.text(), /client-navigation:ready/);
    } finally { await browser.close(); }
  },
  "vinext/app/server-actions": async targetValue => {
    const browser = await launch(targetValue);
    try {
      const page = await browser.newPage();
      await page.goto(targetValue.browserBase.href, { waitUntil: "networkidle" });
      const action = page.waitForRequest(requestValue =>
        requestValue.method() === "POST" && requestValue.headers()["next-action"] !== undefined);
      await page.getByTestId("server-action").click();
      await action;
      await page.getByTestId("action-result").waitFor();
      assert.equal(await page.getByTestId("action-result").textContent(), "action:qualified");
      assert.equal(await page.getByTestId("action-cookie").textContent(), "cookie:qualified");
    } finally { await browser.close(); }
  },
  "vinext/app/routes-middleware": async targetValue => {
    const { response, body } = await text(targetValue, "/api/status?code=201", 201);
    assert.equal(response.headers.get("x-p4-router"), "app");
    assert.equal(response.headers.get("x-p4-proxy"), "/api/status");
    assert.deepEqual(JSON.parse(body), { router: "app", status: 201 });
    const invalid = await text(targetValue, "/api/status?code=500", 400);
    assert.deepEqual(JSON.parse(invalid.body), { router: "app", status: 400 });
  },
  "vinext/app/metadata-assets": async targetValue => {
    const root = await text(targetValue, "/", 200);
    assert.match(root.body, /<title>open-compute P4 vinext qualification<\/title>/);
    assert.match(root.body, /Fixed Next\.js 16 production workload/);
    const source = root.body.match(/(?:src|href)="(\/_next\/static\/[^"]+\.js[^\"]*)"/)?.[1];
    assert(source !== undefined);
    const script = await request(targetValue, source.replaceAll("&amp;", "&"));
    assert.equal(script.status, 200);
    await script.body?.cancel();
  },
  "vinext/pages/gssp": async targetValue => {
    const { response, body } = await text(targetValue, "/pages-qualification", 200);
    assert.equal(response.headers.get("x-p4-proxy"), "/pages-qualification");
    assert.match(body, /pages-router:gssp/);
    const api = await text(targetValue, "/api/pages-status", 200);
    assert.equal(api.response.headers.get("x-p4-router"), "pages");
    assert.deepEqual(JSON.parse(api.body), { router: "pages", status: 200 });
  },
  "vinext/pages/gsp-paths": async targetValue => {
    assert.match((await text(targetValue, "/static-qualification/alpha", 200)).body, /gsp:(?:<!-- -->)?alpha/);
    await text(targetValue, "/static-qualification/missing", 404);
  },
  "vinext/static/export-routing": async targetValue => {
    const asset = await text(targetValue, "/qualification.txt", 200);
    assert.equal(asset.body, "open-compute-p4-vinext-static-asset\n");
    assert.match(asset.response.headers.get("content-type") ?? "", /^text\/plain/);
    await text(targetValue, "/missing", 404);
  },
  "vinext/static/deployment-coherence": async targetValue => {
    const root = await text(targetValue, "/", 200);
    const references = [...new Set(
      [...root.body.matchAll(/(?:src|href)="(\/_next\/static\/[^"]+\.js[^\"]*)"/g)]
        .map(match => match[1]?.replaceAll("&amp;", "&"))
        .filter((value): value is string => value !== undefined),
    )];
    assert(references.length >= 5);
    for (const reference of references) {
      const response = await request(targetValue, reference);
      assert.equal(response.status, 200, reference);
      await response.body?.cancel();
    }
  },
  "vinext/bindings/env-context": async targetValue => {
    const env = await text(targetValue, "/api/env", 200);
    assert.equal(env.response.headers.get("x-p4-env"), "cloudflare-workers");
    assert.deepEqual(JSON.parse(env.body), {
      marker: "p4-public-marker", serverOnlyCanaryExposed: false, serverOnlyCanaryLength: 23,
    });
  },
  "vinext/lifecycle/cold-warm": async targetValue => {
    const first = await text(targetValue, "/api/env", 200);
    const second = await text(targetValue, "/api/env", 200);
    assert.deepEqual(JSON.parse(first.body), JSON.parse(second.body));
  },
  "vinext/security/server-only": async targetValue => {
    const canary = "P4_SERVER_ONLY_7f4a2c9e";
    const publicBodies = [
      (await text(targetValue, "/", 200)).body,
      (await text(targetValue, "/api/env", 200)).body,
      (await text(targetValue, "/navigation", 200)).body,
    ];
    for (const body of publicBodies) assert(!body.includes(canary));
    const clientRoot = resolve("dist/client");
    const directories = [clientRoot];
    while (directories.length > 0) {
      const directory = directories.pop();
      assert(directory !== undefined);
      const listing = await opendir(directory);
      for await (const entry of listing) {
        const filename = join(directory, entry.name);
        if (entry.isDirectory()) directories.push(filename);
        else if (entry.isFile()) assert(!Buffer.from(await readFile(filename)).includes(Buffer.from(canary)));
      }
    }
  },
  "vinext/security/browser-context-isolation": async targetValue => {
    const browser = await launch(targetValue);
    try {
      const first = await browser.newContext();
      const second = await browser.newContext();
      const page = await first.newPage();
      await page.goto(targetValue.browserBase.href, { waitUntil: "networkidle" });
      await page.getByTestId("server-action").click();
      await page.getByTestId("action-result").waitFor();
      assert.equal((await first.cookies()).find(cookie => cookie.name === "p4-action")?.value, "qualified");
      assert.equal((await second.cookies()).find(cookie => cookie.name === "p4-action"), undefined);
      await first.close();
      await second.close();
    } finally { await browser.close(); }
  },
};

async function main(): Promise<void> {
  const args = process.argv.slice(2);
  if (args.length === 1 && args[0] === "--list") {
    process.stdout.write(`${JSON.stringify({ schemaVersion: 1, cases: CASES })}\n`);
    return;
  }
  const requested: CaseId[] = [];
  for (let index = 0; index < args.length; index += 2) {
    if (args[index] !== "--case" || args[index + 1] === undefined
        || !CASES.includes(args[index + 1] as CaseId)) throw new Error("use --case <known-id>");
    requested.push(args[index + 1] as CaseId);
  }
  if (new Set(requested).size !== requested.length) throw new Error("duplicate qualification case");
  const selected = requested.length === 0 ? [...CASES] : requested;
  const targetValue = target();
  const results: CaseResult[] = [];
  for (const id of selected) {
    const started = performance.now();
    try {
      await implementations[id](targetValue);
      results.push({ id, status: "passed", durationMs: Math.round(performance.now() - started) });
    } catch (error) {
      results.push({ id, status: "failed", durationMs: Math.round(performance.now() - started),
        error: (error instanceof Error ? error.message : "qualification failed").slice(0, 2048) });
      break;
    }
  }
  const passed = results.length === selected.length && results.every(result => result.status === "passed");
  process.stdout.write(`${JSON.stringify({ schemaVersion: 1, status: passed ? "passed" : "failed", results })}\n`);
  if (!passed) process.exitCode = 1;
}

await main();
