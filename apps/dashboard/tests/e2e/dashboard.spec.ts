import { expect, test } from "./fixtures";
import { adminToken, dismissDevelopmentNotice, signIn } from "./helpers";

test.describe("operator dashboard", () => {
  test("login page shows brand assets", async ({ page }) => {
    await page.goto("./login");
    await expect(
      page.getByRole("heading", { name: "Log in to open-compute" }),
    ).toBeVisible();
    const mark = page.locator('img[src*="/brand/logo-"]').first();
    await expect(mark).toBeVisible();
    await expect(mark).toHaveAttribute("src", /logo-(white|black)\.svg$/);
    const wordmark = page.locator('img[src*="/brand/logo-text-"]').first();
    await expect(wordmark).toBeVisible();
    await expect(wordmark).toHaveAttribute(
      "src",
      /logo-text-(white|black)\.svg$/,
    );
    const background = page.locator('img[src*="/assets/capabilities/"]');
    await expect(background).toBeVisible();
    await expect(background).toHaveAttribute(
      "src",
      /assets\/capabilities\/(deploy|worker-apis|bindings|extensions|operate)\.webp$/,
    );
  });

  test("login page exposes project links", async ({ page }) => {
    await page.goto("./login");
    await expect(
      page.getByRole("link", { name: "GitHub", exact: true }),
    ).toHaveAttribute("href", "https://github.com/elliothux/open-compute");
    await expect(
      page.getByRole("link", { name: "Website", exact: true }),
    ).toHaveAttribute("href", "https://open-compute.dev");
    await expect(
      page.getByRole("link", { name: "Docs", exact: true }),
    ).toHaveAttribute("href", "https://open-compute.dev/docs/");
    const compatibility = page.getByLabel("Cloudflare compatibility");
    await expect(compatibility).toBeVisible();
    await expect(
      compatibility.getByText("Workers", { exact: true }).first(),
    ).toBeVisible();
    await expect(
      compatibility.getByText("Durable Objects", { exact: true }).first(),
    ).toBeVisible();
  });

  test("sign in reaches overview with branded shell", async ({ page }) => {
    const accountRequests: URL[] = [];
    page.on("request", (request) => {
      const url = new URL(request.url());
      if (url.pathname === "/client/v4/accounts") accountRequests.push(url);
    });
    await page.goto("./login");
    await page.getByLabel("Admin token").fill(adminToken);
    await page.getByRole("button", { name: "Continue" }).click();
    await expect(page).toHaveURL(/\/operator\/?$/);
    await expect(
      page
        .getByRole("alertdialog")
        .getByText("This dashboard is in development"),
    ).toBeVisible();
    await dismissDevelopmentNotice(page);
    await expect(
      page.getByRole("heading", { name: "Account home" }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Switch instance" }),
    ).toContainText("dashboard-dev");
    expect(accountRequests).toHaveLength(2);
    expect(
      accountRequests.every((url) => !url.searchParams.has("per_page")),
    ).toBe(true);
  });

  test("switches the active instance from the account menu", async ({
    page,
  }) => {
    await signIn(page);
    await page.getByRole("button", { name: "Switch instance" }).click();
    const instances = page.getByRole("listbox", { name: "Instances" });
    await expect(
      instances.getByRole("option", { name: /dashboard-dev/ }),
    ).toHaveAttribute("aria-selected", "true");
    await page.getByLabel("Search instances").fill("preview");
    const preview = instances.getByRole("option", {
      name: /dashboard-preview/,
    });
    const previewId = await preview.locator("code").innerText();
    await preview.click();

    await expect(page).toHaveURL(/\/operator\/?$/);
    await expect(
      page.getByRole("button", { name: "Switch instance" }),
    ).toContainText("dashboard-preview");
    await expect
      .poll(() =>
        page.evaluate(() => {
          const raw = sessionStorage.getItem("open-compute.operator.auth");
          return raw ? JSON.parse(raw).instanceId : null;
        }),
      )
      .toBe(previewId);
  });

  test("authenticated navigation reaches Workers catalog", async ({ page }) => {
    await page.goto("./login");
    await page.getByLabel("Admin token").fill(adminToken);
    await page.getByRole("button", { name: "Continue" }).click();
    await dismissDevelopmentNotice(page);
    await page.getByRole("link", { name: "Workers", exact: true }).click();
    await expect(page).toHaveURL(/\/operator\/workers\/?$/);
    await expect(page.getByRole("heading", { name: "Workers" })).toBeVisible();
  });

  test("primary navigation exposes every supported product area", async ({
    page,
  }) => {
    await signIn(page);
    const primaryNavigation = page.getByRole("navigation", {
      name: "Primary navigation",
    });
    for (const name of [
      "Workers",
      "Observability",
      "Durable Objects",
      "Queues",
      "Workflows",
      "Browser Run",
      "Containers",
      "Sandbox",
      "KV",
      "D1",
      "R2",
      "Vectorize",
      "AI Search",
      "Platform",
    ]) {
      await expect(
        primaryNavigation.getByRole("link", { name, exact: true }),
      ).toBeVisible();
    }

    await primaryNavigation
      .getByRole("link", { name: "Platform", exact: true })
      .click();
    await expect(page).toHaveURL(/\/operator\/platform\/?$/);
    await expect(
      page.getByRole("heading", { name: "Platform", level: 1 }),
    ).toBeVisible();
  });

  test("sidebar search filters supported pages and keeps the breadcrumb", async ({
    page,
  }) => {
    await signIn(page);
    const primaryNavigation = page.getByRole("navigation", {
      name: "Primary navigation",
    });
    await page
      .getByRole("textbox", { name: "Search navigation" })
      .fill("queue");
    await expect(
      primaryNavigation.getByRole("link", { name: "Queues" }),
    ).toBeVisible();
    await expect(
      primaryNavigation.getByRole("link", { name: "Workers", exact: true }),
    ).toHaveCount(0);
    await primaryNavigation.getByRole("link", { name: "Queues" }).click();
    await expect(
      page
        .getByRole("navigation", { name: "Breadcrumb" })
        .getByRole("link", { name: "Queues" }),
    ).toBeVisible();
  });

  test("Kumo create dialog remains inside the viewport", async ({ page }) => {
    await signIn(page);
    await page
      .getByRole("navigation", { name: "Primary navigation" })
      .getByRole("link", { name: "KV", exact: true })
      .click();
    await page
      .getByRole("button", { name: "Create namespace", exact: true })
      .first()
      .click();

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

    await expect(
      page.getByRole("heading", { name: "Workers", level: 1 }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Toggle navigation" }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Refresh", exact: true }),
    ).toBeVisible();
    const documentWidth = await page.evaluate(() => ({
      client: document.documentElement.clientWidth,
      scroll: document.documentElement.scrollWidth,
    }));
    expect(documentWidth.scroll).toBeLessThanOrEqual(documentWidth.client);
  });

  test("Kumo components render without runtime contract warnings", async ({
    page,
  }) => {
    const kumoWarnings: string[] = [];
    page.on("console", (message) => {
      if (message.text().includes("[kumo]")) kumoWarnings.push(message.text());
    });
    await signIn(page);
    await page.goto("./workers");
    await expect(
      page.getByRole("heading", { name: "Workers", level: 1 }),
    ).toBeVisible();
    expect(kumoWarnings).toEqual([]);
  });

  test("sign in survives page reload within the same tab", async ({ page }) => {
    await signIn(page);
    await page.reload();
    await expect(page).toHaveURL(/\/operator\/?$/);
    await expect(
      page.getByRole("heading", { name: "Account home" }),
    ).toBeVisible();
    await expect(page.getByRole("button", { name: "Sign out" })).toBeVisible();
  });

  test("invalid token stays on login with error", async ({ page }) => {
    await page.goto("./login");
    await page.getByLabel("Admin token").fill("not-a-valid-token");
    await page.getByRole("button", { name: "Continue" }).click();
    await expect(page).toHaveURL(/\/operator\/login\/?$/);
    await expect(
      page.getByText(
        /admin authentication is required|Unable to verify the admin token/i,
      ),
    ).toBeVisible();
  });

  for (const restricted of [
    {
      role: "deployer",
      token: process.env.OPEN_COMPUTE_DEPLOYER_TOKEN ?? "dev-deployer-token",
    },
    {
      role: "read-only",
      token: process.env.OPEN_COMPUTE_READ_ONLY_TOKEN ?? "dev-read-only-token",
    },
  ]) {
    test(`${restricted.role} token cannot mint an admin browser session`, async ({
      page,
    }) => {
      await page.goto("./login");
      await page.getByLabel("Admin token").fill(restricted.token);
      await page.getByRole("button", { name: "Continue" }).click();
      await expect(page).toHaveURL(/\/operator\/login\/?$/);
      await expect(
        page.getByText(
          /admin authentication is required|Unable to verify the admin token/i,
        ),
      ).toBeVisible();
      expect(
        await page.evaluate(() =>
          sessionStorage.getItem("open-compute.operator.auth"),
        ),
      ).toBeNull();
    });
  }

  test("revoked token clears session and returns to login", async ({
    page,
  }) => {
    await signIn(page);
    await page.evaluate(() => {
      const key = "open-compute.operator.auth";
      const raw = sessionStorage.getItem(key);
      if (!raw) throw new Error("expected persisted auth session");
      const parsed = JSON.parse(raw) as { token: string; instanceId: string };
      parsed.token = "revoked-admin-token";
      sessionStorage.setItem(key, JSON.stringify(parsed));
    });
    await page.reload();
    await expect(page).toHaveURL(/\/operator\/login\/?$/, { timeout: 15_000 });
    await expect(
      page.getByRole("heading", { name: "Log in to open-compute" }),
    ).toBeVisible();
    const session = await page.evaluate(() =>
      sessionStorage.getItem("open-compute.operator.auth"),
    );
    expect(session).toBeNull();
  });

  test("brand static assets are served from embedded dashboard", async ({
    request,
  }) => {
    for (const asset of [
      "brand/logo-black.svg",
      "brand/logo-white.svg",
      "brand/logo-text-black.svg",
      "brand/logo-text-white.svg",
      "assets/capabilities/bindings.webp",
      "assets/capabilities/deploy.webp",
      "assets/capabilities/extensions.webp",
      "assets/capabilities/operate.webp",
      "assets/capabilities/worker-apis.webp",
    ]) {
      const response = await request.get(`./${asset}`);
      expect(response.ok(), `expected ${asset} to be reachable`).toBeTruthy();
      expect(response.headers()["content-type"]).toMatch(/svg|png|webp/);
    }
  });
});
