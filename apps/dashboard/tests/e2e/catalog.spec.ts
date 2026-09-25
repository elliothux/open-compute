import { resolve } from "node:path";
import Cloudflare from "cloudflare";
import { expect, test } from "./fixtures";
import { adminToken, expectNoLoadErrors, signIn } from "./helpers";

const catalogPages = [
  { link: "Workers", heading: "Workers", path: /\/operator\/workers\/?$/ },
  { link: "KV", heading: "KV", path: /\/operator\/kv\/?$/ },
  { link: "D1", heading: "D1 databases", path: /\/operator\/d1\/?$/ },
  { link: "R2", heading: "R2 object storage", path: /\/operator\/r2\/?$/ },
  {
    link: "Durable Objects",
    heading: "Durable Objects",
    path: /\/operator\/durable-objects\/?$/,
  },
  { link: "Queues", heading: "Queues", path: /\/operator\/queues\/?$/ },
  {
    link: "Workflows",
    heading: "Workflows",
    path: /\/operator\/workflows\/?$/,
  },
  { link: "Platform", heading: "Platform", path: /\/operator\/platform\/?$/ },
] as const;

test.describe("operator dashboard catalogs", () => {
  test.beforeEach(async ({ page }) => {
    await signIn(page);
  });

  test("overview loads platform summaries without API error states", async ({
    page,
  }) => {
    await expect(
      page.getByText("Unable to load release metadata."),
    ).toHaveCount(0);
    await expect(page.getByText("Unable to load system status.")).toHaveCount(
      0,
    );
    await expect(
      page.locator("a.text-kumo-link", { hasText: "Unavailable" }),
    ).toHaveCount(0);
  });

  for (const catalog of catalogPages) {
    test(`${catalog.heading} catalog renders without load failures`, async ({
      page,
    }) => {
      await page
        .getByRole("navigation")
        .getByRole("link", { name: catalog.link, exact: true })
        .click();
      await expect(page).toHaveURL(catalog.path);
      await expect(
        page.getByRole("heading", { name: catalog.heading, level: 1 }),
      ).toBeVisible();
      await expectNoLoadErrors(page);
    });
  }

  test("Workers catalog exposes create and refresh actions", async ({
    page,
  }) => {
    await page
      .getByRole("navigation")
      .getByRole("link", { name: "Workers", exact: true })
      .click();
    await expect(
      page.getByRole("button", { name: "Refresh", exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Create Worker" }).first(),
    ).toBeVisible();
  });

  test("KV catalog shows create action and search toolbar", async ({
    page,
  }) => {
    await page
      .getByRole("navigation")
      .getByRole("link", { name: "KV", exact: true })
      .click();
    await expect(
      page
        .getByRole("button", { name: "Create namespace", exact: true })
        .first(),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Refresh", exact: true }),
    ).toBeVisible();
  });

  test("KV list exposes loaded rows, search, and row actions", async ({
    page,
  }) => {
    const root =
      process.env.OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL ??
      "http://127.0.0.1:8787/operator/";
    const client = new Cloudflare({
      apiToken: adminToken,
      baseURL: new URL("/client/v4", root).href,
      maxRetries: 0,
    });
    const accountId = (await client.accounts.list()).result[0]?.id;
    if (!accountId) throw new Error("dashboard E2E account is missing");
    const name = `pw-kv-list-${crypto.randomUUID().replaceAll("-", "")}`;
    const namespace = await client.kv.namespaces.create({
      account_id: accountId,
      title: name,
    });
    try {
      await page
        .getByRole("navigation")
        .getByRole("link", { name: "KV", exact: true })
        .click();
      await expect(page.getByRole("link", { name, exact: true })).toBeVisible();
      await page.setViewportSize({ width: 1808, height: 1113 });
      await page.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/kv-list-loaded-desktop.png",
        ),
        fullPage: true,
      });
      await page.getByRole("textbox", { name: "Search namespaces" }).fill(name);
      await expect(page.getByRole("link", { name, exact: true })).toBeVisible();
      await page.getByRole("button", { name: `Actions for ${name}` }).click();
      await expect(
        page.getByRole("menuitem", { name: "Copy bindings" }),
      ).toBeVisible();
      await expect(
        page.getByRole("menuitem", { name: "Delete", exact: true }),
      ).toBeVisible();
      await page.getByRole("menuitem", { name: "Delete", exact: true }).click();
      await expect(page.getByRole("alertdialog")).toBeVisible();
      await page
        .getByRole("alertdialog")
        .getByRole("button", { name: "Cancel" })
        .click();
      await page.setViewportSize({ width: 390, height: 844 });
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
        .toBeLessThanOrEqual(390);
      await page.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/kv-list-loaded-mobile.png",
        ),
        fullPage: true,
      });
      await page.getByRole("link", { name, exact: true }).click();
      await expect(page).toHaveURL(/\/operator\/kv\/[^/]+$/);
    } finally {
      await client.kv.namespaces.delete(namespace.id, {
        account_id: accountId,
      });
    }
  });
});
