import { expect, type Page } from "@playwright/test";

export const adminToken =
  process.env.OPEN_COMPUTE_ADMIN_TOKEN ?? "dev-admin-token";

/** Sign in through the live Operator login flow. */
export async function signIn(page: Page) {
  await page.goto("./login");
  await expect(
    page.getByRole("heading", { name: "Log in to open-compute" }),
  ).toBeVisible({ timeout: 15_000 });
  await page.getByLabel("Admin token").fill(adminToken);
  await page.getByRole("button", { name: "Continue" }).click();
  await expect(page).toHaveURL(/\/operator\/?$/);
  await dismissDevelopmentNotice(page);
  await expect(
    page.getByRole("heading", { name: "Account home" }),
  ).toBeVisible();
}

/** Close the development-notice alert shown after a successful sign-in. */
export async function dismissDevelopmentNotice(page: Page) {
  const dialog = page.getByRole("alertdialog");
  if (await dialog.isVisible().catch(() => false)) {
    await dialog.getByRole("button", { name: "Got it" }).click();
  }
}

/** Fail when catalog/detail pages surface the shared API error state. */
export async function expectNoLoadErrors(page: Page) {
  await expect(page.getByText(/^Unable to load /i)).toHaveCount(0);
}
