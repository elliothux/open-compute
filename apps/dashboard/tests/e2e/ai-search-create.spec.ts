import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import Cloudflare from "cloudflare";
import { expect, test } from "./fixtures";
import { adminToken, signIn } from "./helpers";

function client() {
  const root =
    process.env.OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL ??
    "http://127.0.0.1:8787/operator/";
  return new Cloudflare({
    apiToken: adminToken,
    baseURL: new URL("/client/v4", root).href,
    maxRetries: 0,
  });
}

async function deleteAiSearchFixture(
  api: Cloudflare,
  account: string,
  namespace: string,
  instance?: string,
) {
  const deleted = async (action: () => Promise<unknown>) => {
    await expect
      .poll(
        async () => {
          try {
            await action();
            return true;
          } catch (error) {
            const status =
              error && typeof error === "object" && "status" in error
                ? error.status
                : null;
            if (status === 404) return true;
            if (status === 409 || status === 503) return false;
            throw error;
          }
        },
        { timeout: 30_000, intervals: [250, 500, 1_000] },
      )
      .toBe(true);
  };
  if (instance)
    await deleted(() =>
      api.aiSearch.namespaces.instances.delete(instance, {
        account_id: account,
        name: namespace,
      }),
    );
  await deleted(() =>
    api.aiSearch.namespaces.delete(namespace, { account_id: account }),
  );
}

test.describe("AI Search create stepper", () => {
  test.setTimeout(120_000);

  test("creates a built-in instance after settings review", async ({
    page,
  }) => {
    const api = client();
    const account = (await api.accounts.list()).result[0]?.id;
    if (!account) throw new Error("Missing isolated test account.");
    const namespace = `pw-ai-${crypto.randomUUID().slice(0, 8)}`;
    const name = `pw-search-${crypto.randomUUID().slice(0, 8)}`;
    await api.aiSearch.namespaces.create({
      account_id: account,
      name: namespace,
    });
    try {
      await signIn(page);
      await page.goto("./ai-search");
      await page
        .getByRole("button", { name: "Create instance" })
        .first()
        .click();
      await expect(
        page.getByRole("heading", { name: "Name your instance" }),
      ).toBeVisible();
      await page.getByLabel("Instance name").fill(name);
      await page.getByLabel("Namespace").selectOption(namespace);
      await page.getByRole("button", { name: "Next" }).click();
      await expect(
        page.getByRole("heading", { name: "Review settings" }),
      ).toBeVisible();
      await page.getByText("Retrieval · 10 results · score 0.4").click();
      await page.getByLabel("Maximum results").fill("0");
      await expect(page.getByRole("button", { name: "Next" })).toBeDisabled();
      await page.getByLabel("Maximum results").fill("12");
      await page.getByRole("button", { name: "Next" }).click();
      await expect(
        page.getByRole("heading", { name: "Create instance", level: 2 }),
      ).toBeVisible();
      await page.getByRole("button", { name: "Create", exact: true }).click();
      await expect(page.getByRole("heading", { name, level: 1 })).toBeVisible({
        timeout: 15_000,
      });
      const created = await api.aiSearch.namespaces.instances.read(name, {
        account_id: account,
        name: namespace,
      });
      expect(created.max_num_results).toBe(12);
    } finally {
      await deleteAiSearchFixture(api, account, namespace, name);
    }
  });

  test("selects a same-account R2 bucket without browser credentials", async ({
    page,
  }) => {
    const api = client();
    const account = (await api.accounts.list()).result[0]?.id;
    if (!account) throw new Error("Missing isolated test account.");
    const namespace = `pw-ai-${crypto.randomUUID().slice(0, 8)}`;
    const name = `pw-search-${crypto.randomUUID().slice(0, 8)}`;
    const bucket = `pw-ai-bucket-${crypto.randomUUID().slice(0, 8)}`;
    await api.aiSearch.namespaces.create({
      account_id: account,
      name: namespace,
    });
    await api.r2.buckets.create({ account_id: account, name: bucket });
    try {
      await signIn(page);
      await page.goto("./ai-search");
      await page
        .getByRole("button", { name: "Create instance" })
        .first()
        .click();
      await page.getByLabel("Instance name").fill(name);
      await page.getByLabel("Namespace").selectOption(namespace);
      await page.getByRole("radio", { name: /R2 bucket/ }).check();
      await page.getByRole("button", { name: "Next" }).click();
      await expect(
        page.getByRole("heading", { name: "Choose an R2 bucket" }),
      ).toBeVisible();
      await page.getByRole("radio", { name: bucket }).check();
      await page.getByLabel("Object key prefix (optional)").fill("docs/");
      await page.getByRole("button", { name: "Next" }).click();
      await page.getByRole("button", { name: "Next" }).click();
      await expect(page.getByText(`R2 · ${bucket}`)).toBeVisible();
      await page.getByRole("button", { name: "Create", exact: true }).click();
      await expect(page.getByRole("heading", { name, level: 1 })).toBeVisible({
        timeout: 15_000,
      });
    } finally {
      await deleteAiSearchFixture(api, account, namespace, name);
      await api.r2.buckets.delete(bucket, { account_id: account });
    }
  });
});

