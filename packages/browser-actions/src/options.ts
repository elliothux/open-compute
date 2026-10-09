import { z } from "zod";

const text = z.string().max(1_048_576);
const pattern = text.refine((value) => {
  try {
    new RegExp(value);
    return true;
  } catch {
    return false;
  }
}, "Invalid regular expression");
const timeout = z.number().int().min(0).max(120_000);
const lifecycle = z.enum([
  "load",
  "domcontentloaded",
  "networkidle0",
  "networkidle2",
]);
const resource = z.enum([
  "document",
  "stylesheet",
  "image",
  "media",
  "font",
  "script",
  "texttrack",
  "xhr",
  "fetch",
  "prefetch",
  "eventsource",
  "websocket",
  "manifest",
  "signedexchange",
  "ping",
  "cspviolationreport",
  "preflight",
  "other",
]);
const cookie = z.strictObject({
  name: text,
  value: text,
  url: text.optional(),
  domain: text.optional(),
  path: text.optional(),
  secure: z.boolean().optional(),
  httpOnly: z.boolean().optional(),
  sameSite: z.enum(["Strict", "Lax", "None"]).optional(),
  expires: z.number().finite().optional(),
  priority: z.enum(["Low", "Medium", "High"]).optional(),
  sameParty: z.boolean().optional(),
  sourceScheme: z.enum(["Unset", "NonSecure", "Secure"]).optional(),
  sourcePort: z.number().int().min(-1).max(65535).optional(),
  partitionKey: text.optional(),
});
const common = {
  url: z.url().max(4096).optional(),
  html: text.optional(),
  addScriptTag: z
    .array(
      z.strictObject({
        content: text.optional(),
        url: text.optional(),
        type: text.optional(),
        id: text.optional(),
      }),
    )
    .max(100)
    .optional(),
  addStyleTag: z
    .array(z.strictObject({ content: text.optional(), url: text.optional() }))
    .max(100)
    .optional(),
  authenticate: z.strictObject({ username: text, password: text }).optional(),
  cookies: z.array(cookie).max(1000).optional(),
  emulateMediaType: text.optional(),
  gotoOptions: z
    .strictObject({
      timeout: timeout.max(60000).optional(),
      waitUntil: z
        .union([lifecycle, z.array(lifecycle).min(1).max(4)])
        .optional(),
      referer: text.optional(),
      referrerPolicy: text.optional(),
    })
    .optional(),
  rejectRequestPattern: z.array(pattern).max(100).optional(),
  allowRequestPattern: z.array(pattern).max(100).optional(),
  rejectResourceTypes: z.array(resource).max(30).optional(),
  allowResourceTypes: z.array(resource).max(30).optional(),
  setExtraHTTPHeaders: z
    .record(z.string().max(256), z.string().max(8192))
    .optional(),
  setJavaScriptEnabled: z.boolean().optional(),
  userAgent: z.string().max(8192).optional(),
  viewport: z
    .strictObject({
      width: z.number().int().positive().max(16384),
      height: z.number().int().positive().max(16384),
      deviceScaleFactor: z.number().positive().max(8).optional(),
      isMobile: z.boolean().optional(),
      isLandscape: z.boolean().optional(),
      hasTouch: z.boolean().optional(),
    })
    .optional(),
  waitForSelector: z
    .strictObject({
      selector: text,
      hidden: z.literal(true).optional(),
      visible: z.literal(true).optional(),
      timeout: timeout.optional(),
    })
    .optional(),
  waitForTimeout: timeout.optional(),
  bestAttempt: z.boolean().optional(),
  actionTimeout: timeout.optional(),
  cacheTTL: z.number().int().min(0).max(86400).optional(),
};
const screenshot = z.strictObject({
  type: z.enum(["png", "jpeg", "webp"]).optional(),
  encoding: z.enum(["binary", "base64"]).optional(),
  quality: z.number().int().min(0).max(100).optional(),
  fullPage: z.boolean().optional(),
  clip: z
    .strictObject({
      x: z.number().nonnegative(),
      y: z.number().nonnegative(),
      width: z.number().positive(),
      height: z.number().positive(),
      scale: z.number().positive().optional(),
    })
    .optional(),
  omitBackground: z.boolean().optional(),
  optimizeForSpeed: z.boolean().optional(),
  captureBeyondViewport: z.boolean().optional(),
  fromSurface: z.boolean().optional(),
});
const pdf = z.strictObject({
  scale: z.number().min(0.1).max(2).optional(),
  displayHeaderFooter: z.boolean().optional(),
  headerTemplate: text.optional(),
  footerTemplate: text.optional(),
  printBackground: z.boolean().optional(),
  landscape: z.boolean().optional(),
  pageRanges: text.optional(),
  format: z
    .enum([
      "letter",
      "legal",
      "tabloid",
      "ledger",
      "a0",
      "a1",
      "a2",
      "a3",
      "a4",
      "a5",
      "a6",
    ])
    .optional(),
  width: z.union([text, z.number().positive()]).optional(),
  height: z.union([text, z.number().positive()]).optional(),
  margin: z
    .strictObject({
      top: z.union([text, z.number()]).optional(),
      right: z.union([text, z.number()]).optional(),
      bottom: z.union([text, z.number()]).optional(),
      left: z.union([text, z.number()]).optional(),
    })
    .optional(),
  preferCSSPageSize: z.boolean().optional(),
  omitBackground: z.boolean().optional(),
  tagged: z.boolean().optional(),
  outline: z.boolean().optional(),
  timeout: timeout.optional(),
});
const options = z
  .strictObject({
    ...common,
    screenshotOptions: screenshot.optional(),
    pdfOptions: pdf.optional(),
    selector: text.optional(),
    scrollPage: z.boolean().optional(),
    elements: z
      .array(z.strictObject({ selector: text }))
      .min(1)
      .max(100)
      .optional(),
    visibleLinksOnly: z.boolean().optional(),
    excludeExternalLinks: z.boolean().optional(),
    formats: z
      .array(z.enum(["content", "screenshot", "markdown", "accessibilityTree"]))
      .min(2)
      .max(4)
      .optional(),
    interestingOnly: z.boolean().optional(),
    root: text.optional(),
    prompt: text.min(1).optional(),
    custom_ai: z
      .array(
        z
          .strictObject({
            model: z
              .string()
              .min(3)
              .max(256)
              .regex(/^[A-Za-z0-9_-]+\/[!-~]+$/),
            authorization: z
              .string()
              .min(1)
              .max(4096)
              .regex(/^(?:[Bb][Ee][Aa][Rr][Ee][Rr] )?[!-~]+$/)
              .optional(),
          })
          .superRefine((model, ctx) => {
            if (
              model.authorization === undefined &&
              !model.model.startsWith("workers-ai/")
            )
              ctx.addIssue({
                code: "custom",
                message: "Model credential is required",
              });
          }),
      )
      .min(1)
      .max(3)
      .optional(),
    response_format: z
      .discriminatedUnion("type", [
        z.strictObject({ type: z.literal("json_object") }),
        z.strictObject({
          type: z.literal("json_schema"),
          json_schema: z.record(z.string(), z.json()),
        }),
      ])
      .optional(),
  })
  .superRefine((options, ctx) => {
    if ((options.url === undefined) === (options.html === undefined))
      ctx.addIssue({
        code: "custom",
        message: "Exactly one source is required",
      });
    if (
      options.url &&
      !["http:", "https:"].includes(new URL(options.url).protocol)
    )
      ctx.addIssue({ code: "custom", message: "Invalid navigation scheme" });
    if (
      (options.rejectRequestPattern && options.allowRequestPattern) ||
      (options.rejectResourceTypes && options.allowResourceTypes)
    )
      ctx.addIssue({ code: "custom", message: "Conflicting filters" });
    if (
      options.formats &&
      new Set(options.formats).size !== options.formats.length
    )
      ctx.addIssue({ code: "custom", message: "Duplicate formats" });
  });
