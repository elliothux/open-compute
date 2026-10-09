import puppeteer, {
  TimeoutError,
  type BrowserWorker,
  type Page,
} from "@cloudflare/puppeteer";
import { cached, cacheKey, remember } from "./cache.js";
import { actionRequest, defined, type ActionRequest } from "./options.js";
import { completePage, metadata, preparePage } from "./page.js";

function failure(message: string, status: number): Response {
  return Response.json({ success: false, errors: [{ message }] }, { status });
}

interface Env {
  BINDING_BACKEND: BrowserWorker;
  BINDING_BACKEND_TOKEN: string;
  INTERNAL_TOKEN: string;
}

function backend(env: Env, sessionId: string): BrowserWorker {
  return {
    fetch: (input, init) => {
      const request = new Request(input, init);
      const url = new URL(request.url);
      if (url.pathname !== `/v1/devtools/browser/${sessionId}`)
        throw new Error("BROWSER_UNSUPPORTED");
      const headers = new Headers();
      if (request.headers.get("upgrade")?.toLowerCase() === "websocket")
        headers.set("upgrade", "websocket");
      headers.set("x-open-compute-binding-token", env.BINDING_BACKEND_TOKEN);
      headers.set("x-open-compute-startup-generation", requestGeneration);
      return env.BINDING_BACKEND.fetch(
        new Request(
          `http://binding-backend/internal/browser-actions/v1/${sessionId}`,
          {
            method: request.method,
            headers,
            body: request.body,
            redirect: "manual",
          },
        ),
      );
    },
  };
}
// The system Worker has one immutable startup generation, set from trusted ingress only.
let requestGeneration = "";

async function accessibility(page: Page, request: ActionRequest) {
  const root = request.options.root
    ? await page.$(request.options.root)
    : undefined;
  if (request.options.root && !root) return { accessibilityTree: null };
  try {
    return {
      accessibilityTree: await page.accessibility.snapshot({
        interestingOnly:
          request.options.interestingOnly ?? !request.options.root,
        ...(root ? { root } : {}),
      }),
    };
  } finally {
    await root?.dispose();
  }
}

async function markdown(page: Page, env: Env, sessionId: string) {
  const response = await env.BINDING_BACKEND.fetch(
    `http://binding-backend/internal/browser-actions/v1/${sessionId}/markdown`,
    {
      method: "POST",
      headers: {
        "content-type": "text/html",
        "x-open-compute-binding-token": env.BINDING_BACKEND_TOKEN,
        "x-open-compute-startup-generation": requestGeneration,
      },
      body: await page.content(),
    },
  );
  if (!response.ok) throw new Error("BROWSER_UNAVAILABLE");
  const result: unknown = await response.json();
  if (typeof result !== "string") throw new Error("BROWSER_UNAVAILABLE");
  return result;
}