test("selects a namespace and edits its description inline", async ({
  page,
}) => {
  const api = client();
  const account = (await api.accounts.list()).result[0]?.id;
  if (!account) throw new Error("Missing isolated test account.");
  const namespace = `pw-ai-z-${crypto.randomUUID().slice(0, 8)}`;
  const otherNamespace = `pw-ai-a-${crypto.randomUUID().slice(0, 8)}`;
  await api.aiSearch.namespaces.create({
    account_id: account,
    name: otherNamespace,
  });
  await api.aiSearch.namespaces.create({
    account_id: account,
    name: namespace,
  });
  try {
    await signIn(page);
    await page.goto("./ai-search");
    await page
      .getByRole("combobox", { name: "Namespace" })
      .selectOption(namespace);
    await expect(page).toHaveURL(new RegExp(`namespace=${namespace}`));
    await page.getByRole("button", { name: "Settings" }).click();
    await expect(page.getByRole("heading", { name: "General" })).toBeVisible();
    await page.getByRole("button", { name: "Edit description" }).click();
    await page.getByLabel("Namespace description").fill("Discard this text");
    await page.getByRole("button", { name: "Cancel" }).click();
    await expect(page.getByText("No description")).toBeVisible();
    await page.getByRole("button", { name: "Edit description" }).click();
    await page
      .getByLabel("Namespace description")
      .fill("Dashboard QA namespace");
    await page.getByRole("button", { name: "Save" }).click();
    await expect(page.getByText("Dashboard QA namespace")).toBeVisible();
    expect(
      (await api.aiSearch.namespaces.read(namespace, { account_id: account }))
        .description,
    ).toBe("Dashboard QA namespace");
    await page.getByRole("button", { name: "Delete namespace" }).click();
    await page.getByLabel("Resource name").fill(namespace);
    await page.getByRole("button", { name: "Delete", exact: true }).click();
    await expect(
      page.getByRole("heading", { name: "AI Search", level: 1 }),
    ).toBeVisible();
  } finally {
    await api.aiSearch.namespaces
      .delete(namespace, { account_id: account })
      .catch(() => {});
    await api.aiSearch.namespaces
      .delete(otherNamespace, { account_id: account })
      .catch(() => {});
  }
});

test("shows AI Search job history and logs in a detail dialog", async ({
  page,
}) => {
  const api = client();
  const account = (await api.accounts.list()).result[0]?.id;
  if (!account) throw new Error("Missing isolated test account.");
  const namespace = `pw-ai-${crypto.randomUUID().slice(0, 8)}`;
  const name = `pw-search-${crypto.randomUUID().slice(0, 8)}`;
  await api.aiSearch.namespaces.create({
    account_id: account,
    name: namespace,
  });
  try {
    await api.aiSearch.namespaces.instances.create(namespace, {
      account_id: account,
      id: name,
    });
    const job = await api.aiSearch.namespaces.instances.jobs.create(name, {
      account_id: account,
      name: namespace,
      description: "Dashboard job detail test",
    });
    await signIn(page);
    await page.goto(`./ai-search/${namespace}/${name}?tab=jobs`);
    await expect(page.getByRole("heading", { name: "Last job" })).toBeVisible();
    await expect(page.getByRole("button", { name: job.id })).toBeVisible();
    await page.getByRole("button", { name: job.id }).click();
    await expect(page.getByRole("dialog")).toContainText(
      `Job ${job.id.slice(0, 8)}`,
    );
    await expect(page.getByRole("dialog")).toContainText("Source");
    await page.getByRole("button", { name: "Close" }).click();
    await expect(page.getByRole("dialog")).toHaveCount(0);
  } finally {
    await deleteAiSearchFixture(api, account, namespace, name);
  }
});

