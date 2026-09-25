import { resolve } from "node:path";
import type { Page } from "@playwright/test";
import Cloudflare from "cloudflare";
import { expect, test } from "./fixtures";
import { adminToken, expectNoLoadErrors, signIn } from "./helpers";

function liveClient() {
  const dashboardRoot =
    process.env.OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL ??
    "http://127.0.0.1:8787/operator/";
  return new Cloudflare({
    apiToken: adminToken,
    baseURL: new URL("/client/v4", dashboardRoot).href,
    maxRetries: 0,
  });
}

async function accountId(client: Cloudflare): Promise<string> {
  const accounts = await client.accounts.list();
  const id = accounts.result[0]?.id;
  if (id === undefined) throw new Error("dashboard E2E account is missing");
  return id;
}

async function openCatalog(page: Page, name: string) {
  await page
    .getByRole("navigation", { name: "Primary navigation" })
    .getByRole("link", {
      name,
      exact: true,
    })
    .first()
    .click();
}

async function returnToCatalog(page: Page, name: string) {
  await openCatalog(page, name);
}

async function createCatalogResource(
  page: Page,
  name: string,
  kind: "database" | "namespace" | "bucket",
) {
  await page
    .getByRole("button", { name: `Create ${kind}`, exact: true })
    .first()
    .click();
  if (kind === "database") {
    await expect(
      page.getByRole("heading", { name: "Create D1 database" }),
    ).toBeVisible();
    await page.getByRole("textbox", { name: "Name" }).fill(name);
    await page.getByRole("button", { name: "Create", exact: true }).click();
    await expect(page).toHaveURL(/\/d1\/[^/]+$/);
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
    return;
  }
  if (kind === "bucket") {
    await expect(page).toHaveURL(/\/r2\/new$/);
    await expect(
      page.getByRole("heading", { name: "Create a bucket" }),
    ).toBeVisible();
    await page.getByRole("textbox", { name: "Bucket name" }).fill(name);
    await page.getByRole("button", { name: "Create bucket" }).click();
    await expect(page).toHaveURL(/\/r2\/[^/?]+(?:\?.*)?$/);
    return;
  }
  const dialog = page.getByRole("dialog");
  await dialog
    .getByLabel(`${kind[0]!.toUpperCase()}${kind.slice(1)} name`, {
      exact: true,
    })
    .fill(name);
  await dialog.getByRole("button", { name: "Create", exact: true }).click();
  await expect(page.getByRole("link", { name, exact: true })).toBeVisible({
    timeout: 15_000,
  });
}

