import { expect, test } from "./fixtures";
import { expectNoLoadErrors, signIn } from "./helpers";

const catalogPages = [
  { link: "Workers", heading: "Workers", path: /\/operator\/workers\/?$/ },
  { link: "KV", heading: "KV", path: /\/operator\/kv\/?$/ },
  { link: "D1", heading: "D1", path: /\/operator\/d1\/?$/ },
  { link: "R2", heading: "R2", path: /\/operator\/r2\/?$/ },
  { link: "Durable Objects", heading: "Durable Objects", path: /\/operator\/durable-objects\/?$/ },
  { link: "Queues", heading: "Queues", path: /\/operator\/queues\/?$/ },
  { link: "Workflows", heading: "Workflows", path: /\/operator\/workflows\/?$/ },
  { link: "Platform", heading: "Platform", path: /\/operator\/platform\/?$/ },
] as const;

test.describe("operator dashboard catalogs", () => {
  test.beforeEach(async ({ page }) => {
    await signIn(page);
  });

  test("overview loads platform summaries without API error states", async ({ page }) => {
    await expect(page.getByText("Unable to load release metadata.")).toHaveCount(0);
    await expect(page.getByText("Unable to load system status.")).toHaveCount(0);
    await expect(page.locator("a.text-kumo-link", { hasText: "Unavailable" })).toHaveCount(0);
  });

  for (const catalog of catalogPages) {
    test(`${catalog.heading} catalog renders without load failures`, async ({ page }) => {
      await page.getByRole("navigation").getByRole("link", { name: catalog.link, exact: true }).click();
      await expect(page).toHaveURL(catalog.path);
      await expect(page.getByRole("heading", { name: catalog.heading, level: 1 })).toBeVisible();
      await expectNoLoadErrors(page);
    });
  }

  test("Workers catalog exposes only its supported actions", async ({ page }) => {
    await page.getByRole("navigation").getByRole("link", { name: "Workers", exact: true }).click();
    await expect(page.getByRole("button", { name: "Refresh catalog" })).toBeVisible();
    await expect(page.getByRole("button", { name: "Create" })).toHaveCount(0);
  });

  test("KV catalog shows create action and search toolbar", async ({ page }) => {
    await page.getByRole("navigation").getByRole("link", { name: "KV", exact: true }).click();
    await expect(page.getByRole("button", { name: "Create", exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Refresh catalog" })).toBeVisible();
  });
});