test("filters AI Search items through the server and keeps the current page bounded", async ({
  page,
}) => {
  test.setTimeout(90_000);
  const capture = process.env.OPEN_COMPUTE_CAPTURE_AI_SEARCH_DROP === "1";
  if (capture) await page.setViewportSize({ width: 1808, height: 1113 });
  const api = client();
  const account = (await api.accounts.list()).result[0]?.id;
  if (!account) throw new Error("Missing isolated test account.");
  const namespace = `pw-ai-${crypto.randomUUID().slice(0, 8)}`;
  const name = `pw-search-${crypto.randomUUID().slice(0, 8)}`;
  const tempRoot = fileURLToPath(
    new URL("../../../../.temp/", import.meta.url),
  );
  await mkdir(tempRoot, { recursive: true });
  const folder = await mkdtemp(join(tempRoot, "ai-search-folder-"));
  for (const directory of ["docs", "other"]) {
    await mkdir(join(folder, directory));
    await writeFile(join(folder, directory, "same.txt"), directory);
  }
  await api.aiSearch.namespaces.create({
    account_id: account,
    name: namespace,
  });
  try {
    await api.aiSearch.namespaces.instances.create(namespace, {
      account_id: account,
      id: name,
      custom_metadata: [{ field_name: "category", data_type: "text" }],
    });
    await api.aiSearch.namespaces.instances.items.upload(name, {
      account_id: account,
      name: namespace,
      file: {
        file: new File(["Visual QA text"], "guide.txt", { type: "text/plain" }),
        metadata: JSON.stringify({ category: "guide" }),
      },
    });
    await signIn(page);
    await page.goto(`./ai-search/${namespace}/${name}?tab=items`);
    await expect(
      page.getByRole("cell", { name: "guide.txt", exact: true }),
    ).toBeVisible();
    await page.getByRole("cell", { name: "guide.txt", exact: true }).click();
    await expect(page.getByRole("button", { name: "Reindex" })).toBeVisible();
    await expect(page.getByRole("button", { name: "Download" })).toBeVisible();
    await expect(page.getByRole("heading", { name: "Chunks" })).toBeVisible();
    await expect(
      page.getByRole("heading", { name: "Processing logs" }),
    ).toBeVisible();
    if (capture) {
      await page.waitForLoadState("networkidle");
      await page.screenshot({
        path: resolve(
          tempRoot,
          "../dashboard-refactor/implementation-screenshots/ai-search-instance-item-detail-desktop.png",
        ),
        fullPage: true,
      });
    }
    await page.getByRole("button", { name: "Delete", exact: true }).click();
    await expect(page.getByRole("alertdialog")).toContainText("guide.txt");
    if (capture) {
      await expect(page.getByRole("alertdialog")).toHaveCSS("opacity", "1");
      await page.getByRole("alertdialog").evaluate(async (element) => {
        await Promise.all(
          element
            .getAnimations({ subtree: true })
            .map((animation) => animation.finished),
        );
      });
      await page.screenshot({
        path: resolve(
          tempRoot,
          "../dashboard-refactor/implementation-screenshots/ai-search-instance-item-delete-desktop.png",
        ),
        fullPage: false,
      });
    }
    await page
      .getByRole("alertdialog")
      .getByRole("button", { name: "Cancel" })
      .click();
    await expect(
      page.getByRole("cell", { name: "guide.txt", exact: true }),
    ).toBeVisible();
    await page
      .getByRole("button", { name: "Upload file", exact: true })
      .click();
    const uploadDialog = page.getByRole("dialog");
    await expect(
      uploadDialog.getByRole("heading", { name: "Upload files" }),
    ).toBeVisible();
    await expect(
      uploadDialog.getByRole("button", { name: "Upload files" }),
    ).toBeDisabled();
    await uploadDialog.getByLabel("Choose files").setInputFiles({
      name: "empty.txt",
      mimeType: "text/plain",
      buffer: Buffer.alloc(0),
    });
    await expect(uploadDialog.getByRole("alert")).toContainText("nonempty");
    await uploadDialog.getByLabel("Choose files").setInputFiles({
      name: "second.txt",
      mimeType: "text/plain",
      buffer: Buffer.from("Second local document"),
    });
    await expect(uploadDialog.getByText("second.txt")).toBeVisible();
    await uploadDialog.getByRole("button", { name: "Upload files" }).click();
    await expect(uploadDialog.getByLabel("Upload progress")).toContainText(
      "Uploaded 1/1",
    );
    await uploadDialog.getByRole("button", { name: "Close" }).click();
    await expect(
      page.getByRole("heading", { name: "Upload files" }),
    ).toBeHidden();
    await expect(page.getByRole("cell", { name: "second.txt" })).toBeVisible();
    await page
      .getByRole("button", { name: "Upload file", exact: true })
      .click();
    const folderDialog = page.getByRole("dialog", { name: "Upload files" });
    await folderDialog.getByLabel("Choose a folder").setInputFiles(folder);
    await expect(
      folderDialog.locator('[title*="docs/same.txt"]'),
    ).toBeVisible();
    await expect(
      folderDialog.locator('[title*="other/same.txt"]'),
    ).toBeVisible();
    await folderDialog.getByRole("button", { name: "Upload files" }).click();
    await expect(folderDialog.getByLabel("Upload progress")).toContainText(
      "Uploaded 2/2",
    );
    await expect(folderDialog.getByLabel("Upload progress")).toContainText(
      "2 succeeded, 0 failed",
    );
    await expect(
      folderDialog.getByRole("button", { name: "Upload files" }),
    ).toBeDisabled();
    if (process.env.OPEN_COMPUTE_CAPTURE_AI_SEARCH_DROP === "1") {
      await folderDialog.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/ai-search-instance-items-upload-folder-complete-desktop.png",
        ),
      });
    }
    await folderDialog.getByRole("button", { name: "Close" }).click();
    await expect(
      page.getByRole("cell", { name: /docs\/same\.txt/ }),
    ).toBeVisible();
    await expect(
      page.getByRole("cell", { name: /other\/same\.txt/ }),
    ).toBeVisible();
    await page
      .getByRole("button", { name: "Upload file", exact: true })
      .click();
    const droppedDialog = page.getByRole("dialog", { name: "Upload files" });
    await droppedDialog
      .getByLabel("Drop files or folders")
      .evaluate((element) => {
        const entries = Array.from({ length: 101 }, (_, index) => ({
          isFile: true,
          isDirectory: false,
          name: `file-${index}.txt`,
          file: (done: (value: File) => void) =>
            done(new File(["x"], `file-${index}.txt`)),
        }));
        let batch = 0;
        const root = {
          isFile: false,
          isDirectory: true,
          name: "too-many",
          createReader: () => ({
            readEntries: (done: (value: object[]) => void) =>
              done(
                batch++ === 0
                  ? entries.slice(0, 100)
                  : batch === 2
                    ? entries.slice(100)
                    : [],
              ),
          }),
        };
        const drop = new Event("drop", { bubbles: true, cancelable: true });
        Object.defineProperty(drop, "dataTransfer", {
          value: { items: [{ webkitGetAsEntry: () => root }], files: [] },
        });
        element.dispatchEvent(drop);
      });
    await expect(droppedDialog.getByRole("alert")).toContainText(
      "up to 100 files",
      { timeout: 15_000 },
    );
    await droppedDialog
      .getByLabel("Drop files or folders")
      .evaluate((element) => {
        const file = (content: string) => ({
          isFile: true,
          isDirectory: false,
          name: "same.txt",
          file: (done: (value: File) => void) =>
            done(new File([content], "same.txt", { type: "text/plain" })),
        });
        const directory = (name: string, batches: object[][]) => ({
          isFile: false,
          isDirectory: true,
          name,
          createReader: () => ({
            readEntries: (done: (value: object[]) => void) =>
              done(batches.shift() ?? []),
          }),
        });
        const root = directory("dropped", [
          [directory("docs", [[file("docs")], []])],
          [directory("other", [[file("other")], []])],
          [],
        ]);
        const drop = new Event("drop", { bubbles: true, cancelable: true });
        Object.defineProperty(drop, "dataTransfer", {
          value: {
            items: [{ webkitGetAsEntry: () => root }],
            files: [],
          },
        });
        element.dispatchEvent(drop);
      });
    await expect(
      droppedDialog.locator('[title="dropped/docs/same.txt"]'),
    ).toBeVisible();
    await expect(
      droppedDialog.locator('[title="dropped/other/same.txt"]'),
    ).toBeVisible();
    await expect(droppedDialog.getByRole("alert")).toHaveCount(0);
    if (process.env.OPEN_COMPUTE_CAPTURE_AI_SEARCH_DROP === "1") {
      const output = resolve(
        import.meta.dirname,
        "../../../../dashboard-refactor/implementation-screenshots",
      );
      await mkdir(output, { recursive: true });
      await expect
        .poll(() =>
          droppedDialog.evaluate((dialog) =>
            Number(getComputedStyle(dialog).opacity),
          ),
        )
        .toBeGreaterThanOrEqual(0.99);
      await expect(page.getByText("2 files uploaded for indexing.")).toBeHidden(
        { timeout: 10_000 },
      );
      await page.screenshot({
        path: resolve(
          output,
          "ai-search-instance-items-upload-dropped-folder-desktop.png",
        ),
      });
    }
    await droppedDialog.getByRole("button", { name: "Upload files" }).click();
    await expect(droppedDialog.getByLabel("Upload progress")).toContainText(
      "Uploaded 2/2",
    );
    await droppedDialog.getByRole("button", { name: "Close" }).click();
    await expect(
      page.getByRole("cell", { name: "dropped/docs/same.txt" }),
    ).toBeVisible();
    await expect(
      page.getByRole("cell", { name: "dropped/other/same.txt" }),
    ).toBeVisible();
    await page.getByRole("button", { name: /^Filters(?: \d+)?$/ }).click();
    await page.getByRole("button", { name: "Add filter" }).click();
    await page.getByRole("textbox", { name: "Value" }).fill("missing");
    const filtered = page.waitForRequest(
      (request) =>
        request.url().includes("metadata_filter=") &&
        request.url().includes("per_page=20") &&
        request.url().includes("sort_by=modified_at"),
    );
    await page.getByRole("button", { name: "Apply" }).click();
    expect(
      new URL((await filtered).url()).searchParams.get("metadata_filter"),
    ).toBe(JSON.stringify({ category: "missing" }));
    await expect(
      page.getByRole("heading", { name: "No matching items" }),
    ).toBeVisible();
    await page.getByRole("button", { name: /^Filters(?: \d+)?$/ }).click();
    await page.getByRole("button", { name: "Clear all", exact: true }).click();
    await page.getByRole("button", { name: "Apply" }).click();
    await expect(
      page.getByRole("cell", { name: "guide.txt", exact: true }),
    ).toBeVisible();
    await page.getByRole("button", { name: /^Filters(?: \d+)?$/ }).click();
    await page.getByRole("combobox", { name: "Source" }).click();
    await page.getByRole("option", { name: "Built-in storage" }).click();
    const sourced = page.waitForRequest(
      (request) =>
        new URL(request.url()).searchParams.get("source") === "builtin",
    );
    await page.getByRole("button", { name: "Apply" }).click();
    await sourced;
    await expect(
      page.getByRole("cell", { name: "guide.txt", exact: true }),
    ).toBeVisible();
    await page.getByRole("combobox", { name: "Item status" }).click();
    await page.getByRole("option", { name: "Skipped" }).click();
    await expect(
      page.getByRole("heading", { name: "No matching items" }),
    ).toBeVisible();
    await page.getByRole("combobox", { name: "Item status" }).click();
    await page.getByRole("option", { name: "All" }).click();
    await expect(
      page.getByRole("cell", { name: "guide.txt", exact: true }),
    ).toBeVisible();
    await page.getByRole("textbox", { name: "Search items" }).fill("not-there");
    await expect(
      page.getByRole("heading", { name: "No matching items" }),
    ).toBeVisible();
  } finally {
    await rm(folder, { recursive: true, force: true });
    await deleteAiSearchFixture(api, account, namespace, name);
  }
});
