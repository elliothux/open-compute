import wrappedBinding from "cloudflare-internal:wrapped-binding";

const actions = new Set([
  "screenshot",
  "pdf",
  "content",
  "scrape",
  "links",
  "snapshot",
  "markdown",
  "json",
  "accessibilityTree",
]);
/** Native wrapped Fetcher with the fixed BrowserRun quick-action surface. */
export class BrowserRun extends wrappedBinding.WrappedBinding {
  readonly #fetcher: Pick<Fetcher, "fetch">;
  constructor(raw: unknown) {
    super(raw);
    if (
      raw === null ||
      typeof raw !== "object" ||
      !("fetch" in raw) ||
      typeof raw.fetch !== "function"
    )
      throw new TypeError("BROWSER_UNAVAILABLE");
    this.#fetcher = raw as Pick<Fetcher, "fetch">;
  }
  fetch(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
    return this.#fetcher.fetch(input, init);
  }
  quickAction(action: string, options: unknown): Promise<Response> {
    if (!actions.has(action)) throw new TypeError("BROWSER_UNSUPPORTED");
    return this.#fetcher.fetch(`https://browser/v1/${action}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(options),
    });
  }
}
export default function browserBinding(env: { fetcher: unknown }): BrowserRun {
  return new BrowserRun(env.fetcher);
}
