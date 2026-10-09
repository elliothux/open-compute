import type { HTTPResponse, Page } from "@cloudflare/puppeteer";
import { defined, type ActionOptions } from "./options.js";

const DEFAULT_AGENT =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/119.0.0.0 Safari/537.36";

/** Configure the page through the fixed upstream client before navigation. */
export async function preparePage(
  page: Page,
  options: ActionOptions,
): Promise<HTTPResponse | null> {
  page.setDefaultTimeout(30_000);
  await page.setViewport(
    defined(options.viewport ?? { width: 1920, height: 1080 }),
  );
  await page.setUserAgent(options.userAgent ?? DEFAULT_AGENT);
  if (options.authenticate) await page.authenticate(options.authenticate);
  if (options.cookies?.length)
    await page.setCookie(...defined(options.cookies));
  if (options.setExtraHTTPHeaders)
    await page.setExtraHTTPHeaders(options.setExtraHTTPHeaders);
  if (options.setJavaScriptEnabled !== undefined)
    await page.setJavaScriptEnabled(options.setJavaScriptEnabled);
  if (options.emulateMediaType !== undefined)
    await page.emulateMediaType(options.emulateMediaType);
  if (
    options.rejectRequestPattern ||
    options.allowRequestPattern ||
    options.rejectResourceTypes ||
    options.allowResourceTypes
  ) {
    // Tenant regular expressions run in the disposable browser context, never the host isolate.
    const matcher =
      options.rejectRequestPattern || options.allowRequestPattern
        ? await page.browserContext().newPage()
        : undefined;
    await page.setRequestInterception(true);
    page.on("request", (request) => {
      void (async () => {
        let deny =
          options.rejectResourceTypes?.includes(request.resourceType()) ||
          (options.allowResourceTypes &&
            !options.allowResourceTypes.includes(request.resourceType()));
        try {
          if (matcher && !deny) {
            deny = await matcher.evaluate(
              ({ url, rejected, allowed }) =>
                rejected?.some((pattern) => new RegExp(pattern).test(url)) ||
                Boolean(
                  allowed &&
                  !allowed.some((pattern) => new RegExp(pattern).test(url)),
                ),
              {
                url: request.url(),
                rejected: options.rejectRequestPattern,
                allowed: options.allowRequestPattern,
              },
            );
          }
        } catch {
          deny = true;
        }
        await (deny ? request.abort() : request.continue()).catch(
          () => undefined,
        );
      })();
    });
  }
  let navigation: HTTPResponse | null = null;
  try {
    const goto = {
      ...options.gotoOptions,
      timeout: options.gotoOptions?.timeout ?? 30_000,
      waitUntil:
        options.gotoOptions?.waitUntil ?? ("domcontentloaded" as const),
    };
    if (options.url) navigation = await page.goto(options.url, defined(goto));
    else await page.setContent(options.html ?? "", defined(goto));
  } catch (error: unknown) {
    if (!options.bestAttempt) throw error;
  }
  return navigation;
}

/** Post-navigation waits and page mutations belong to the action deadline. */
export async function completePage(
  page: Page,
  options: ActionOptions,
): Promise<void> {
  try {
    if (options.waitForSelector) {
      const { selector, ...wait } = options.waitForSelector;
      await page.waitForSelector(selector, defined(wait));
    }
    if (options.waitForTimeout)
      await new Promise<void>((resolve) =>
        setTimeout(resolve, options.waitForTimeout),
      );
  } catch (error: unknown) {
    if (!options.bestAttempt) throw error;
  }
  for (const script of options.addScriptTag ?? [])
    await page.addScriptTag(defined(script));
  for (const style of options.addStyleTag ?? [])
    await page.addStyleTag(defined(style));
}

function headers(response: HTTPResponse): Record<string, string> {
  const blocked = new Set([
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "connection",
    "keep-alive",
    "transfer-encoding",
    "upgrade",
    "trailer",
    "te",
  ]);
  return Object.fromEntries(
    Object.entries(response.headers()).filter(
      ([name]) => !blocked.has(name.toLowerCase()),
    ),
  );
}

export async function metadata(page: Page, navigation: HTTPResponse | null) {
  const meta: {
    status: number;
    title: string;
    headers?: Record<string, string>;
    finalUrl?: string;
    redirectChain?: {
      url: string;
      status: number;
      headers: Record<string, string>;
    }[];
  } = { status: navigation?.status() ?? 200, title: await page.title() };
  if (navigation) {
    meta.headers = headers(navigation);
    meta.finalUrl = navigation.url();
    const chain = navigation.request().redirectChain();
    if (chain.length)
      meta.redirectChain = chain.flatMap((request) => {
        const response = request.response();
        return response
          ? [
              {
                url: response.url(),
                status: response.status(),
                headers: headers(response),
              },
            ]
          : [];
      });
  }
  return meta;
}
