import { expect, test } from "@playwright/test";
import { expectNoLoadErrors, signIn } from "./helpers";

test.describe("operator dashboard detail pages", () => {
  test.beforeEach(async ({ page }) => {
    await signIn(page);
  });

  test("Worker detail opens from catalog row actions", async ({ page }) => {
    await page.getByRole("navigation").getByRole("link", { name: "Workers", exact: true }).click();
    const name = `pw-worker-${Date.now()}`;
    await page.getByRole("button", { name: "Create Worker" }).first().click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("Worker name").fill(name);
    await dialog.getByRole("button", { name: "Create Worker" }).click();
    await expect(page.getByText(name, { exact: true })).toBeVisible({ timeout: 15_000 });

    await page.getByRole("button", { name: `Actions for ${name}` }).click();
    await page.getByRole("menuitem", { name: "Open" }).click();
    await expect(page).toHaveURL(/\/operator\/workers\//);
    await expect(page.getByRole("heading", { name, level: 1 })).toBeVisible();
    await expectNoLoadErrors(page);
  });

  test("KV namespace detail opens from catalog row actions", async ({ page }) => {
    await page.getByRole("navigation").getByRole("link", { name: "KV", exact: true }).click();
    const name = `PW_KV_${Date.now()}`;
    await page.getByRole("button", { name: "Create namespace" }).first().click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("Namespace name").fill(name);
    await dialog.getByRole("button", { name: "Create namespace" }).click();
    await expect(page.getByRole("cell", { name, exact: true })).toBeVisible({ timeout: 15_000 });

    await page.getByRole("button", { name: `Actions for ${name}` }).click();
    await page.getByRole("menuitem", { name: "Browse keys" }).click();
    await expect(page).toHaveURL(/\/operator\/kv\//);
    await expectNoLoadErrors(page);
    await expect(page.getByRole("tab", { name: "KV pairs" })).toBeVisible();
  });
});
