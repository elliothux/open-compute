import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { chromium } from "@playwright/test";
import Cloudflare from "cloudflare";
import { createOpenComputeClient } from "@open-compute/sdk";

const base =
  process.env.OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL ??
  "http://127.0.0.1:19787/operator/";
const token = process.env.OPEN_COMPUTE_ADMIN_TOKEN ?? "dev-admin-token";
const output = resolve(
  import.meta.dirname,
  "../../../dashboard-refactor/implementation-screenshots",
);
const api = new Cloudflare({
  apiToken: token,
  baseURL: new URL("/client/v4", base).href,
  maxRetries: 0,
});
const management = createOpenComputeClient({
  apiToken: token,
  baseURL: new URL("/client/v4", base).href,
});
const account = (await api.accounts.list()).result[0]?.id;
if (!account) throw new Error("No account available for visual QA.");
const suffix = crypto.randomUUID().slice(0, 8);
const names = {
  worker: `visual-worker-${suffix}`,
  kv: `visual-kv-${suffix}`,
  d1: `visual-d1-${suffix}`,
  r2: `visual-r2-${suffix}`,
  queue: `visual-queue-${suffix}`,
  workflow: `visual-workflow-${suffix}`,
  ai: `visual-ai-${suffix}`,
  aiInstance: `visual-ai-instance-${suffix}`,
};
let kvId: string | undefined;
let d1Id: string | undefined;
let queueId: string | undefined;
let folderFixture: string | undefined;
const browser = await chromium.launch({ channel: "chrome" });
const context = await browser.newContext({
  viewport: { width: 1440, height: 900 },
  deviceScaleFactor: 1,
});
const page = await context.newPage();