async function output(
  page: Page,
  request: ActionRequest,
  env: Env,
): Promise<Response> {
  const options = request.options;
  switch (request.action) {
    case "content":
      return Response.json(await page.content());
    case "screenshot": {
      if (options.scrollPage)
        await page.evaluate(async () => {
          for (
            let y = 0;
            y < document.body.scrollHeight && y < 1_000_000;
            y += window.innerHeight
          )
            window.scrollTo(0, y);
          window.scrollTo(0, 0);
        });
      const element = options.selector ? await page.$(options.selector) : null;
      if (options.selector && !element)
        throw new Error("BROWSER_INPUT_INVALID");
      try {
        const type = options.screenshotOptions?.type ?? "png";
        if (options.screenshotOptions?.encoding === "base64") {
          const image = await (element ?? page).screenshot({
            ...defined(options.screenshotOptions),
            encoding: "base64",
          });
          return new Response(`data:image/${type};base64,${image}`, {
            headers: { "content-type": "text/plain;charset=UTF-8" },
          });
        }
        const image = await (element ?? page).screenshot({
          ...defined(options.screenshotOptions),
          encoding: "binary",
        });
        return new Response(Uint8Array.from(image).buffer, {
          headers: { "content-type": `image/${type}` },
        });
      } finally {
        await element?.dispose();
      }
    }
    case "pdf":
      return new Response(
        Uint8Array.from(await page.pdf(defined(options.pdfOptions))).buffer,
        { headers: { "content-type": "application/pdf" } },
      );
    case "scrape": {
      const results = [];
      for (const { selector } of options.elements ?? [])
        results.push({
          selector,
          results: await page.$$eval(selector, (elements) =>
            elements.map((element) => {
              const rect = element.getBoundingClientRect();
              return {
                html: element.innerHTML,
                text:
                  element instanceof HTMLElement
                    ? element.innerText
                    : (element.textContent ?? ""),
                width: rect.width,
                height: rect.height,
                top: rect.top,
                left: rect.left,
                attributes: Array.from(element.attributes, (attribute) => ({
                  name: attribute.name,
                  value: attribute.value,
                })),
              };
            }),
          ),
        });
      return Response.json(results);
    }
    case "links":
      return Response.json(
        await page.$$eval(
          "a[href]",
          (elements, filter) => {
            const seen = new Set<string>();
            for (const element of elements) {
              const url = new URL(
                element.getAttribute("href") ?? "",
                document.baseURI,
              );
              if (filter.visible && !element.getClientRects().length) continue;
              if (filter.external && url.hostname !== location.hostname)
                continue;
              seen.add(url.href);
            }
            return [...seen];
          },
          {
            visible: options.visibleLinksOnly ?? false,
            external: options.excludeExternalLinks ?? false,
          },
        ),
      );
    case "accessibilityTree":
      return Response.json(await accessibility(page, request));
    case "json": {
      const response = await env.BINDING_BACKEND.fetch(
        `http://binding-backend/internal/browser-actions/v1/${request.sessionId}/json`,
        {
          method: "POST",
          headers: {
            "content-type": "application/json",
            "x-open-compute-binding-token": env.BINDING_BACKEND_TOKEN,
            "x-open-compute-startup-generation": requestGeneration,
          },
          body: JSON.stringify({
            html: await page.content(),
            prompt: options.prompt,
            response_format: options.response_format,
            custom_ai: options.custom_ai,
          }),
        },
      );
      if (!response.ok) return response;
      return Response.json(await response.json());
    }
    case "markdown":
      return Response.json(await markdown(page, env, request.sessionId));
    case "snapshot": {
      const formats = options.formats ?? ["content", "screenshot"];
      const result: {
        content?: string;
        screenshot?: string;
        markdown?: string;
        accessibilityTree?: unknown;
      } = {};
      for (const format of formats) {
        if (format === "content") result.content = await page.content();
        if (format === "screenshot")
          result.screenshot = await page.screenshot({
            ...defined(options.screenshotOptions),
            encoding: "base64",
          });
        if (format === "markdown") {
          const content = await markdown(page, env, request.sessionId);
          const title = await page.title();
          result.markdown = title
            ? `---\ntitle: ${JSON.stringify(title)}\n---\n\n${content}`
            : content;
        }
        if (format === "accessibilityTree")
          result.accessibilityTree = (
            await accessibility(page, request)
          ).accessibilityTree;
      }
      return Response.json(result);
    }
  }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    if (
      request.method !== "POST" ||
      request.headers.get("x-open-compute-internal-token") !==
        env.INTERNAL_TOKEN
    )
      return new Response(null, { status: 404 });
    const started = performance.now();
    let payload: unknown;
    try {
      payload = await request.json();
    } catch {
      return failure("BROWSER_INPUT_INVALID", 400);
    }
    const parsed = actionRequest.safeParse(payload);
    const generation = request.headers.get("x-open-compute-startup-generation");
    if (
      !parsed.success ||
      generation !== env.INTERNAL_TOKEN ||
      (requestGeneration && requestGeneration !== generation)
    )
      return failure("BROWSER_INPUT_INVALID", 400);
    requestGeneration = generation;
    const action = parsed.data;
    if (action.validateOnly) return new Response(null, { status: 204 });
    const ttl = action.options.cacheTTL ?? 5;
    const key = await cacheKey(action.action, action.options);
    const hit = ttl ? cached(key, action.maxResultBytes) : undefined;
    if (hit) return hit;
    let browser;
    try {
      browser = await puppeteer.connect(
        backend(env, action.sessionId),
        action.sessionId,
      );
      const context = await browser.createBrowserContext();
      // Native dispose-on-detach owns cleanup, including a renderer stuck in tenant code.
      const page = await context.newPage();
      const navigation = await preparePage(page, action.options);
      let timer: ReturnType<typeof setTimeout> | undefined;
      const duration = action.options.actionTimeout ?? 30_000;
      const run = async () => {
        await completePage(page, action.options);
        return output(page, action, env);
      };
      let response: Response;
      try {
        response =
          duration === 0
            ? await run()
            : await Promise.race([
                run(),
                new Promise<Response>((_, reject) => {
                  timer = setTimeout(
                    () => reject(new TimeoutError("BROWSER_TIMEOUT")),
                    duration,
                  );
                }),
              ]);
      } finally {
        clearTimeout(timer);
      }
      if (!response.ok) return response;
      const binary = ["screenshot", "pdf"].includes(action.action);
      let final = response;
      if (!binary)
        final = Response.json({
          success: true,
          result: await response.json(),
          meta: await metadata(page, navigation),
        });
      const bytes = await final.arrayBuffer();
      if (bytes.byteLength > action.maxResultBytes)
        return failure("BROWSER_LIMIT_EXCEEDED", 413);
      const headers = new Headers(final.headers);
      headers.set(
        "x-browser-ms-used",
        String(Math.ceil(performance.now() - started)),
      );
      remember(key, bytes, headers, ttl, action.maxResultBytes);
      return new Response(bytes, { status: final.status, headers });
    } catch (error: unknown) {
      if (error instanceof TimeoutError) return failure("BROWSER_TIMEOUT", 504);
      return failure("BROWSER_UNAVAILABLE", 503);
    } finally {
      browser?.disconnect();
    }
  },
};
