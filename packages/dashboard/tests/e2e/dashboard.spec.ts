import { expect, test } from "@playwright/test";
import { adminToken, signIn } from "./helpers";

test.describe("operator dashboard", () => {
  test("login page shows brand assets", async ({ page }) => {
    await page.goto("./login");
    await expect(page.getByRole("heading", { name: "Operator sign in" })).toBeVisible();
    await expect(page.getByText("open-compute", { exact: true })).toBeVisible();
    const mark = page.locator('img[src*="/brand/logo-"]').first();
    await expect(mark).toBeVisible();
    await expect(mark).toHaveAttribute("src", /logo-(white|black)\.svg$/);
  });

  test("sign in reaches overview with branded shell", async ({ page }) => {
    await page.goto("./login");
    await page.getByLabel("Admin token").fill(adminToken);
    await page.getByRole("button", { name: "Continue" }).click();
    await expect(page).toHaveURL(/\/operator\/?$/);
    await expect(page.getByRole("heading", { name: "Overview" })).toBeVisible();
    await expect(page.locator('aside').getByText("open-compute", { exact: true })).toBeVisible();
    await expect(page.locator('header img[alt="open-compute"]')).toBeVisible();
  });

  test("authenticated navigation reaches Workers catalog", async ({ page }) => {
    await page.goto("./login");
    await page.getByLabel("Admin token").fill(adminToken);
    await page.getByRole("button", { name: "Continue" }).click();
    await page.getByRole("link", { name: "Workers" }).click();
    await expect(page).toHaveURL(/\/operator\/workers\/?$/);
    await expect(page.getByRole("heading", { name: "Workers" })).toBeVisible();
  });

  test("primary navigation and command palette expose every product area", async ({ page }) => {
    await signIn(page);
    const primaryNavigation = page.getByRole("navigation", { name: "Primary navigation" });
    await expect(primaryNavigation.getByRole("link", { name: "Workers", exact: true })).toBeVisible();
    await expect(primaryNavigation.getByRole("link", { name: "Platform", exact: true })).toBeVisible();

    await page.getByRole("button", { name: "Search pages" }).click();
    const palette = page.getByRole("dialog");
    await palette.getByRole("combobox").fill("Platform");
    await palette.getByRole("option", { name: /^Platform/ }).click();
    await expect(page).toHaveURL(/\/operator\/platform\/?$/);
    await expect(page.getByRole("heading", { name: "Platform", level: 1 })).toBeVisible();
  });

  test("Kumo create dialog remains inside the viewport", async ({ page }) => {
    await signIn(page);
    await page.getByRole("navigation", { name: "Primary navigation" })
      .getByRole("link", { name: "Workers", exact: true })
      .click();
    await page.getByRole("button", { name: "Create Worker" }).first().click();

    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible();
    const box = await dialog.boundingBox();
    const viewport = page.viewportSize();
    expect(box).not.toBeNull();
    expect(viewport).not.toBeNull();
    expect(box!.x).toBeGreaterThanOrEqual(0);
    expect(box!.y).toBeGreaterThanOrEqual(0);
    expect(box!.x + box!.width).toBeLessThanOrEqual(viewport!.width);
    expect(box!.y + box!.height).toBeLessThanOrEqual(viewport!.height);
    await dialog.getByRole("button", { name: "Cancel" }).click();
    await expect(dialog).toBeHidden();
  });

  test("Workers catalog stays usable at a 390px viewport", async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await signIn(page);
    await page.goto("./workers");

    await expect(page.getByRole("heading", { name: "Workers", level: 1 })).toBeVisible();
    await expect(page.getByRole("button", { name: "Toggle navigation" })).toBeVisible();
    await expect(page.getByRole("button", { name: "Create Worker" }).first()).toBeVisible();
    await expect(page.getByRole("columnheader", { name: "Worker ID" })).toBeHidden();
    const documentWidth = await page.evaluate(() => ({
      client: document.documentElement.clientWidth,
      scroll: document.documentElement.scrollWidth,
    }));
    expect(documentWidth.scroll).toBeLessThanOrEqual(documentWidth.client);
  });

  test("Kumo components render without runtime contract warnings", async ({ page }) => {
    const kumoWarnings: string[] = [];
    page.on("console", message => {
      if (message.text().includes("[kumo]")) kumoWarnings.push(message.text());
    });
    await signIn(page);
    await page.goto("./workers");
    await expect(page.getByRole("heading", { name: "Workers", level: 1 })).toBeVisible();
    expect(kumoWarnings).toEqual([]);
  });

  test("sign in survives page reload within the same tab", async ({ page }) => {
    await signIn(page);
    await page.reload();
    await expect(page).toHaveURL(/\/operator\/?$/);
    await expect(page.getByRole("heading", { name: "Overview" })).toBeVisible();
    await expect(page.getByRole("button", { name: "Sign out" })).toBeVisible();
  });

  test("invalid token stays on login with error", async ({ page }) => {
    await page.goto("./login");
    await page.getByLabel("Admin token").fill("not-a-valid-token");
    await page.getByRole("button", { name: "Continue" }).click();
    await expect(page).toHaveURL(/\/operator\/login\/?$/);
    await expect(page.getByText(/admin authentication is required|Unable to verify the admin token/i)).toBeVisible();
  });

  test("revoked token clears session and returns to login", async ({ page }) => {
    await signIn(page);
    await page.evaluate(() => {
      const key = "open-compute.operator.auth";
      const raw = sessionStorage.getItem(key);
      if (!raw) throw new Error("expected persisted auth session");
      const parsed = JSON.parse(raw) as { token: string; accountId: string };
      parsed.token = "revoked-admin-token";
      sessionStorage.setItem(key, JSON.stringify(parsed));
    });
    await page.reload();
    await expect(page).toHaveURL(/\/operator\/login\/?$/, { timeout: 15_000 });
    await expect(page.getByRole("heading", { name: "Operator sign in" })).toBeVisible();
    const session = await page.evaluate(() => sessionStorage.getItem("open-compute.operator.auth"));
    expect(session).toBeNull();
  });

  test("brand static assets are served from embedded dashboard", async ({ request }) => {
    for (const asset of ["brand/logo-black.svg", "brand/logo-white.svg"]) {
      const response = await request.get(`./${asset}`);
      expect(response.ok(), `expected ${asset} to be reachable`).toBeTruthy();
      expect(response.headers()["content-type"]).toMatch(/svg|png/);
    }
  });
});