try {
  await mkdir(output, { recursive: true });
  const capabilities = await management.openCompute.capabilities.get();
  const worker = await api.workers.scripts.update(names.worker, {
    account_id: account,
    metadata: {
      main_module: "index.js",
      compatibility_date: capabilities.compatibility_date.maximum,
    },
    files: [
      new File(
        [
          "import { WorkflowEntrypoint } from 'cloudflare:workers'; export class Flow extends WorkflowEntrypoint { async run() { return { ok: true }; } } export default { fetch() { return new Response('visual QA'); } };",
        ],
        "index.js",
        { type: "application/javascript+module" },
      ),
    ],
  });
  if (!worker) throw new Error("Unable to create visual QA Worker.");
  kvId = (
    await api.kv.namespaces.create({ account_id: account, title: names.kv })
  ).id;
  d1Id = (await api.d1.database.create({ account_id: account, name: names.d1 }))
    .uuid;
  await api.r2.buckets.create({ account_id: account, name: names.r2 });
  queueId = (
    await api.queues.create({ account_id: account, queue_name: names.queue })
  ).queue_id;
  await api.workflows.update(names.workflow, {
    account_id: account,
    script_name: names.worker,
    class_name: "Flow",
  });
  await api.aiSearch.namespaces.create({ account_id: account, name: names.ai });
  await api.aiSearch.namespaces.instances.create(names.ai, {
    account_id: account,
    id: names.aiInstance,
    custom_metadata: [{ field_name: "folder", data_type: "text" }],
  });
  const aiJob = await api.aiSearch.namespaces.instances.jobs.create(
    names.aiInstance,
    {
      account_id: account,
      name: names.ai,
      description: "Visual QA sync",
    },
  );
  await page.goto(new URL("login", base).href);
  await page.getByLabel("Admin token").fill(token);
  await page.getByRole("button", { name: "Continue" }).click();
  await page.getByRole("heading", { name: "Account home" }).waitFor();

  const pages = [
    ["account-home", "", "Account home"],
    ["workers", "workers", "Workers"],
    ["observability", "observability", "Observability"],
    ["durable-objects", "durable-objects", "Durable Objects"],
    ["queues", "queues", "Queues"],
    ["workflows", "workflows", "Workflows"],
    ["kv", "kv", "Workers KV"],
    ["d1", "d1", "D1 databases"],
    ["r2", "r2", "R2 object storage"],
    ["vectorize", "vectorize", "Vectorize"],
    ["ai-search", "ai-search", "AI Search"],
    ["platform", "platform", "Platform"],
  ] as const;
  const createActions: Record<string, string> = {
    workers: "Create Worker",
    kv: "Create namespace",
    d1: "Create database",
    r2: "Create bucket",
    queues: "Create queue",
    workflows: "Create workflow",
    "ai-search": "Create instance",
  };

  for (const [file, path, heading] of pages) {
    await page.goto(new URL(path, base).href);
    await page.getByRole("heading", { name: heading, level: 1 }).waitFor();
    await page.waitForLoadState("networkidle");
    await page.screenshot({
      path: resolve(output, `${file}-desktop.png`),
      fullPage: true,
    });
    const createLabel = createActions[file];
    if (createLabel) {
      await page
        .getByRole("button", { name: createLabel, exact: true })
        .first()
        .click();
      if (file === "workers") {
        await page
          .getByRole("heading", { name: "Make something new" })
          .waitFor();
        await page.screenshot({
          path: resolve(output, "workers-create-desktop.png"),
          fullPage: false,
        });
        await page
          .getByRole("button", { name: "Start with Hello World!" })
          .click();
        await page
          .getByRole("heading", { name: "Deploy Hello World" })
          .waitFor();
        await page.screenshot({
          path: resolve(output, "workers-create-form-desktop.png"),
          fullPage: false,
        });
        await page.getByRole("button", { name: "Close" }).click();
      } else if (file === "ai-search") {
        await page
          .getByRole("heading", { name: "Name your instance" })
          .waitFor();
        await page.screenshot({
          path: resolve(output, "ai-search-create-desktop.png"),
          fullPage: false,
        });
        await page.getByRole("radio", { name: /R2 bucket/ }).check();
        await page.getByRole("button", { name: "Next" }).click();
        await page
          .getByRole("heading", { name: "Choose an R2 bucket" })
          .waitFor();
        await page.screenshot({
          path: resolve(output, "ai-search-create-r2-desktop.png"),
          fullPage: false,
        });
        await page.getByRole("radio", { name: names.r2 }).check();
        await page.getByRole("button", { name: "Next" }).click();
        await page.getByRole("heading", { name: "Review settings" }).waitFor();
        await page.screenshot({
          path: resolve(output, "ai-search-create-review-desktop.png"),
          fullPage: false,
        });
        await page.getByRole("button", { name: "Next" }).click();
        await page
          .getByRole("heading", { name: "Create instance", level: 2 })
          .waitFor();
        await page.screenshot({
          path: resolve(output, "ai-search-create-confirm-desktop.png"),
          fullPage: false,
        });
        await page.getByRole("button", { name: "Close" }).click();
      } else {
        const dialog = page.getByRole("dialog");
        await dialog.waitFor({ state: "visible" });
        await page.waitForFunction(() => {
          const dialog = document.querySelector('[role="dialog"]');
          return (
            dialog !== null && Number(getComputedStyle(dialog).opacity) >= 0.99
          );
        });
        await page.screenshot({
          path: resolve(output, `${file}-create-desktop.png`),
          fullPage: true,
        });
        await dialog.getByRole("button", { name: "Cancel" }).click();
      }
    }
    if (["workers", "kv", "d1", "r2", "queues", "workflows"].includes(file)) {
      const detail = page.locator(`main a[href^="/operator/${path}/"]`).first();
      if (await detail.count()) {
        await detail.click();
        await page.getByRole("heading", { level: 1 }).waitFor();
        await page.waitForLoadState("networkidle");
        await page.screenshot({
          path: resolve(output, `${file}-detail-desktop.png`),
          fullPage: true,
        });
      }
    }
  }

  await page.goto(
    new URL(`ai-search/namespace/${names.ai}/settings`, base).href,
  );
  await page.getByRole("heading", { name: "General" }).waitFor();
  await page.waitForLoadState("networkidle");
  await page.screenshot({
    path: resolve(output, "ai-search-namespace-settings-desktop.png"),
    fullPage: true,
  });
  await page.getByRole("button", { name: "Edit description" }).click();
  await page.getByLabel("Namespace description").waitFor();
  await page.screenshot({
    path: resolve(output, "ai-search-namespace-edit-desktop.png"),
    fullPage: true,
  });
  await page.getByRole("button", { name: "Cancel" }).click();
  await page.goto(
    new URL(`ai-search/namespace/${names.ai}/playground`, base).href,
  );
  await page.waitForLoadState("networkidle");
  await page.screenshot({
    path: resolve(output, "ai-search-namespace-playground-desktop.png"),
    fullPage: true,
  });
  const playgroundPath = `ai-search/${names.ai}/${names.aiInstance}?tab=playground`;
  await page.goto(new URL(playgroundPath, base).href);
  await page.getByRole("tab", { name: "Search" }).waitFor();
  await page.waitForLoadState("networkidle");
  await page.screenshot({
    path: resolve(
      output,
      "ai-search-instance-playground-search-empty-desktop.png",
    ),
    fullPage: true,
  });
  const playgroundChunk = {
    id: "visual-reference-chunk",
    type: "text",
    text: "Visual QA reference document for the AI Search Playground.",
    score: 0.535,
    item: { key: "reference.txt", timestamp: 1_700_000_000 },
    scoring_details: { vector_score: 0.535 },
  };
  await page.route(
    `**/ai-search/namespaces/${names.ai}/instances/${names.aiInstance}/search`,
    (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          success: true,
          errors: [],
          messages: [],
          result: {
            query_kind: "text",
            search_query: "reference",
            chunks: [playgroundChunk],
          },
        }),
      }),
  );
  await page.route(
    `**/ai-search/namespaces/${names.ai}/instances/${names.aiInstance}/chat/completions`,
    (route) =>
      route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        body: [
          `event: chunks\ndata: ${JSON.stringify([playgroundChunk])}\n\n`,
          'data: {"choices":[{"delta":{"content":"The reference describes "}}]}\n\n',
          'data: {"choices":[{"delta":{"content":"the Playground."}}]}\n\n',
          "data: [DONE]\n\n",
        ].join(""),
      }),
  );
  await page
    .getByRole("textbox", { name: "Search your documents" })
    .fill("reference");
  await page.getByRole("button", { name: "Search", exact: true }).click();
  await page.getByText(playgroundChunk.text).waitFor();
  await page.screenshot({
    path: resolve(
      output,
      "ai-search-instance-playground-search-result-desktop.png",
    ),
    fullPage: true,
  });
  await page.getByRole("tab", { name: "Chat" }).click();
  await page
    .getByRole("textbox", { name: "Enter your message" })
    .fill("What is in the reference?");
  await page.getByRole("button", { name: "Send message" }).click();
  await page.getByText("The reference describes the Playground.").waitFor();
  await page.getByRole("button", { name: "Sources (1)" }).click();
  await page.screenshot({
    path: resolve(
      output,
      "ai-search-instance-playground-chat-result-desktop.png",
    ),
    fullPage: true,
  });
  await page.getByRole("button", { name: "Edit" }).click();
  await page.getByRole("dialog").waitFor();
  await page
    .getByRole("dialog")
    .getByRole("heading", { name: "Metadata filters" })
    .waitFor();
  await page.waitForFunction(() => {
    const dialog = document.querySelector('[role="dialog"]');
    return dialog !== null && Number(getComputedStyle(dialog).opacity) >= 0.99;
  });
  await page.screenshot({
    path: resolve(output, "ai-search-instance-playground-filters-desktop.png"),
    fullPage: false,
  });
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Cancel" })
    .click();
  await page.unrouteAll();
  await page.goto(
    new URL(`ai-search/${names.ai}/${names.aiInstance}?tab=jobs`, base).href,
  );
  await page.getByRole("heading", { name: "Last job" }).waitFor();
  await page.getByRole("button", { name: aiJob.id }).waitFor();
  await page.waitForLoadState("networkidle");
  await page.screenshot({
    path: resolve(output, "ai-search-instance-jobs-desktop.png"),
    fullPage: true,
  });
  await page.getByRole("button", { name: aiJob.id }).click();
  await page.getByRole("dialog").waitFor();
  await page.waitForFunction(() => {
    const dialog = document.querySelector('[role="dialog"]');
    return dialog !== null && Number(getComputedStyle(dialog).opacity) >= 0.99;
  });
  await page.screenshot({
    path: resolve(output, "ai-search-instance-job-detail-desktop.png"),
    fullPage: false,
  });
  await page.getByRole("button", { name: "Close" }).click();
  await page.goto(
    new URL(`ai-search/${names.ai}/${names.aiInstance}?tab=items`, base).href,
  );
  await page.getByRole("heading", { name: "No items found" }).waitFor();
  await page.getByLabel("Loading").waitFor({ state: "detached" });
  await page.screenshot({
    path: resolve(output, "ai-search-instance-items-initial-empty-desktop.png"),
    fullPage: true,
  });
  await page.getByRole("button", { name: "Upload file", exact: true }).click();
  const uploadDialog = page.getByRole("dialog");
  await uploadDialog.getByRole("heading", { name: "Upload files" }).waitFor();
  await page.waitForFunction(() => {
    const dialog = document.querySelector('[role="dialog"]');
    return dialog !== null && Number(getComputedStyle(dialog).opacity) >= 0.99;
  });
  await page.screenshot({
    path: resolve(output, "ai-search-instance-items-upload-desktop.png"),
    fullPage: false,
  });
  await uploadDialog.getByLabel("Choose files").setInputFiles({
    name: "reference.txt",
    mimeType: "text/plain",
    buffer: Buffer.from("Visual QA reference document"),
  });
  await uploadDialog.getByText("reference.txt").waitFor();
  await page.screenshot({
    path: resolve(
      output,
      "ai-search-instance-items-upload-selected-desktop.png",
    ),
    fullPage: false,
  });
  await uploadDialog
    .getByRole("button", { name: "Remove reference.txt" })
    .click();
  const tempRoot = resolve(import.meta.dirname, "../../../.temp");
  await mkdir(tempRoot, { recursive: true });
  folderFixture = await mkdtemp(resolve(tempRoot, "visual-ai-folder-"));
  await mkdir(resolve(folderFixture, "docs"));
  await writeFile(
    resolve(folderFixture, "docs/reference.txt"),
    "Folder visual QA",
  );
  await uploadDialog.getByLabel("Choose a folder").setInputFiles(folderFixture);
  await uploadDialog.getByText(/docs\/reference\.txt/).waitFor();
  await page.screenshot({
    path: resolve(output, "ai-search-instance-items-upload-folder-desktop.png"),
    fullPage: false,
  });
  await uploadDialog.getByRole("button", { name: "Close" }).click();
  await page
    .getByRole("heading", { name: "Upload files" })
    .waitFor({ state: "hidden" });
  await api.aiSearch.namespaces.instances.items.upload(names.aiInstance, {
    account_id: account,
    name: names.ai,
    file: {
      file: new File(["Visual QA reference document"], "reference.txt", {
        type: "text/plain",
      }),
      metadata: JSON.stringify({ folder: "docs" }),
    },
  });
  await page.getByRole("button", { name: "Refresh items" }).click();
  await page.getByRole("cell", { name: "reference.txt" }).waitFor();
  await page.getByLabel("Loading").waitFor({ state: "detached" });
  await page.waitForLoadState("networkidle");
  await page.screenshot({
    path: resolve(output, "ai-search-instance-items-desktop.png"),
    fullPage: true,
  });
  await page.getByRole("cell", { name: "reference.txt", exact: true }).click();
  await page.getByRole("heading", { name: "Processing logs" }).waitFor();
  await page.screenshot({
    path: resolve(output, "ai-search-instance-item-detail-desktop.png"),
    fullPage: true,
  });
  await page.getByRole("button", { name: "Delete", exact: true }).click();
  await page.getByRole("alertdialog").waitFor();
  await page.waitForFunction(() => {
    const dialog = document.querySelector('[role="alertdialog"]');
    return dialog !== null && Number(getComputedStyle(dialog).opacity) >= 0.99;
  });
  await page.screenshot({
    path: resolve(output, "ai-search-instance-item-delete-desktop.png"),
    fullPage: false,
  });
  await page
    .getByRole("alertdialog")
    .getByRole("button", { name: "Cancel" })
    .click();
  await page.getByRole("cell", { name: "reference.txt", exact: true }).click();
  await page.getByRole("button", { name: "Filters" }).click();
  await page.getByRole("button", { name: "Add filter" }).click();
  await page.getByRole("textbox", { name: "Value" }).fill("docs");
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.screenshot({
    path: resolve(output, "ai-search-instance-items-filter-desktop.png"),
    fullPage: false,
  });
  await page.getByRole("button", { name: "Apply" }).click();
  await page
    .getByRole("button", { name: "Apply" })
    .waitFor({ state: "detached" });
  await page.getByRole("textbox", { name: "Search items" }).fill("missing");
  await page.getByRole("heading", { name: "No matching items" }).waitFor();
  await page.screenshot({
    path: resolve(output, "ai-search-instance-items-empty-desktop.png"),
    fullPage: true,
  });

  await page.setViewportSize({ width: 390, height: 844 });
  for (const [file, path, heading] of pages) {
    await page.goto(new URL(path, base).href);
    await page.getByRole("heading", { name: heading, level: 1 }).waitFor();
    await page.waitForLoadState("networkidle");
    await page.screenshot({
      path: resolve(output, `${file}-mobile.png`),
      fullPage: true,
    });
    if (file === "workers") {
      await page.getByRole("button", { name: "Create Worker" }).first().click();
      await page.getByRole("heading", { name: "Make something new" }).waitFor();
      await page.screenshot({
        path: resolve(output, "workers-create-mobile.png"),
        fullPage: false,
      });
      await page
        .getByRole("button", { name: "Start with Hello World!" })
        .click();
      await page.getByRole("heading", { name: "Deploy Hello World" }).waitFor();
      await page.screenshot({
        path: resolve(output, "workers-create-form-mobile.png"),
        fullPage: false,
      });
    } else if (file === "ai-search") {
      await page
        .getByRole("button", { name: "Create instance" })
        .first()
        .click();
      await page.getByRole("heading", { name: "Name your instance" }).waitFor();
      await page.screenshot({
        path: resolve(output, "ai-search-create-mobile.png"),
        fullPage: false,
      });
    }
  }
  await page.goto(
    new URL(`ai-search/namespace/${names.ai}/settings`, base).href,
  );
  await page.getByRole("heading", { name: "General" }).waitFor();
  await page.waitForLoadState("networkidle");
  await page.screenshot({
    path: resolve(output, "ai-search-namespace-settings-mobile.png"),
    fullPage: true,
  });
  await page.goto(
    new URL(`ai-search/${names.ai}/${names.aiInstance}?tab=jobs`, base).href,
  );
  await page.getByRole("button", { name: aiJob.id }).waitFor();
  await page.screenshot({
    path: resolve(output, "ai-search-instance-jobs-mobile.png"),
    fullPage: true,
  });
  await page.goto(new URL(playgroundPath, base).href);
  await page.getByRole("tab", { name: "Search" }).waitFor();
  await page.waitForLoadState("networkidle");
  await page.screenshot({
    path: resolve(
      output,
      "ai-search-instance-playground-search-empty-mobile.png",
    ),
    fullPage: true,
  });
  await page.goto(
    new URL(`ai-search/${names.ai}/${names.aiInstance}?tab=items`, base).href,
  );
  await page.getByRole("cell", { name: "reference.txt" }).waitFor();
  await page.getByLabel("Loading").waitFor({ state: "detached" });
  await page.screenshot({
    path: resolve(output, "ai-search-instance-items-mobile.png"),
    fullPage: true,
  });
} finally {
  await browser.close();
  if (folderFixture) await rm(folderFixture, { recursive: true, force: true });
  await Promise.allSettled([
    api.workflows.delete(names.workflow, { account_id: account }),
    api.aiSearch.namespaces.instances
      .delete(names.aiInstance, {
        account_id: account,
        name: names.ai,
      })
      .then(() =>
        api.aiSearch.namespaces.delete(names.ai, { account_id: account }),
      ),
    queueId
      ? api.queues.delete(queueId, { account_id: account })
      : Promise.resolve(),
    api.r2.buckets.delete(names.r2, { account_id: account }),
    d1Id
      ? api.d1.database.delete(d1Id, { account_id: account })
      : Promise.resolve(),
    kvId
      ? api.kv.namespaces.delete(kvId, { account_id: account })
      : Promise.resolve(),
  ]);
  await api.workers.scripts
    .delete(names.worker, { account_id: account })
    .catch(() => {});
}
