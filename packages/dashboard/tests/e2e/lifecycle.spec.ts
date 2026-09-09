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
    .getByRole("link", { name, exact: true })
    .click();
}

async function returnToCatalog(page: Page, name: string) {
  await page
    .getByRole("navigation", { name: "Breadcrumb" })
    .getByRole("link", { name, exact: true })
    .click();
}

async function createCatalogResource(page: Page, name: string) {
  await page.getByRole("button", { name: "Create", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Name", { exact: true }).fill(name);
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
    .getByRole("button", { name: `Actions for ${currentName}`, exact: true })
    .click();
  await page.getByRole("menuitem", { name: "Rename" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("textbox").fill(newName);
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(
    page.getByRole("cell", { name: newName, exact: true }),
  ).toBeVisible({ timeout: 15_000 });
}

async function deleteCatalogResource(page: Page, name: string) {
  await page
    .getByRole("button", { name: `Actions for ${name}`, exact: true })
    .click();
  await page.getByRole("menuitem", { name: "Delete" }).click();
  const dialog = page.getByRole("alertdialog");
  await dialog.getByRole("textbox").fill(name);
  await dialog.getByRole("button", { name: "Delete", exact: true }).click();
  await expect(page.getByRole("cell", { name, exact: true })).toHaveCount(0);
}

test.describe("operator dashboard live lifecycle", () => {
  test.setTimeout(120_000);

  test.beforeEach(async ({ page }) => {
    await signIn(page);
  });

  test("D1 create, query, backup, restore, detail, and deletion stay canonical", async ({
    page,
  }) => {
    const name = `pw-d1-${crypto.randomUUID().replaceAll("-", "")}`;
    const restoredName = `${name}-restored`;
    await openCatalog(page, "D1");
    await createCatalogResource(page, name);
    await page.getByRole("link", { name, exact: true }).click();
    await expect(page.getByRole("heading", { name, level: 1 })).toBeVisible();
    await page
      .getByLabel("SQL query")
      .fill("CREATE TABLE lifecycle_test (id INTEGER PRIMARY KEY);");
    await page.getByRole("button", { name: "Run query" }).click();
    await expect(page.getByText(/lifecycle_test|success/i)).toBeVisible({
      timeout: 15_000,
    });
    await page.getByRole("button", { name: "Create backup" }).click();
    await expect(
      page.getByText("D1 backup created.", { exact: true }),
    ).toBeVisible({ timeout: 15_000 });
    const backupRow = page.getByRole("row").filter({ hasText: "ready" }).last();
    await backupRow.getByRole("button", { name: "Restore" }).click();
    const restoreDialog = page.getByRole("dialog");
    await restoreDialog.getByLabel("New database name").fill(restoredName);
    await restoreDialog.getByRole("button", { name: "Restore backup" }).click();
    await expect(
      page.getByText(`D1 backup restored as ${restoredName}.`, { exact: true }),
    ).toBeVisible({ timeout: 15_000 });
    await returnToCatalog(page, "D1");
    await expect(
      page.getByRole("cell", { name: restoredName, exact: true }),
    ).toBeVisible({ timeout: 15_000 });
    await deleteCatalogResource(page, name);
    await deleteCatalogResource(page, restoredName);
  });

  test("KV create, rename, value, backup, restore, detail, and deletion stay canonical", async ({
    page,
  }) => {
    const name = `pw-kv-${crypto.randomUUID().replaceAll("-", "")}`;
    const renamedName = `${name}-renamed`;
    const restoredName = `${name}-restored`;
    const key = `profile/${crypto.randomUUID().replaceAll("-", "")}`;
    const value = "live KV value";
    await openCatalog(page, "KV");
    await createCatalogResource(page, name);
    await renameCatalogResource(page, name, renamedName);
    await page.getByRole("link", { name: renamedName, exact: true }).click();
    await page.getByLabel("Key", { exact: true }).fill(key);
    await page.getByLabel("Value", { exact: true }).fill(value);
    await page.getByLabel("JSON metadata (optional)").fill('{"region":"test"}');
    await page
      .getByLabel("Expiration TTL seconds (optional, minimum 60)")
      .fill("120");
    await page.getByRole("button", { name: "Save value" }).click();
    await expect(page.getByText(value, { exact: true })).toBeVisible({
      timeout: 15_000,
    });
    await page.getByRole("button", { name: "Create backup" }).click();
    await expect(
      page.getByText("KV backup created.", { exact: true }),
    ).toBeVisible({ timeout: 15_000 });
    const backupRow = page.getByRole("row").filter({ hasText: "ready" }).last();
    await backupRow.getByRole("button", { name: "Restore" }).click();
    const restoreDialog = page.getByRole("dialog");
    await restoreDialog.getByLabel("New namespace name").fill(restoredName);
    await restoreDialog.getByRole("button", { name: "Restore backup" }).click();
    await expect(
      page.getByText(`KV backup restored as ${restoredName}.`, { exact: true }),
    ).toBeVisible({ timeout: 15_000 });
    const keyRow = page.getByRole("row").filter({ hasText: key });
    await keyRow.getByRole("button", { name: "Delete", exact: true }).click();
    const deleteKeyDialog = page.getByRole("alertdialog");
    await deleteKeyDialog.getByRole("textbox").fill(key);
    await deleteKeyDialog.getByRole("button", { name: "Delete key" }).click();
    await expect(
      page.getByRole("cell", { name: key, exact: true }),
    ).toHaveCount(0);
    await returnToCatalog(page, "KV");
    await deleteCatalogResource(page, renamedName);
    await deleteCatalogResource(page, restoredName);
  });

  test("R2 create, populated detail, and deletion use the packaged API contract", async ({
    page,
  }) => {
    const name = `pw-r2-${crypto.randomUUID().replaceAll("-", "")}`;
    await openCatalog(page, "R2");
    await createCatalogResource(page, name);
    await page.getByRole("link", { name, exact: true }).click();
    await expect(page.getByRole("heading", { name, level: 1 })).toBeVisible();
    await expect(
      page.getByRole("cell", { name: "Storage class" }),
    ).toBeVisible();
    await expectNoLoadErrors(page);
    await returnToCatalog(page, "R2");
    await deleteCatalogResource(page, name);
  });

  test("Queue and Workflow create, update, detail, and deletion stay live", async ({
    page,
  }) => {
    const queueName = `pw-queue-${crypto.randomUUID().replaceAll("-", "")}`;
    const renamedQueueName = `${queueName}-renamed`;
    await openCatalog(page, "Queues");
    await page.getByRole("button", { name: "Create Queue" }).click();
    const queueDialog = page.getByRole("dialog");
    await queueDialog.getByLabel("Queue name").fill(queueName);
    await queueDialog.getByLabel("Retention (seconds)").fill("3600");
    await queueDialog.getByRole("button", { name: "Create queue" }).click();
    await expect(page.getByText("Queue created.", { exact: true })).toBeVisible(
      { timeout: 15_000 },
    );
    await renameCatalogResource(page, queueName, renamedQueueName);
    await page
      .getByRole("link", { name: renamedQueueName, exact: true })
      .click();
    await expectNoLoadErrors(page);
    await page.getByRole("button", { name: "Edit configuration" }).click();
    const editQueueDialog = page.getByRole("dialog");
    await editQueueDialog.getByLabel("Retention (seconds)").fill("7200");
    await editQueueDialog
      .getByRole("button", { name: "Save configuration" })
      .click();
    await expect(
      page.getByText("Queue configuration updated.", { exact: true }),
    ).toBeVisible({ timeout: 15_000 });
    await returnToCatalog(page, "Queues");
    await deleteCatalogResource(page, renamedQueueName);

    const client = liveClient();
    const accountID = await accountId(client);
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
      await page.getByRole("button", { name: "Create Workflow" }).click();
      const workflowDialog = page.getByRole("dialog");
      await workflowDialog.getByLabel("Workflow name").fill(workflowName);
      await workflowDialog.getByLabel("Worker script name").fill(scriptName);
      await workflowDialog.getByLabel("Exported class name").fill("Flow");
      await workflowDialog
        .getByRole("button", { name: "Create workflow" })
        .click();
      await expect(
        page.getByText("Workflow created.", { exact: true }),
      ).toBeVisible({ timeout: 30_000 });
      await page.getByRole("link", { name: workflowName, exact: true }).click();
      await expect(
        page.getByRole("button", { name: "Update definition" }),
      ).toBeVisible();
      await page.getByRole("button", { name: "Update definition" }).click();
      await page
        .getByRole("dialog")
        .getByRole("button", { name: "Update workflow" })
        .click();
      await expect(
        page.getByText("Workflow definition updated.", { exact: true }),
      ).toBeVisible({ timeout: 30_000 });
      await returnToCatalog(page, "Workflows");
      await deleteCatalogResource(page, workflowName);
    } finally {
      await client.workers.scripts.delete(scriptName, {
        account_id: accountID,
      });
    }
  });
});
