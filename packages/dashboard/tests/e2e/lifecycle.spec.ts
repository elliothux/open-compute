import { expect, test } from "@playwright/test";
import { expectNoLoadErrors, signIn } from "./helpers";

async function deleteCatalogResource(page: import("@playwright/test").Page, name: string) {
  await page.getByRole("button", { name: `Actions for ${name}` }).click();
  await page.getByRole("menuitem", { name: "Delete" }).click();
  const dialog = page.getByRole("alertdialog");
  await expect(dialog.getByRole("button", { name: "Delete" })).toBeDisabled();
  await dialog.getByRole("textbox").fill(name);
  await dialog.getByRole("button", { name: "Delete" }).click();
  await expect(page.getByRole("cell", { name, exact: true })).toHaveCount(0);
}

async function renameCatalogResource(
  page: import("@playwright/test").Page,
  currentName: string,
  newName: string,
) {
  await page.getByRole("button", { name: `Actions for ${currentName}` }).click();
  await page.getByRole("menuitem", { name: "Rename" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("textbox").fill(newName);
  await dialog.getByRole("button", { name: "Save" }).click();
  await expect(page.getByRole("cell", { name: newName, exact: true })).toBeVisible({ timeout: 15_000 });
  await expect(page.getByRole("cell", { name: currentName, exact: true })).toHaveCount(0);
}

test.describe("operator dashboard live lifecycle", () => {
  test.beforeEach(async ({ page }) => {
    await signIn(page);
  });

  test("catalog filters and sort are server-backed and survive reload", async ({ page }) => {
    await page.getByRole("navigation").getByRole("link", { name: "Workers", exact: true }).click();
    await page.getByRole("combobox", { name: "Deployment status" }).click();
    await page.getByRole("option", { name: "Not deployed" }).click();
    await page.getByRole("combobox", { name: "Sort Workers" }).click();
    await page.getByRole("option", { name: "Name A–Z" }).click();
    await expect(page).toHaveURL(/deployed=undeployed/);
    await expect(page).toHaveURL(/sort=name/);
    await expect(page).toHaveURL(/direction=asc/);
    await page.reload();
    await expect(page.getByRole("combobox", { name: "Deployment status" })).toContainText("Not deployed");
    await expect(page.getByRole("combobox", { name: "Sort Workers" })).toContainText("Name A–Z");
    await expect(page.getByRole("heading", { name: "Usage since startup" })).toBeVisible();
    await expectNoLoadErrors(page);
  });

  test("D1 mutation, validation, migration, backup, and authority refresh", async ({ page }) => {
    const name = `pw-d1-${Date.now()}`;
    const renamedName = `${name}-renamed`;
    const restoredName = `${name}-restored`;
    await page.getByRole("navigation").getByRole("link", { name: "D1", exact: true }).click();
    await page.route("**/accounts/*/d1/databases", async route => {
      if (route.request().method() === "POST") await new Promise(resolve => setTimeout(resolve, 250));
      await route.continue();
    });
    await page.getByRole("button", { name: "Create database" }).first().click();
    const createDialog = page.getByRole("dialog");
    await expect(createDialog.getByRole("button", { name: "Create database" })).toBeDisabled();
    await createDialog.getByLabel("Database name").fill(name);
    await createDialog.getByRole("button", { name: "Create database" }).click();
    await expect(createDialog.getByRole("button", { name: "Creating…" })).toBeDisabled();
    await expect(page.getByRole("cell", { name, exact: true })).toBeVisible({ timeout: 15_000 });
    await page.unroute("**/accounts/*/d1/databases");

    await renameCatalogResource(page, name, renamedName);

    await page.getByRole("button", { name: "Create database" }).first().click();
    await page.getByRole("dialog").getByLabel("Database name").fill(renamedName);
    await page.getByRole("dialog").getByRole("button", { name: "Create database" }).click();
    await expect(page.getByRole("dialog").getByRole("alert")).toBeVisible();
    await page.getByRole("dialog").getByRole("button", { name: "Cancel" }).click();

    await page.getByRole("button", { name: `Actions for ${renamedName}` }).click();
    await page.getByRole("menuitem", { name: "Open studio" }).click();
    await page.getByRole("tab", { name: "Query" }).click();
    await page.locator("textarea").fill("SELECT * FROM");
    await page.getByRole("button", { name: "Run query" }).click();
    await expect(page.getByText("Query failed.", { exact: true })).toBeVisible();

    await page.getByRole("tab", { name: "Migrations" }).click();
    await page.getByLabel("Migration id").fill("1");
    await page.getByRole("textbox", { name: "Name", exact: true }).fill("0001_init.sql");
    await page.locator("textarea").fill("CREATE TABLE lifecycle_test (id INTEGER PRIMARY KEY);");
    await page.getByRole("button", { name: "Apply migration" }).click();
    await expect(page.getByText("Migration applied.", { exact: true })).toBeVisible({ timeout: 15_000 });
    await expect(page.getByRole("cell", { name: "0001_init.sql", exact: true })).toBeVisible();

    await page.getByRole("tab", { name: "Backups" }).click();
    await page.getByRole("button", { name: "Create backup" }).click();
    await expect(page.getByText("Backup created.", { exact: true })).toBeVisible({ timeout: 15_000 });
    const backupRow = page.getByRole("row").filter({ hasText: "ready" }).last();
    await expect(backupRow.getByRole("cell", { name: "ready", exact: true })).toBeVisible();
    await backupRow.getByRole("button", { name: "Restore" }).click();
    const restoreDialog = page.getByRole("dialog");
    await restoreDialog.getByLabel("New database name").fill(restoredName);
    await restoreDialog.getByRole("button", { name: "Restore database" }).click();
    await expect(page.getByText(/Restored database/)).toBeVisible({ timeout: 15_000 });

    await page.getByRole("link", { name: "Back to D1" }).click();
    await expect(page.getByRole("cell", { name: restoredName, exact: true })).toBeVisible({ timeout: 15_000 });
    await deleteCatalogResource(page, renamedName);
    await deleteCatalogResource(page, restoredName);
  });

  test("KV rename, value metadata, backup, restore, and deletion stay canonical", async ({ page }) => {
    const name = `pw-kv-${Date.now()}`;
    const renamedName = `${name}-renamed`;
    const restoredName = `${name}-restored`;
    const key = `profile/${Date.now()}`;
    const value = "live KV value";
    await page.getByRole("navigation").getByRole("link", { name: "KV", exact: true }).click();
    await page.getByRole("button", { name: "Create namespace" }).first().click();
    const createDialog = page.getByRole("dialog");
    await createDialog.getByLabel("Namespace name").fill(name);
    await createDialog.getByRole("button", { name: "Create namespace" }).click();
    await expect(page.getByRole("cell", { name, exact: true })).toBeVisible({ timeout: 15_000 });

    await renameCatalogResource(page, name, renamedName);
    await page.getByRole("button", { name: `Actions for ${renamedName}` }).click();
    await page.getByRole("menuitem", { name: "Browse keys" }).click();
    await expect(page.getByRole("button", { name: "Copy namespace id", exact: false })).toBeVisible();
    await page.getByRole("tab", { name: "Write" }).click();
    await page.getByLabel("Key").fill(key);
    await page.getByLabel("Value").fill(value);
    await page.getByLabel("JSON metadata").fill('{"region":"test"}');
    await page.getByLabel("Expiration TTL (seconds, optional)").fill("59");
    await expect(page.getByRole("button", { name: "Save value" })).toBeDisabled();
    await page.getByLabel("Expiration TTL (seconds, optional)").fill("120");
    await page.getByRole("button", { name: "Save value" }).click();
    await expect(page.getByText(value, { exact: true })).toBeVisible({ timeout: 15_000 });
    await expect(page.getByText(/"region": "test"/)).toBeVisible();

    await page.getByRole("tab", { name: "Backups" }).click();
    await page.getByRole("button", { name: "Create backup" }).click();
    await expect(page.getByText("KV backup created.", { exact: true })).toBeVisible({ timeout: 15_000 });
    const backupRow = page.getByRole("row").filter({ hasText: "ready" }).last();
    await backupRow.getByRole("button", { name: "Restore" }).click();
    const restoreDialog = page.getByRole("dialog");
    await restoreDialog.getByLabel("New namespace name").fill(restoredName);
    await restoreDialog.getByRole("button", { name: "Restore namespace" }).click();
    await expect(page.getByText(/Restored namespace/)).toBeVisible({ timeout: 15_000 });

    await page.getByRole("tab", { name: "KV pairs" }).click();
    await page.getByRole("button", { name: "Delete key" }).click();
    const deleteKeyDialog = page.getByRole("alertdialog");
    await deleteKeyDialog.getByRole("textbox").fill(key);
    await deleteKeyDialog.getByRole("button", { name: "Delete key" }).click();
    await expect(page.getByRole("cell", { name: key, exact: true })).toHaveCount(0);

    await page.getByRole("link", { name: "Back to KV" }).click();
    await expect(page.getByRole("cell", { name: restoredName, exact: true })).toBeVisible({ timeout: 15_000 });
    await deleteCatalogResource(page, renamedName);
    await deleteCatalogResource(page, restoredName);
  });

  test("R2 upload, preview, download, and confirmed deletion", async ({ page }) => {
    const name = `pw-r2-${Date.now()}`;
    const renamedName = `${name}-renamed`;
    const key = `notes/${Date.now()}.txt`;
    const body = "open-compute live R2 browser lifecycle";
    await page.getByRole("navigation").getByRole("link", { name: "R2", exact: true }).click();
    await page.getByRole("button", { name: "Create bucket" }).first().click();
    await page.getByRole("dialog").getByLabel("Bucket name").fill(name);
    await page.getByRole("dialog").getByRole("button", { name: "Create bucket" }).click();
    await expect(page.getByRole("cell", { name, exact: true })).toBeVisible({ timeout: 15_000 });
    await renameCatalogResource(page, name, renamedName);
    await page.getByRole("button", { name: `Actions for ${renamedName}` }).click();
    await page.getByRole("menuitem", { name: "Browse objects" }).click();
    await page.getByRole("tab", { name: "Upload" }).click();
    await page.getByLabel("Object key").fill(key);
    await page.locator('input[type="file"]').setInputFiles({
      name: "lifecycle.txt",
      mimeType: "text/plain",
      buffer: Buffer.from(body),
    });
    await page.getByRole("button", { name: "Upload object" }).click();
    await expect(page.getByText(body, { exact: true })).toBeVisible({ timeout: 15_000 });
    const download = page.waitForEvent("download");
    await page.getByRole("button", { name: "Download" }).click();
    await download;
    await page.getByRole("button", { name: "Delete object" }).click();
    const deleteObjectDialog = page.getByRole("alertdialog");
    await deleteObjectDialog.getByRole("textbox").fill(key);
    await deleteObjectDialog.getByRole("button", { name: "Delete object" }).click();
    await expect(page.getByText(body, { exact: true })).toHaveCount(0);

    const retainedKey = `retained/${Date.now()}.txt`;
    await page.getByRole("tab", { name: "Upload" }).click();
    await page.getByLabel("Object key").fill(retainedKey);
    await page.locator('input[type="file"]').setInputFiles({
      name: "retained.txt",
      mimeType: "text/plain",
      buffer: Buffer.from("force-delete coverage"),
    });
    await page.getByRole("button", { name: "Upload object" }).click();
    await expect(page.getByText("force-delete coverage", { exact: true })).toBeVisible({ timeout: 15_000 });
    await page.getByRole("link", { name: "Back to R2" }).click();
    await page.getByRole("button", { name: `Actions for ${renamedName}` }).click();
    await page.getByRole("menuitem", { name: "Delete" }).click();
    const deleteBucketDialog = page.getByRole("alertdialog");
    await deleteBucketDialog.getByRole("textbox").fill(renamedName);
    await deleteBucketDialog.getByRole("checkbox", { name: "Delete all objects in this bucket" }).check();
    await deleteBucketDialog.getByRole("button", { name: "Delete" }).click();
    await expect(page.getByRole("cell", { name: renamedName, exact: true })).toHaveCount(0);
  });

  test("Queue config, Workflow failure, and Platform maintenance stay live", async ({ page }) => {
    const queueName = `pw-queue-${Date.now()}`;
    await page.getByRole("navigation").getByRole("link", { name: "Queues", exact: true }).click();
    await page.getByRole("button", { name: "Create Queue" }).first().click();
    const queueDialog = page.getByRole("dialog");
    await queueDialog.getByLabel("Queue name").fill(queueName);
    await queueDialog.getByLabel("Retention (seconds)").fill("1.5");
    await expect(queueDialog.getByRole("alert")).toContainText("whole-number");
    await expect(queueDialog.getByRole("button", { name: "Create queue" })).toBeDisabled();
    await queueDialog.getByLabel("Retention (seconds)").fill("120");
    await queueDialog.getByRole("button", { name: "Create queue" }).click();
    await expect(page.getByRole("cell", { name: queueName, exact: true })).toBeVisible({ timeout: 15_000 });
    const renamedQueueName = `${queueName}-renamed`;
    await renameCatalogResource(page, queueName, renamedQueueName);
    await page.getByRole("button", { name: `Actions for ${renamedQueueName}` }).click();
    await page.getByRole("menuitem", { name: "Open" }).click();
    await page.getByRole("button", { name: "Edit configuration" }).click();
    await page.getByRole("dialog").getByLabel("Retention (seconds)").fill("240");
    await page.getByRole("dialog").getByRole("button", { name: "Save configuration" }).click();
    await expect(page.getByText("240s", { exact: true })).toBeVisible({ timeout: 15_000 });
    await page.getByRole("link", { name: "Back to queues" }).click();
    await deleteCatalogResource(page, renamedQueueName);

    const workflowName = `pw-workflow-${Date.now()}`;
    await page.getByRole("navigation").getByRole("link", { name: "Workflows", exact: true }).click();
    await page.getByRole("button", { name: "Create Workflow" }).first().click();
    await page.getByRole("dialog").getByLabel("Workflow name").fill(workflowName);
    await page.getByRole("dialog").getByRole("button", { name: "Create Workflow" }).click();
    await expect(page.getByRole("cell", { name: workflowName, exact: true })).toBeVisible({ timeout: 15_000 });
    const renamedWorkflowName = `${workflowName}-renamed`;
    await renameCatalogResource(page, workflowName, renamedWorkflowName);
    await page.getByRole("button", { name: `Actions for ${renamedWorkflowName}` }).click();
    await page.getByRole("menuitem", { name: "Open" }).click();
    await page.getByRole("tab", { name: "Versions" }).click();
    await page.getByRole("button", { name: "Create version" }).click();
    await page.getByRole("dialog").getByLabel("Deployment ID").fill("not-a-deployment-id");
    await page.getByRole("dialog").getByLabel("Exported class name").fill("MyWorkflow");
    await page.getByRole("dialog").getByRole("button", { name: "Create version" }).click();
    await expect(page.getByRole("dialog").getByRole("alert")).toBeVisible();
    await page.getByRole("dialog").getByRole("button", { name: "Cancel" }).click();
    await page.getByRole("link", { name: "Back to Workflows" }).click();
    await deleteCatalogResource(page, renamedWorkflowName);

    await page.getByRole("navigation").getByRole("link", { name: "Platform", exact: true }).click();
    await page.getByRole("button", { name: "Pause", exact: true }).first().click();
    await expect(page.getByText("Scheduler paused.", { exact: true })).toBeVisible({ timeout: 15_000 });
    await expect(page.getByText("Global scheduler state: paused", { exact: true })).toBeVisible();
    await page.getByRole("button", { name: "Resume", exact: true }).first().click();
    await expect(page.getByText("Scheduler resumed.", { exact: true })).toBeVisible({ timeout: 15_000 });
    await expect(page.getByText("Global scheduler state: running", { exact: true })).toBeVisible();
    await page.getByRole("button", { name: "Repair", exact: true }).click();
    await expect(page.getByText(/Scheduler repair completed/)).toBeVisible({ timeout: 15_000 });
    await page.getByRole("button", { name: "Run cache GC" }).click();
    const gcDialog = page.getByRole("alertdialog");
    await expect(gcDialog.getByRole("button", { name: "Run garbage collection" })).toBeDisabled();
    await gcDialog.getByRole("textbox").fill("cache");
    await gcDialog.getByRole("button", { name: "Run garbage collection" }).click();
    await expect(page.getByText(/Cache garbage collection removed/)).toBeVisible({ timeout: 15_000 });
    await page.getByRole("button", { name: "Reconcile workflows" }).click();
    await expect(page.getByText("Workflow reconciliation completed.", { exact: true })).toBeVisible({ timeout: 15_000 });
  });
});