async function renameCatalogResource(
  page: Page,
  currentName: string,
  newName: string,
) {
  await page
    .getByRole("button", { name: `Rename ${currentName}`, exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("textbox").fill(newName);
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(
    page.getByRole("link", { name: newName, exact: true }),
  ).toBeVisible({ timeout: 15_000 });
}

async function deleteCatalogResource(page: Page, name: string) {
  await page
    .getByRole("button", { name: `Delete ${name}`, exact: true })
    .click();
  const dialog = page.getByRole("alertdialog");
  await dialog.getByLabel("Resource name").fill(name);
  await dialog.getByRole("button", { name: "Delete", exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByRole("link", { name, exact: true })).toHaveCount(0);
}

test.describe("operator dashboard live lifecycle", () => {
  test.setTimeout(120_000);

  test.beforeEach(async ({ page }) => {
    await signIn(page);
  });

  test("Worker create, detail, and deletion use the browser SDK", async ({
    page,
  }) => {
    const name = `pw-worker-${crypto.randomUUID().replaceAll("-", "")}`;
    await openCatalog(page, "Workers");
    await page.getByRole("button", { name: "Create Worker" }).first().click();
    await expect(
      page.getByRole("heading", { name: "Make something new" }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Start with Hello World!" }).click();
    await expect(
      page.getByRole("heading", { name: "Deploy Hello World" }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Back" }).click();
    await page.getByRole("button", { name: "Start with Hello World!" }).click();
    await page.getByLabel("Worker name").fill(name);
    await page.getByRole("button", { name: "Deploy", exact: true }).click();
    await expect(page.getByRole("heading", { name, level: 1 })).toBeVisible();
    await page.getByRole("tab", { name: "Settings" }).click();
    await expect(
      page.getByRole("heading", { name: "Runtime variables and secrets" }),
    ).toBeVisible();
    await expect(page.getByRole("heading", { name: "Bindings" })).toBeVisible();
    await expect(
      page.getByRole("heading", { name: "Trigger events" }),
    ).toBeVisible();
    await expect(page.getByText("CPU time limit").locator("..")).toContainText(
      /\d+ ms/,
    );
    await expect(
      page.getByRole("button", { name: "Add secret" }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Add binding" }),
    ).toBeVisible();
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VISUAL === "1") {
      await page.setViewportSize({ width: 1808, height: 900 });
      await page.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-settings-desktop.png",
        ),
        fullPage: true,
      });
      await page.setViewportSize({ width: 390, height: 844 });
      await page.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-settings-mobile.png",
        ),
        fullPage: true,
      });
      await page.setViewportSize({ width: 1280, height: 720 });
    }
    await returnToCatalog(page, "Workers");
    await deleteCatalogResource(page, name);
  });

  test("Worker module upload uses the same two-step create flow", async ({
    page,
  }) => {
    const name = `pw-upload-${crypto.randomUUID().replaceAll("-", "")}`;
    await openCatalog(page, "Workers");
    await page.getByRole("button", { name: "Create Worker" }).first().click();
    await page.getByRole("button", { name: "Upload a Worker module" }).click();
    await page.getByLabel("Worker name").fill(name);
    await page.getByLabel("Worker module").setInputFiles({
      name: "index.js",
      mimeType: "application/javascript",
      buffer: Buffer.from(
        "export default { fetch() { return new Response('uploaded'); } };",
      ),
    });
    await page.getByRole("button", { name: "Deploy", exact: true }).click();
    await expect(page.getByRole("heading", { name, level: 1 })).toBeVisible();
    await returnToCatalog(page, "Workers");
    await deleteCatalogResource(page, name);
  });

  test("D1 create, query, backup, restore, detail, and deletion stay canonical", async ({
    page,
  }) => {
    const name = `pw-d1-${crypto.randomUUID().replaceAll("-", "")}`;
    const restoredName = `${name}-restored`;
    await openCatalog(page, "D1");
    await page.getByRole("button", { name: "Create database" }).first().click();
    await expect(
      page.getByRole("button", { name: "Create", exact: true }),
    ).toBeDisabled();
    await page.getByRole("button", { name: "Cancel" }).click();
    await expect(
      page.getByRole("heading", { name: "D1 databases" }),
    ).toBeVisible();
    await createCatalogResource(page, name, "database");
    await expect(page.getByRole("heading", { name, level: 1 })).toBeVisible();
    await page.getByRole("button", { name: "Console", exact: true }).click();
    await page
      .getByLabel("SQL statement")
      .fill("CREATE TABLE lifecycle_test (id INTEGER PRIMARY KEY);");
    await page.getByRole("button", { name: "Execute" }).click();
    await expect(page.getByText(/"success": true/)).toBeVisible({
      timeout: 15_000,
    });
    await page.getByRole("button", { name: "Time travel" }).click();
    await page.getByRole("button", { name: "Create backup" }).click();
    const backupRow = page.getByRole("row").filter({ hasText: "ready" }).last();
    await backupRow.getByRole("button", { name: "Restore" }).click();
    const restoreDialog = page.getByRole("dialog");
    await restoreDialog.getByLabel("New database name").fill(restoredName);
    await restoreDialog
      .getByRole("button", { name: "Restore", exact: true })
      .click();
    await returnToCatalog(page, "D1");
    await expect(
      page.getByRole("link", { name: restoredName, exact: true }),
    ).toBeVisible({ timeout: 15_000 });
    await deleteCatalogResource(page, name);
    await deleteCatalogResource(page, restoredName);
  });

  test("D1 time travel separates current bookmark from restore confirmation", async ({
    page,
  }) => {
    const name = `pw-d1-travel-${crypto.randomUUID().replaceAll("-", "")}`;
    await openCatalog(page, "D1");
    await createCatalogResource(page, name, "database");
    await page.getByRole("button", { name: "Time travel" }).click();

    await expect(page.getByText("0 retained checkpoints")).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Restore database" }),
    ).toBeDisabled();
    await page.getByRole("button", { name: "Get current bookmark ID" }).click();
    const bookmark = page.getByRole("code");
    await expect(bookmark).not.toBeEmpty();
    await expect(page.getByText("1 retained checkpoints")).toBeVisible();
    await page.getByRole("button", { name: /\d{4}.*\d{2}/ }).click();
    await expect(page.getByLabel("Choose date and time")).not.toBeEmpty();

    await page.getByRole("tab", { name: "Bookmark" }).click();
    await expect(
      page.getByRole("button", { name: "Restore database" }),
    ).toBeDisabled();
    await page
      .getByRole("textbox", { name: "Bookmark ID" })
      .fill((await bookmark.textContent())!);
    await page.getByRole("button", { name: "Restore database" }).click();
    const confirmation = page.getByRole("alertdialog", {
      name: `Restore ${name}?`,
    });
    await expect(
      confirmation.getByText(/Restore target: Bookmark ID/),
    ).toBeVisible();
    await expect(
      confirmation.getByRole("button", { name: "Restore" }),
    ).toBeDisabled();
    await confirmation.getByRole("textbox").fill(name);
    await expect(
      confirmation.getByRole("button", { name: "Restore" }),
    ).toBeEnabled();
    await confirmation.getByRole("button", { name: "Cancel" }).click();

    await page.getByRole("tab", { name: "Date" }).click();
    await expect(
      page.getByRole("button", { name: "Restore database" }),
    ).toBeEnabled();
    await page.getByLabel("Choose date and time").fill("");
    await expect(
      page.getByRole("button", { name: "Restore database" }),
    ).toBeDisabled();
    await returnToCatalog(page, "D1");
    await deleteCatalogResource(page, name);
  });

  test("KV create, rename, value, backup, restore, detail, and deletion stay canonical", async ({
    page,
  }) => {
    const name = `pw-kv-${crypto.randomUUID().replaceAll("-", "")}`;
    const renamedName = `${name}-renamed`;
    const restoredName = `${name}-restored`;
    const key = `profile/${crypto.randomUUID().replaceAll("-", "")}`;
    const value = "live KV value";
    const editedValue = "edited KV value";
    const capture = async (filename: string) => {
      if (process.env.OPEN_COMPUTE_CAPTURE_KV_DETAIL !== "1") return;
      await page
        .getByText(/^KV pair (added|saved)\.$/)
        .first()
        .waitFor({ state: "hidden", timeout: 10_000 });
      await page.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots",
          filename,
        ),
        fullPage: true,
      });
    };
    await openCatalog(page, "KV");
    await createCatalogResource(page, name, "namespace");
    await page.getByRole("link", { name, exact: true }).click();
    await page.getByLabel("Key", { exact: true }).fill(key);
    await page.getByLabel("Value", { exact: true }).fill(value);
    await page.getByRole("button", { name: "Add entry" }).click();
    await expect(
      page.getByRole("cell", { name: key, exact: true }),
    ).toBeVisible({
      timeout: 15_000,
    });
    await expect(
      page.getByRole("cell", { name: value, exact: true }),
    ).toBeVisible();
    await page.setViewportSize({ width: 1808, height: 1113 });
    await capture("kv-pairs-loaded-desktop.png");
    await page.getByRole("button", { name: `Expand ${key}` }).click();
    await expect(
      page.getByRole("textbox", { name: "Selected value" }),
    ).toHaveValue(value);
    await capture("kv-pair-expanded-desktop.png");
    await page.setViewportSize({ width: 390, height: 844 });
    await expect
      .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
      .toBeLessThanOrEqual(390);
    await capture("kv-pair-expanded-mobile.png");
    await page.setViewportSize({ width: 1808, height: 1113 });
    const download = page.waitForEvent("download");
    await page.getByRole("button", { name: "Download", exact: true }).click();
    expect((await download).suggestedFilename()).toBe(
      `${key.split("/").at(-1)}.txt`,
    );
    await page.getByRole("button", { name: "Edit", exact: true }).click();
    await page
      .getByRole("textbox", { name: "Selected value" })
      .fill("discarded value");
    await capture("kv-pair-edit-desktop.png");
    await page.getByLabel("Upload value").setInputFiles({
      name: "value.txt",
      mimeType: "text/plain",
      buffer: Buffer.from("uploaded draft"),
    });
    await expect(
      page.getByRole("textbox", { name: "Selected value" }),
    ).toHaveValue("uploaded draft");
    await page.getByRole("button", { name: "Cancel", exact: true }).click();
    await page.getByRole("button", { name: `Expand ${key}` }).click();
    await page.getByRole("button", { name: "Edit", exact: true }).click();
    await page
      .getByRole("textbox", { name: "Selected value" })
      .fill(editedValue);
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(
      page.getByRole("cell", { name: editedValue, exact: true }),
    ).toBeVisible();
    await page.getByLabel("Search keys by prefix").fill("missing/");
    await expect(page.getByText("No matching keys")).toBeVisible();
    await page.getByLabel("Search keys by prefix").fill("profile/");
    await expect(
      page.getByRole("cell", { name: key, exact: true }),
    ).toBeVisible();
    await page.getByRole("button", { name: `Expand ${key}` }).click();
    await page.getByRole("button", { name: `More actions for ${key}` }).click();
    await expect(
      page.getByRole("button", { name: "Delete", exact: true }),
    ).toBeVisible();
    await capture("kv-pair-menu-desktop.png");
    await page.getByRole("button", { name: "Delete", exact: true }).click();
    const cancelDelete = page.getByRole("alertdialog");
    await expect(cancelDelete).toBeVisible();
    await cancelDelete.getByRole("button", { name: "Cancel" }).click();
    await page.getByRole("button", { name: `Collapse ${key}` }).click();
    await page.setViewportSize({ width: 390, height: 844 });
    await expect
      .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
      .toBeLessThanOrEqual(390);
    await capture("kv-pairs-loaded-mobile.png");
    await page.setViewportSize({ width: 1808, height: 1113 });
    await page.getByRole("button", { name: "Settings", exact: true }).click();
    await capture("kv-settings-desktop.png");
    await page.setViewportSize({ width: 390, height: 844 });
    await expect
      .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
      .toBeLessThanOrEqual(390);
    await capture("kv-settings-mobile.png");
    await page.setViewportSize({ width: 1808, height: 1113 });
    await page.getByRole("button", { name: "Rename" }).click();
    await capture("kv-rename-form-desktop.png");
    await page.getByRole("button", { name: "Cancel", exact: true }).click();
    await page.getByRole("button", { name: "Rename" }).click();
    await page.getByRole("textbox", { name: "Name" }).fill(renamedName);
    await page.getByRole("button", { name: "Save", exact: true }).click();
    await expect(
      page.getByText(renamedName, { exact: true }).last(),
    ).toBeVisible();
    await page.getByRole("button", { name: "Delete", exact: true }).click();
    const namespaceDelete = page.getByRole("alertdialog");
    await expect(namespaceDelete).toBeVisible();
    await capture("kv-namespace-delete-desktop.png");
    await namespaceDelete.getByRole("button", { name: "Cancel" }).click();
    await page.getByRole("button", { name: "Backups", exact: true }).click();
    await page.getByRole("button", { name: "Create backup" }).click();
    const backupRow = page.getByRole("row").filter({ hasText: "ready" }).last();
    await backupRow.getByRole("button", { name: "Restore" }).click();
    const restoreDialog = page.getByRole("dialog");
    await restoreDialog.getByLabel("New namespace name").fill(restoredName);
    await restoreDialog
      .getByRole("button", { name: "Restore", exact: true })
      .click();
    await page.getByRole("button", { name: "KV pairs" }).click();
    const keyRow = page.getByRole("row").filter({ hasText: key });
    await keyRow
      .getByRole("button", { name: `More actions for ${key}` })
      .click();
    await page.getByRole("button", { name: "Delete", exact: true }).click();
    const deleteKeyDialog = page.getByRole("alertdialog");
    await deleteKeyDialog.getByRole("textbox").fill(key);
    await deleteKeyDialog
      .getByRole("button", { name: "Delete", exact: true })
      .click();
    await expect(
      page.getByRole("cell", { name: key, exact: true }),
    ).toHaveCount(0);
    await returnToCatalog(page, "KV");
    for (const target of [renamedName, restoredName]) {
      await page.getByRole("button", { name: `Actions for ${target}` }).click();
      await page.getByRole("menuitem", { name: "Delete" }).click();
      const dialog = page.getByRole("alertdialog");
      await dialog.getByLabel("Resource name").fill(target);
      await dialog.getByRole("button", { name: "Delete", exact: true }).click();
      await expect(
        page.getByRole("link", { name: target, exact: true }),
      ).toHaveCount(0);
    }
  });

  test("R2 create, populated detail, and deletion use the packaged API contract", async ({
    page,
  }) => {
    const name = `pw-r2-${crypto.randomUUID().replaceAll("-", "")}`;
    await openCatalog(page, "R2");
    await page.getByRole("button", { name: "Create bucket" }).first().click();
    await expect(
      page.getByRole("heading", { name: "Create a bucket" }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Create bucket" }),
    ).toBeDisabled();
    await page.getByRole("button", { name: "Cancel" }).click();
    await expect(page).toHaveURL(/\/r2\/?$/);
    await createCatalogResource(page, name, "bucket");
    await expect(page.getByRole("heading", { name, level: 1 })).toBeVisible();
    await expect(
      page.getByText("Default storage class", { exact: true }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Add folder" }).click();
    const folderDialog = page.getByRole("dialog", { name: "Add folder" });
    await folderDialog.getByLabel("Folder name").fill("audit-folder");
    await folderDialog.getByRole("button", { name: "Add folder" }).click();
    await expect(
      page.getByRole("row").filter({ hasText: "audit-folder/" }),
    ).toBeVisible();
    await page
      .getByRole("row")
      .filter({ hasText: "audit-folder/" })
      .getByRole("button", { name: "audit-folder/", exact: true })
      .click();
    await expect(page).toHaveURL(/prefix=audit-folder%2F/);
    await expectNoLoadErrors(page);
    await liveClient().r2.buckets.objects.delete("audit-folder/", {
      account_id: await accountId(liveClient()),
      bucket_name: name,
    });
    await page.goto(`./r2/${name}`);
    await page.getByRole("button", { name: "Settings" }).click();
    await page.getByRole("button", { name: "Delete", exact: true }).click();
    const dialog = page.getByRole("alertdialog");
    await dialog.getByRole("textbox").fill(name);
    await dialog.getByRole("button", { name: "Delete", exact: true }).click();
    await expect(page).toHaveURL(/\/r2\/?$/);
  });

  test("R2 file row opens a directly reloadable object detail page", async ({
    page,
  }) => {
    const client = liveClient();
    const account = await accountId(client);
    const bucket = `pw-r2-detail-${crypto.randomUUID().replaceAll("-", "")}`;
    const key = "docs/readme.txt";
    let deleted = false;
    await client.r2.buckets.create({ account_id: account, name: bucket });
    try {
      await page.goto(`./r2/${bucket}`);
      await page.getByRole("button", { name: "Add folder" }).click();
      const folderDialog = page.getByRole("dialog", { name: "Add folder" });
      await folderDialog.getByLabel("Folder name").fill("docs");
      await folderDialog.getByRole("button", { name: "Add folder" }).click();
      await page
        .getByRole("row")
        .filter({ hasText: "docs/" })
        .getByRole("button", { name: "docs/", exact: true })
        .click();
      await page
        .getByRole("dialog", { name: "Folder added." })
        .waitFor({ state: "hidden" });
      await page.getByRole("button", { name: "Upload", exact: true }).click();
      if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VISUAL === "1") {
        const output = resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots",
        );
        await page.setViewportSize({ width: 1440, height: 900 });
        await page.screenshot({
          path: resolve(output, "r2-upload-open-after-desktop.png"),
          fullPage: true,
        });
        await page.setViewportSize({ width: 390, height: 844 });
        await page.screenshot({
          path: resolve(output, "r2-upload-open-after-mobile.png"),
          fullPage: true,
        });
        await page.setViewportSize({ width: 1440, height: 900 });
      }
      await page.getByLabel("Choose files to upload").setInputFiles({
        name: "readme.txt",
        mimeType: "text/plain",
        buffer: Buffer.from("R2 detail probe"),
      });
      await expect(
        page.getByText("All files uploaded successfully."),
      ).toBeVisible();
      await expect(page.getByText("1/1 files uploaded")).toBeVisible();
      if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VISUAL === "1") {
        const output = resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots",
        );
        await page.screenshot({
          path: resolve(output, "r2-upload-complete-after-desktop.png"),
          fullPage: true,
        });
        await page.setViewportSize({ width: 390, height: 844 });
        await expect
          .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
          .toBeLessThanOrEqual(390);
        await page.screenshot({
          path: resolve(output, "r2-upload-complete-after-mobile.png"),
          fullPage: true,
        });
        await page.setViewportSize({ width: 1440, height: 900 });
      }
      await page.getByRole("button", { name: "Close", exact: true }).click();
      await page
        .getByRole("row")
        .filter({ hasText: "readme.txt" })
        .getByRole("button", { name: "readme.txt", exact: true })
        .click();
      await expect(page).toHaveURL(/\/objects\/docs%2Freadme\.txt\/details$/);
      await expect(
        page.getByRole("heading", { name: key, level: 1 }),
      ).toBeVisible();
      await expect(page.getByText("R2 detail probe")).toBeVisible();
      if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VISUAL === "1") {
        const output = resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots",
        );
        await page.setViewportSize({ width: 1440, height: 900 });
        await page.screenshot({
          path: resolve(output, "r2-object-detail-after-desktop.png"),
          fullPage: true,
        });
        await page.setViewportSize({ width: 390, height: 844 });
        await expect
          .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
          .toBeLessThanOrEqual(390);
        await page.screenshot({
          path: resolve(output, "r2-object-detail-after-mobile.png"),
          fullPage: true,
        });
      }
      await page.reload();
      await expect(
        page.getByRole("heading", { name: key, level: 1 }),
      ).toBeVisible();
      await expectNoLoadErrors(page);
      const downloadEvent = page.waitForEvent("download");
      await page.getByRole("button", { name: "Download" }).click();
      expect((await downloadEvent).suggestedFilename()).toBe("readme.txt");
      await page.getByRole("button", { name: "Delete", exact: true }).click();
      const deleteDialog = page.getByRole("alertdialog");
      await deleteDialog.getByLabel("Resource name").fill(key);
      await deleteDialog.getByRole("button", { name: "Delete" }).click();
      await expect(page).toHaveURL(new RegExp(`/r2/${bucket}(?:\\?prefix=)?$`));
      deleted = true;
    } finally {
      if (!deleted)
        await client.r2.buckets.objects
          .delete(key, {
            account_id: account,
            bucket_name: bucket,
          })
          .catch(() => undefined);
      await client.r2.buckets.objects
        .delete("docs/", { account_id: account, bucket_name: bucket })
        .catch(() => undefined);
      await client.r2.buckets.delete(bucket, { account_id: account });
    }
  });

  test("R2 Settings permits deletion only for an empty bucket", async ({
    page,
  }) => {
    const client = liveClient();
    const account = await accountId(client);
    const filled = `pw-r2-filled-${crypto.randomUUID().replaceAll("-", "")}`;
    const empty = `pw-r2-empty-${crypto.randomUUID().replaceAll("-", "")}`;
    await client.r2.buckets.create({ account_id: account, name: filled });
    await client.r2.buckets.create({ account_id: account, name: empty });
    let emptyDeleted = false;
    try {
      await client.r2.buckets.objects.upload(
        "keep.txt",
        new Blob(["keep"], { type: "text/plain" }),
        { account_id: account, bucket_name: filled },
      );
      await page.goto(`./r2/${filled}`);
      await page.getByRole("button", { name: "Settings" }).click();
      await expect(
        page.getByRole("heading", { name: "General" }),
      ).toBeVisible();
      await expect(
        page.getByRole("heading", { name: "Default storage class" }),
      ).toBeVisible();
      await expect(page.getByText("Standard", { exact: true })).toBeVisible();
      const sectionNav = page.getByRole("navigation", {
        name: "Bucket settings sections",
      });
      await sectionNav
        .getByRole("link", { name: "Default storage class" })
        .click();
      await expect(
        sectionNav.getByRole("link", { name: "Default storage class" }),
      ).toHaveAttribute("aria-current", "location");
      await expect(
        page.getByRole("button", { name: "Delete", exact: true }),
      ).toBeDisabled();
      await page.goto(`./r2/${empty}`);
      await page.getByRole("button", { name: "Settings" }).click();
      const deleteButton = page.getByRole("button", {
        name: "Delete",
        exact: true,
      });
      await expect(deleteButton).toBeEnabled();
      await deleteButton.click();
      const dialog = page.getByRole("alertdialog");
      await expect(
        dialog.getByRole("button", { name: "Delete" }),
      ).toBeDisabled();
      await dialog.getByRole("button", { name: "Cancel" }).click();
      await expect(dialog).toBeHidden();
      await deleteButton.click();
      await dialog.getByRole("textbox").fill(empty);
      await dialog.getByRole("button", { name: "Delete" }).click();
      await expect(page).toHaveURL(/\/r2\/?$/);
      emptyDeleted = true;
      await expectNoLoadErrors(page);
    } finally {
      await client.r2.buckets.objects
        .delete("keep.txt", { account_id: account, bucket_name: filled })
        .catch(() => undefined);
      await client.r2.buckets.delete(filled, { account_id: account });
      if (!emptyDeleted)
        await client.r2.buckets.delete(empty, { account_id: account });
    }
  });

  test("R2 overview filters and paginates the supported bucket table", async ({
    page,
  }) => {
    const client = liveClient();
    const account = await accountId(client);
    const prefix = `pw-r2-list-${crypto.randomUUID().replaceAll("-", "")}`;
    const names = Array.from(
      { length: 11 },
      (_, index) => `${prefix}-${String(index).padStart(2, "0")}`,
    );
    try {
      for (const name of names) {
        await client.r2.buckets.create({ account_id: account, name });
      }
      await client.r2.buckets.objects.upload(
        "sample.txt",
        new Blob(["data"], { type: "text/plain" }),
        { account_id: account, bucket_name: names[4]! },
      );
      await page.goto("./r2");
      const search = page.getByRole("textbox", { name: "Search buckets" });
      const filtered = page.waitForResponse((response) => {
        const url = new URL(response.url());
        return (
          url.pathname.endsWith("/r2/buckets") &&
          url.searchParams.get("name_contains") === prefix &&
          response.status() === 200
        );
      });
      await search.fill(prefix);
      await filtered;
      const table = page.getByRole("table");
      await expect(table.getByRole("columnheader")).toHaveText([
        "Bucket",
        "Objects",
        "Size",
      ]);
      await expect(table.getByRole("row")).toHaveCount(11);
      await expect(table.getByRole("link", { name: names[0] })).toBeVisible();
      await expect(
        table.getByRole("row").nth(1).getByRole("cell").nth(1),
      ).toHaveText("0");
      await expect(
        table.getByRole("row").nth(1).getByRole("cell").nth(2),
      ).toHaveText("0 B");
      if (process.env.OPEN_COMPUTE_CAPTURE_R2_USAGE === "1") {
        const output = resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots",
        );
        await page.setViewportSize({ width: 1808, height: 1113 });
        await page.screenshot({
          path: resolve(output, "r2-overview-usage-desktop.png"),
          fullPage: true,
        });
        await page.setViewportSize({ width: 390, height: 844 });
        await page.screenshot({
          path: resolve(output, "r2-overview-usage-mobile.png"),
          fullPage: true,
        });
        await page.setViewportSize({ width: 1280, height: 720 });
      }
      await expect(
        page.getByRole("button", { name: "Previous page" }),
      ).toBeDisabled();
      await page.getByRole("button", { name: "Next page" }).click();
      await expect(table.getByRole("row")).toHaveCount(2);
      await expect(table.getByRole("link", { name: names[10] })).toBeVisible();
      await expect(
        page.getByRole("button", { name: "Next page" }),
      ).toBeDisabled();
      await page.getByRole("button", { name: "Previous page" }).click();
      await expect(table.getByRole("row")).toHaveCount(11);
      await search.fill(names[4]!);
      await expect(table.getByRole("row")).toHaveCount(2);
      await expect(
        table.getByRole("row").nth(1).getByRole("cell").nth(1),
      ).toHaveText("1");
      await expect(
        table.getByRole("row").nth(1).getByRole("cell").nth(2),
      ).toHaveText("4 B");
      await expect(
        page.getByRole("button", { name: "Delete", exact: true }),
      ).toHaveCount(0);
      await page.setViewportSize({ width: 390, height: 844 });
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
        .toBeLessThanOrEqual(390);
      expect(
        await table.evaluate(
          (element) =>
            element.parentElement!.scrollWidth >
            element.parentElement!.clientWidth,
        ),
      ).toBe(true);
      await table.getByRole("link", { name: names[4] }).click();
      await expect(page).toHaveURL(new RegExp(`/r2/${names[4]}$`));
      await expectNoLoadErrors(page);
    } finally {
      await client.r2.buckets.objects
        .delete("sample.txt", { account_id: account, bucket_name: names[4]! })
        .catch(() => undefined);
      for (const name of names) {
        await client.r2.buckets
          .delete(name, { account_id: account })
          .catch(() => undefined);
      }
    }
  });

  test("R2 folder picker keeps same-named files in separate paths", async ({
    page,
  }) => {
    const client = liveClient();
    const account = await accountId(client);
    const bucket = `pw-r2-folder-${crypto.randomUUID().replaceAll("-", "")}`;
    const keys = [
      "r2-upload-folder/docs/same.txt",
      "r2-upload-folder/other/same.txt",
    ];
    await client.r2.buckets.create({ account_id: account, name: bucket });
    try {
      await page.goto(`./r2/${bucket}`);
      await page.getByRole("button", { name: "Upload", exact: true }).click();
      await page
        .getByLabel("Choose folder to upload")
        .setInputFiles(
          resolve(import.meta.dirname, "../fixtures/r2-upload-folder"),
        );
      await expect(page.getByText("2/2 files uploaded")).toBeVisible();
      await expect(
        page.getByText("All files uploaded successfully."),
      ).toBeVisible();
      const listed = await client.r2.buckets.objects.list(bucket, {
        account_id: account,
      });
      expect(listed.result.map((object) => object.key).sort()).toEqual(keys);
      await expectNoLoadErrors(page);
    } finally {
      for (const key of keys) {
        await client.r2.buckets.objects
          .delete(key, { account_id: account, bucket_name: bucket })
          .catch(() => undefined);
      }
      await client.r2.buckets.delete(bucket, { account_id: account });
    }
  });

  test("R2 selection deletes every object in a selected folder without touching other keys", async ({
    page,
  }) => {
    const client = liveClient();
    const account = await accountId(client);
    const bucket = `pw-r2-bulk-${crypto.randomUUID().replaceAll("-", "")}`;
    const keys = ["audit-folder/", "audit-folder/deep/note.txt", "keep.txt"];
    await client.r2.buckets.create({ account_id: account, name: bucket });
    try {
      for (const key of keys) {
        await client.r2.buckets.objects.upload(
          key,
          new Blob([key.endsWith("/") ? "" : key], { type: "text/plain" }),
          { account_id: account, bucket_name: bucket },
          { headers: { "Content-Type": "text/plain" } },
        );
      }
      await page.goto(`./r2/${bucket}`);
      await expect(
        page.getByRole("row").filter({ hasText: "audit-folder/" }),
      ).toBeVisible();
      await page
        .getByRole("checkbox", { name: "Select all listed objects" })
        .check();
      if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VISUAL === "1") {
        const output = resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots",
        );
        await page.setViewportSize({ width: 1440, height: 900 });
        await page.screenshot({
          path: resolve(output, "r2-objects-selection-after-desktop.png"),
          fullPage: true,
        });
        await page.setViewportSize({ width: 390, height: 844 });
        await expect
          .poll(() =>
            page
              .locator("main")
              .first()
              .evaluate((element) => element.getBoundingClientRect().width),
          )
          .toBeGreaterThan(300);
        expect(
          await page.evaluate(() => document.documentElement.scrollWidth),
        ).toBeLessThanOrEqual(390);
        await page.screenshot({
          path: resolve(output, "r2-objects-selection-after-mobile.png"),
          fullPage: true,
        });
        await page.setViewportSize({ width: 1440, height: 900 });
      }
      await page
        .getByRole("button", { name: "Delete 1 folder and 1 file" })
        .click();
      const mixedDialog = page.getByRole("alertdialog");
      await expect(
        mixedDialog.getByLabel("Type delete to confirm"),
      ).toBeVisible();
      if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VISUAL === "1") {
        await page.screenshot({
          path: resolve(
            import.meta.dirname,
            "../../../../dashboard-refactor/implementation-screenshots/r2-bulk-delete-dialog-after-desktop.png",
          ),
          fullPage: true,
        });
      }
      await mixedDialog.getByRole("button", { name: "Cancel" }).click();
      await expect(
        page.getByRole("button", { name: "Delete 1 folder and 1 file" }),
      ).toBeVisible();
      await page.getByRole("checkbox", { name: "Select keep.txt" }).uncheck();
      await page.getByRole("button", { name: "Delete 1 folder" }).click();
      const folderDialog = page.getByRole("alertdialog");
      await folderDialog.getByLabel("Type delete to confirm").fill("delete");
      let blockedLists = 0;
      const folderListPattern = /\/r2\/buckets\/[^/]+\/objects\?/;
      await page.route(folderListPattern, (route) => {
        const url = new URL(route.request().url());
        if (url.searchParams.get("prefix") === "audit-folder/") {
          blockedLists += 1;
          return route.abort();
        }
        return route.continue();
      });
      await folderDialog
        .getByRole("button", { name: "Delete", exact: true })
        .click();
      await expect.poll(() => blockedLists).toBe(1);
      await expect(folderDialog).toBeVisible();
      await expect(
        folderDialog.getByRole("button", { name: "Delete", exact: true }),
      ).toBeEnabled();
      const beforeRetry = await client.r2.buckets.objects.list(bucket, {
        account_id: account,
      });
      expect(beforeRetry.result.map((object) => object.key)).toEqual(keys);
      await page.unroute(folderListPattern);
      await folderDialog
        .getByRole("button", { name: "Delete", exact: true })
        .click();
      await expect(
        page.getByRole("row").filter({ hasText: "audit-folder/" }),
      ).toHaveCount(0);
      await expect(
        page.getByRole("row").filter({ hasText: "keep.txt" }),
      ).toBeVisible();
      const remaining = await client.r2.buckets.objects.list(bucket, {
        account_id: account,
      });
      expect(remaining.result.map((object) => object.key)).toEqual([
        "keep.txt",
      ]);
      await page.getByRole("checkbox", { name: "Select keep.txt" }).check();
      await page.getByRole("button", { name: "Delete 1 file" }).click();
      const fileDialog = page.getByRole("alertdialog");
      await expect(fileDialog.getByLabel("Type delete to confirm")).toHaveCount(
        0,
      );
      await fileDialog
        .getByRole("button", { name: "Delete", exact: true })
        .click();
      await expect(
        page.getByRole("row").filter({ hasText: "keep.txt" }),
      ).toHaveCount(0);
    } finally {
      for (const key of keys) {
        await client.r2.buckets.objects
          .delete(key, { account_id: account, bucket_name: bucket })
          .catch(() => undefined);
      }
      await client.r2.buckets.delete(bucket, { account_id: account });
    }
  });

  test("Queue inline Worker consumer saves seconds through the millisecond API", async ({
    page,
  }) => {
    const client = liveClient();
    const account = await accountId(client);
    const workerName = `pw-queue-worker-${crypto.randomUUID().replaceAll("-", "")}`;
    const queueName = `pw-queue-consumer-${crypto.randomUUID().replaceAll("-", "")}`;
    let queueId: string | undefined;
    try {
      await client.workers.scripts.update(workerName, {
        account_id: account,
        metadata: {
          main_module: "index.js",
          compatibility_date: "2026-09-08",
        },
        files: [
          new File(
            [
              "export default { fetch() { return new Response('ok'); }, async queue(batch) { for (const message of batch.messages) message.ack(); } };",
            ],
            "index.js",
            { type: "application/javascript+module" },
          ),
        ],
      });
      const queue = await client.queues.create({
        account_id: account,
        queue_name: queueName,
      });
      queueId = queue.queue_id;
      if (!queueId) throw new Error("Created Queue has no ID");
      await page.goto(`./queues/${queueId}`);
      await page.getByRole("tab", { name: "Settings" }).click();
      await page.getByRole("button", { name: "Add", exact: true }).click();
      await expect(
        page.getByRole("heading", { name: "Add consumer" }),
      ).toBeVisible();
      if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VISUAL === "1") {
        await page.setViewportSize({ width: 1440, height: 900 });
        await page.screenshot({
          path: resolve(
            import.meta.dirname,
            "../../../../dashboard-refactor/implementation-screenshots/queue-add-consumer-after-desktop.png",
          ),
          fullPage: true,
        });
      }
      await page.getByRole("combobox", { name: "Worker" }).click();
      await page.getByRole("option", { name: workerName }).click();
      await page.getByLabel("Max wait (seconds)").fill("7");
      await page
        .getByRole("button", { name: "Add", exact: true })
        .last()
        .click();
      await expect(
        page.getByRole("row").filter({ hasText: workerName }),
      ).toBeVisible();
      const created = await client.queues.consumers.list(queueId, {
        account_id: account,
      });
      expect(created.result).toHaveLength(1);
      expect(created.result[0]?.settings?.max_wait_time_ms).toBe(7000);
      await expectNoLoadErrors(page);
      const row = page.getByRole("row").filter({ hasText: workerName });
      await row
        .getByRole("button", { name: `Actions for ${workerName}` })
        .click();
      await page.getByRole("menuitem", { name: "Edit consumer" }).click();
      await page.getByLabel("Max wait (seconds)").fill("8");
      await page.getByRole("button", { name: "Save", exact: true }).click();
      await expect(
        page.getByRole("heading", { name: "Edit consumer" }),
      ).toBeHidden();
      const updated = await client.queues.consumers.list(queueId, {
        account_id: account,
      });
      expect(updated.result[0]?.settings?.max_wait_time_ms).toBe(8000);
      await row
        .getByRole("button", { name: `Actions for ${workerName}` })
        .click();
      await page.getByRole("menuitem", { name: "Delete consumer" }).click();
      const dialog = page.getByRole("alertdialog");
      await dialog.getByRole("textbox").fill(workerName);
      await dialog.getByRole("button", { name: "Delete" }).click();
      await expect(row).toHaveCount(0);
    } finally {
      if (queueId) {
        const consumers = await client.queues.consumers.list(queueId, {
          account_id: account,
        });
        for (const consumer of consumers.result) {
          if (consumer.consumer_id) {
            await client.queues.consumers.delete(consumer.consumer_id, {
              account_id: account,
              queue_id: queueId,
            });
          }
        }
        await client.queues.delete(queueId, { account_id: account });
      }
      await client.workers.scripts
        .delete(workerName, { account_id: account })
        .catch(() => undefined);
    }
  });

  test("Queue and Workflow create, update, detail, and deletion stay live", async ({
    page,
  }) => {
    const queueName = `pw-queue-${crypto.randomUUID().replaceAll("-", "")}`;
    const renamedQueueName = `${queueName}-renamed`;
    const client = liveClient();
    const accountID = await accountId(client);
    try {
      await openCatalog(page, "Queues");
      await page
        .getByRole("button", { name: "Create queue", exact: true })
        .first()
        .click();
      await expect(page).toHaveURL(/\/queues\/new$/);
      await expect(
        page.getByRole("button", { name: "Create", exact: true }),
      ).toBeDisabled();
      await page.getByRole("link", { name: "Back" }).click();
      await expect(page).toHaveURL(/\/queues\/?$/);
      await page
        .getByRole("button", { name: "Create queue", exact: true })
        .first()
        .click();
      await page.getByRole("textbox", { name: "Name" }).fill(queueName);
      await page.getByRole("button", { name: "Create", exact: true }).click();
      await expect(page).toHaveURL(/\/queues\/[^/]+$/);
      await expectNoLoadErrors(page);
      await page.getByRole("tab", { name: "Settings" }).click();
      await expect(
        page.getByRole("heading", { name: "General" }),
      ).toBeVisible();
      if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VISUAL === "1") {
        const output = resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots",
        );
        await page.setViewportSize({ width: 1440, height: 900 });
        await page.screenshot({
          path: resolve(output, "queue-settings-after-desktop.png"),
          fullPage: true,
        });
        await page.setViewportSize({ width: 390, height: 844 });
        await expect
          .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
          .toBeLessThanOrEqual(390);
        await page.screenshot({
          path: resolve(output, "queue-settings-after-mobile.png"),
          fullPage: true,
        });
        await page.setViewportSize({ width: 1440, height: 900 });
      }
      await page
        .getByRole("button", { name: "Edit message retention" })
        .click();
      if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VISUAL === "1") {
        await page.screenshot({
          path: resolve(
            import.meta.dirname,
            "../../../../dashboard-refactor/implementation-screenshots/queue-edit-retention-after-desktop.png",
          ),
          fullPage: true,
        });
      }
      await expect(
        page.getByRole("button", { name: "Save", exact: true }),
      ).toBeDisabled();
      await page.getByLabel("Message retention (seconds)").fill("59");
      await expect(
        page.getByRole("button", { name: "Save", exact: true }),
      ).toBeDisabled();
      await page.getByLabel("Message retention (seconds)").fill("7200");
      const queueUpdate = page.waitForRequest(
        (request) =>
          request.method() === "PUT" &&
          /\/queues\/[^/]+$/.test(new URL(request.url()).pathname),
      );
      await page.getByRole("button", { name: "Save", exact: true }).click();
      expect(JSON.parse((await queueUpdate).postData() ?? "{}")).toEqual({
        queue_name: queueName,
        settings: {
          delivery_delay: 0,
          delivery_paused: false,
          message_retention_period: 7200,
        },
      });
      await expect(page.getByText("7200 seconds")).toBeVisible();
      await page.getByRole("button", { name: "Edit delivery delay" }).click();
      if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VISUAL === "1") {
        await page.screenshot({
          path: resolve(
            import.meta.dirname,
            "../../../../dashboard-refactor/implementation-screenshots/queue-edit-delay-after-desktop.png",
          ),
          fullPage: true,
        });
      }
      await page.getByLabel("Delivery delay (seconds)").fill("1");
      await page.getByRole("button", { name: "Cancel", exact: true }).click();
      await expect(page.getByText("0 seconds", { exact: true })).toBeVisible();
      await page.getByRole("button", { name: "Rename queue" }).click();
      const renameQueueDialog = page.getByRole("dialog", {
        name: "Rename queue",
      });
      await renameQueueDialog.getByLabel("Queue name").fill(renamedQueueName);
      await renameQueueDialog
        .getByRole("button", { name: "Save name" })
        .click();
      await expect(
        page.getByRole("heading", { name: renamedQueueName }),
      ).toBeVisible();
      await returnToCatalog(page, "Queues");
      await page
        .getByRole("button", { name: `Actions for ${renamedQueueName}` })
        .click();
      await page.getByRole("menuitem", { name: "Delete queue" }).click();
      await page
        .getByRole("alertdialog")
        .getByRole("textbox")
        .fill(renamedQueueName);
      await page
        .getByRole("alertdialog")
        .getByRole("button", { name: "Delete" })
        .click();
      await expect(
        page.getByRole("link", { name: new RegExp(renamedQueueName) }),
      ).toHaveCount(0);
    } finally {
      for await (const queue of client.queues.list({ account_id: accountID })) {
        if (
          (queue.queue_name === queueName ||
            queue.queue_name === renamedQueueName) &&
          queue.queue_id
        ) {
          await client.queues.delete(queue.queue_id, { account_id: accountID });
        }
      }
    }

    const scriptName = `pw-workflow-worker-${crypto.randomUUID().replaceAll("-", "")}`;
    const workflowName = `pw-workflow-${crypto.randomUUID().replaceAll("-", "")}`;
    await client.workers.scripts.update(scriptName, {
      account_id: accountID,
      metadata: {
        main_module: "index.js",
        compatibility_date: "2026-09-08",
      },
      files: [
        new File(
          [
            "import { WorkflowEntrypoint } from 'cloudflare:workers'; export class Flow extends WorkflowEntrypoint { async run() { return { ok: true }; } } export default { fetch() { return new Response('workflow dashboard e2e'); } };",
          ],
          "index.js",
          { type: "application/javascript+module" },
        ),
      ],
    });
    try {
      await openCatalog(page, "Workflows");
      await page
        .getByRole("button", { name: "Create workflow", exact: true })
        .first()
        .click();
      await expect(page).toHaveURL(/\/workflows\/new$/);
      await page
        .getByRole("button", { name: "Use an existing Worker class" })
        .click();
      await page.getByRole("button", { name: "Back" }).click();
      await expect(
        page.getByRole("button", { name: "Use an existing Worker class" }),
      ).toBeVisible();
      await page
        .getByRole("button", { name: "Close workflow creation" })
        .click();
      await expect(page).toHaveURL(/\/workflows\/?$/);
      await page
        .getByRole("button", { name: "Create workflow", exact: true })
        .first()
        .click();
      await page
        .getByRole("button", { name: "Use an existing Worker class" })
        .click();
      await expect(
        page.getByRole("button", { name: "Create workflow" }),
      ).toBeDisabled();
      await page.getByLabel("Workflow name").fill(workflowName);
      await page.getByLabel("Worker script").fill(scriptName);
      await page.getByLabel("Exported class").fill("Flow");
      await page.getByRole("button", { name: "Create workflow" }).click();
      await expect(page).toHaveURL(new RegExp(`/workflows/${workflowName}$`), {
        timeout: 30_000,
      });
      await expect(
        page.getByRole("heading", { name: workflowName }),
      ).toBeVisible();
      await page.getByRole("tab", { name: "Settings" }).click();
      await page.getByRole("button", { name: "Edit definition" }).click();
      await page
        .getByRole("dialog")
        .getByRole("button", { name: "Save definition" })
        .click();
      await expect(
        page.getByRole("dialog", { name: "Edit workflow definition" }),
      ).toBeHidden();
      await returnToCatalog(page, "Workflows");
      await page
        .getByRole("button", { name: `Actions for ${workflowName}` })
        .click();
      await page.getByRole("menuitem", { name: "Delete workflow" }).click();
      await page
        .getByRole("alertdialog")
        .getByRole("textbox")
        .fill(workflowName);
      await page
        .getByRole("alertdialog")
        .getByRole("button", { name: "Delete" })
        .click();
      await expect(
        page.getByRole("link", { name: new RegExp(workflowName) }),
      ).toHaveCount(0);
    } finally {
      await client.workers.scripts.delete(scriptName, {
        account_id: accountID,
      });
    }
  });
});