export const actionRequest = z
  .strictObject({
    sessionId: z.uuid(),
    validateOnly: z.boolean().optional(),
    action: z.enum([
      "content",
      "screenshot",
      "pdf",
      "scrape",
      "links",
      "snapshot",
      "markdown",
      "json",
      "accessibilityTree",
    ]),
    options,
    maxResultBytes: z
      .number()
      .int()
      .positive()
      .max(128 * 1024 * 1024),
  })
  .superRefine((request, ctx) => {
    const fields: Record<typeof request.action, readonly string[]> = {
      content: [],
      markdown: [],
      json: ["prompt", "response_format", "custom_ai"],
      screenshot: ["screenshotOptions", "selector", "scrollPage"],
      pdf: ["pdfOptions"],
      scrape: ["elements"],
      links: ["visibleLinksOnly", "excludeExternalLinks"],
      snapshot: ["formats", "screenshotOptions"],
      accessibilityTree: ["interestingOnly", "root"],
    };
    const allowed = new Set([
      ...Object.keys(common),
      ...fields[request.action],
    ]);
    if (Object.keys(request.options).some((key) => !allowed.has(key)))
      ctx.addIssue({ code: "custom", message: "Unsupported action option" });
    if (
      request.action === "json" &&
      !request.options.prompt &&
      !request.options.response_format
    )
      ctx.addIssue({
        code: "custom",
        message: "Extraction instructions are required",
      });
    if (request.action === "scrape" && !request.options.elements)
      ctx.addIssue({ code: "custom", message: "Elements are required" });
    if (
      request.action === "snapshot" &&
      request.options.screenshotOptions?.encoding
    )
      ctx.addIssue({ code: "custom", message: "Snapshot encoding is fixed" });
  });

// Normalize optional JSON fields once before passing the validated values to Puppeteer.
type Defined<T> = T extends readonly unknown[]
  ? Defined<T[number]>[]
  : T extends object
    ? { [K in keyof T]: Defined<Exclude<T[K], undefined>> }
    : T;
export function defined<T>(value: T): Defined<T> {
  if (Array.isArray(value))
    return (value as readonly unknown[]).map((item) =>
      defined(item),
    ) as Defined<T>;
  if (value !== null && typeof value === "object")
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>)
        .filter(([, item]) => item !== undefined)
        .map(([key, item]) => [key, defined(item)]),
    ) as Defined<T>;
  return value as Defined<T>;
}
export type ActionRequest = z.infer<typeof actionRequest>;
export type ActionOptions = ActionRequest["options"];
