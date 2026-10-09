import {
  launch as launchPlaywright,
  sessions as playwrightSessions,
} from "@cloudflare/playwright";
import puppeteer, { type BrowserWorker } from "@cloudflare/puppeteer";
import type {
  BrowserRun,
  Response as WorkerResponse,
} from "@cloudflare/workers-types";
import { downloads } from "./downloads";

interface Env {
  BROWSER: BrowserWorker & Pick<BrowserRun, "quickAction">;
}

export default {
  async fetch(request: Request, env: Env): Promise<Response | WorkerResponse> {
    const url = new URL(request.url);
    if (url.pathname === "/playwright") {
      const browser = await launchPlaywright(env.BROWSER);
      try {
        const page = await browser.newPage();
        await page.setContent(
          "<html><title>Playwright</title><body>rendered</body></html>",
        );
        const sessionMetadata = {
          puppeteer: await puppeteer.sessions(env.BROWSER),
          playwright: await playwrightSessions(env.BROWSER),
        };
        const downloaded = [];
        for (const accept of [undefined, true, false])
          downloaded.push(await downloads(browser, accept));
        return Response.json({
          title: await page.title(),
          sessionMetadata,
          downloads: downloaded,
        });
      } finally {
        await browser.close();
      }
    }
    if (url.pathname === "/history") {
      return Response.json(await puppeteer.history(env.BROWSER));
    }
    if (url.pathname === "/sessions") {
      return Response.json(await puppeteer.sessions(env.BROWSER));
    }
    if (url.pathname === "/content") {
      return env.BROWSER.quickAction("content", { html: "<p>rendered</p>" });
    }
    const html =
      '<html><title>Actions</title><body><h1>rendered</h1><a href="https://example.com/linked">link</a><button>Submit</button></body></html>';
    switch (url.pathname) {
      case "/json":
        return env.BROWSER.quickAction("json", {
          html,
          prompt: "Extract the heading",
          cacheTTL: 0,
        });
      case "/json-object":
        return env.BROWSER.quickAction("json", {
          html,
          response_format: { type: "json_object" },
          cacheTTL: 0,
        });
      case "/json-custom":
      case "/json-custom-fallback":
      case "/json-custom-denied":
        return env.BROWSER.quickAction("json", {
          html,
          prompt: "Extract the heading",
          custom_ai: [
            {
              model: "openai/gpt-4o",
              authorization:
                url.pathname === "/json-custom"
                  ? "Bearer custom-request-key"
                  : "Bearer rejected-request-key",
            },
            ...(url.pathname === "/json-custom-fallback"
              ? [
                  {
                    model: "anthropic/claude-sonnet-4-20250514",
                    authorization: "fallback-request-key",
                  },
                ]
              : []),
          ],
          cacheTTL: 0,
        });
      case "/json-custom-workers-ai":
        return env.BROWSER.quickAction("json", {
          html,
          prompt: "Extract the heading",
          custom_ai: [
            { model: "workers-ai/@cf/meta/llama-3.3-70b-instruct-fp8-fast" },
          ],
          cacheTTL: 0,
        });
      case "/json-schema":
      case "/json-schema-mismatch":
        return env.BROWSER.quickAction("json", {
          html,
          response_format: {
            type: "json_schema",
            json_schema: {
              $defs: { heading: { type: "string" } },
              type: "object",
              properties: { heading: { $ref: "#/$defs/heading" } },
              required: [
                url.pathname === "/json-schema" ? "heading" : "missing",
              ],
              additionalProperties: false,
            },
          },
          cacheTTL: 0,
        });
      case "/json-malformed":
        return env.BROWSER.quickAction("json", {
          html,
          prompt: "malformed-model",
          cacheTTL: 0,
        });
      case "/json-rate":
        return env.BROWSER.quickAction("json", {
          html,
          prompt: "rate-limit-fixture",
          cacheTTL: 0,
        });
      case "/json-limit":
        return env.BROWSER.quickAction("json", {
          html,
          prompt: "x".repeat(10_000),
          cacheTTL: 0,
        });
      case "/filter":
        return env.BROWSER.quickAction("content", {
          html: '<title>Filtered</title><script src="http://127.0.0.1:9/script.js"></script>',
          rejectRequestPattern: ["^http://127\\.0\\.0\\.1:9/"],
          cacheTTL: 0,
        });
      case "/pattern-timeout":
        return env.BROWSER.quickAction("content", {
          html: `<script src="http://127.0.0.1:9/${"a".repeat(1000)}b"></script>`,
          rejectRequestPattern: ["(a+)+$"],
          actionTimeout: 500,
          gotoOptions: { timeout: 500 },
          cacheTTL: 0,
        });
      case "/navigation-before-action":
        return env.BROWSER.quickAction("content", {
          html: "<script>const end=performance.now()+750;while(performance.now()<end){}</script><p>Loaded before action</p>",
          actionTimeout: 500,
          cacheTTL: 0,
        });
      case "/snapshot-markdown":
        return env.BROWSER.quickAction("snapshot", {
          html,
          formats: ["content", "markdown"],
          cacheTTL: 0,
        });
      case "/markdown":
        return env.BROWSER.quickAction("markdown", { html, cacheTTL: 0 });
      case "/screenshot":
        return env.BROWSER.quickAction("screenshot", {
          html,
          viewport: { width: 320, height: 240 },
          cacheTTL: 0,
        });
      case "/pdf":
        return env.BROWSER.quickAction("pdf", { html, cacheTTL: 0 });
      case "/scrape":
        return env.BROWSER.quickAction("scrape", {
          html,
          elements: [{ selector: "h1" }],
          cacheTTL: 0,
        });
      case "/links":
        return env.BROWSER.quickAction("links", { html, cacheTTL: 0 });
      case "/snapshot":
        return env.BROWSER.quickAction("snapshot", {
          html,
          formats: ["content", "screenshot", "accessibilityTree"],
          cacheTTL: 0,
        });
      case "/accessibilityTree":
        return env.BROWSER.quickAction("accessibilityTree", {
          html,
          cacheTTL: 0,
        });
      case "/invalid":
        return env.BROWSER.quickAction("content", {
          html,
          url: "https://example.com/",
        });
      case "/timeout":
        return env.BROWSER.quickAction("content", {
          html,
          waitForTimeout: 10_000,
          actionTimeout: 500,
          cacheTTL: 0,
        });
    }
    const browser = await puppeteer.launch(env.BROWSER);
    const sessionId = browser.sessionId();
    browser.disconnect();
    const connected = await puppeteer.connect(env.BROWSER, sessionId);
    try {
      const page = await connected.newPage();
      await page.setContent(
        "<html><title>Puppeteer</title><body>rendered</body></html>",
      );
      const frameSource = url.searchParams.get("frame");
      if (frameSource) {
        await page.goto(frameSource);
        const frame = await page.waitForFrame(
          (frame) =>
            frame.parentFrame() !== null && frame.url().endsWith("/frame"),
        );
        const element = await frame.frameElement();
        try {
          return Response.json({
            text: await frame.evaluate(
              () => document.querySelector("h1")?.textContent,
            ),
            element: await element?.evaluate((node) => node.tagName),
          });
        } finally {
          await element?.dispose();
          await page.close();
        }
      }
      const context = await connected.createBrowserContext();
      const isolated = await context.newPage();
      await isolated.setContent("<title>Isolated</title>");
      const result = {
        sessionId,
        title: await page.title(),
        isolated: await isolated.title(),
      };
      await context.close();
      return Response.json(result);
    } finally {
      if (url.searchParams.get("disconnect") === "true") connected.disconnect();
      else await connected.close();
    }
  },
};
