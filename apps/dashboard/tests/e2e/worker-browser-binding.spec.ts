import { createOpenComputeClient } from "@open-compute/sdk";
import { expect, test } from "./fixtures";
import { adminToken, signIn } from "./helpers";

test.use({ actionTimeout: 30_000 });

test("Browser Run bindings preserve other bindings across immutable deployments", async ({
  page,
}, testInfo) => {
  test.setTimeout(180_000);
  if (!process.env.OPEN_COMPUTE_TEST_BROWSER)
    throw new Error(
      "Prepare chrome-headless-shell and set OPEN_COMPUTE_TEST_BROWSER.",
    );
  const root =
    process.env.OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL ??
    "http://127.0.0.1:8787/operator/";
  const api = createOpenComputeClient({
    apiToken: adminToken,
    baseURL: new URL("/client/v4", root).href,
    maxRetries: 0,
  });
  const accountId = (await api.accounts.list()).result[0]?.id;
  if (!accountId) throw new Error("Missing isolated test account.");
  const capabilities =
    await api.openCompute.capabilities.getForAccount(accountId);
  expect(capabilities.limits["browser.max_sessions"]).toBeGreaterThan(0);
  const worker = `pw-browser-${crypto.randomUUID().slice(0, 8)}`;
  let created = false;
  const active = async () => {
    const deployment = await api.workers.scripts.deployments.list(worker, {
      account_id: accountId,
    });
    const id = deployment.deployments[0]?.versions[0]?.version_id;
    if (!id) throw new Error("Missing deployed Worker Version.");
    const settings = await api.workers.scripts.scriptAndVersionSettings.get(
      worker,
      {
        account_id: accountId,
      },
    );
    return { id, bindings: settings.bindings ?? [] };
  };
  const open = async (label: string, dialogLabel = label) => {
    await page
      .locator("#bindings")
      .getByRole("button", { name: "Add binding", exact: true })
      .click();
    const gallery = page.getByRole("dialog", { name: "Add binding" });
    await gallery.getByRole("option", { name: label, exact: true }).click();
    await gallery.getByRole("button", { name: "Add binding" }).click();
    return page.getByRole("dialog", { name: `Add ${dialogLabel} binding` });
  };
  try {
    await signIn(page);
    await page
      .getByRole("navigation", { name: "Primary navigation" })
      .getByRole("link", { name: "Workers", exact: true })
      .first()
      .click();
    await page.getByRole("button", { name: "Create Worker" }).first().click();
    await page.getByRole("button", { name: "Start with Hello World!" }).click();
    await page.getByLabel("Worker name").fill(worker);
    await page.getByRole("button", { name: "Deploy", exact: true }).click();
    await expect(
      page.getByRole("heading", { name: worker, level: 1 }),
    ).toBeVisible();
    created = true;
    await page.getByRole("tab", { name: "Settings" }).click();
    const metadata = await open("Version metadata", "version metadata");
    await metadata
      .getByRole("textbox", { name: "Variable name" })
      .fill("VERSION");
    await metadata.getByRole("button", { name: "Add binding" }).click();
    await expect(metadata).toBeHidden();
    const before = await active();
    const add = await open("Browser Run");
    await add
      .getByRole("textbox", { name: "Variable name" })
      .fill("INVALID-NAME");
    await expect(
      add.getByRole("button", { name: "Add binding" }),
    ).toBeDisabled();
    await add.getByRole("textbox", { name: "Variable name" }).fill("VERSION");
    await expect(
      add.getByRole("button", { name: "Add binding" }),
    ).toBeDisabled();
    await add.getByRole("textbox", { name: "Variable name" }).fill("BROWSER");
    await page.setViewportSize({ width: 390, height: 844 });
    await expect
      .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
      .toBe(390);
    await add.screenshot({
      path: testInfo.outputPath("browser-binding-mobile.png"),
    });
    await page.setViewportSize({ width: 1280, height: 900 });
    await add.getByRole("button", { name: "Add binding" }).click();
    await expect(add).toBeHidden();
    const added = await active();
    expect(added.id).not.toBe(before.id);
    expect(added.bindings).toContainEqual({ type: "browser", name: "BROWSER" });
    expect(added.bindings).toContainEqual({
      type: "version_metadata",
      name: "VERSION",
    });
    await page
      .getByRole("button", { name: "Edit BROWSER", exact: true })
      .click();
    const edit = page.getByRole("dialog", { name: "Edit Browser Run binding" });
    await expect(edit.getByRole("button", { name: "Deploy" })).toBeDisabled();
    await edit.getByRole("textbox", { name: "Variable name" }).fill("RENDER");
    await edit.getByRole("button", { name: "Deploy" }).click();
    await expect(edit).toBeHidden();
    const renamed = await active();
    expect(renamed.id).not.toBe(added.id);
    expect(renamed.bindings).toContainEqual({
      type: "browser",
      name: "RENDER",
    });
    expect(renamed.bindings.some((binding) => binding.name === "BROWSER")).toBe(
      false,
    );
    await page
      .getByRole("button", { name: "Delete RENDER", exact: true })
      .click();
    const confirmation = page.getByRole("alertdialog", {
      name: "Delete Browser Run binding?",
    });
    await confirmation
      .getByRole("button", { name: "Delete and deploy" })
      .click();
    await expect(confirmation).toBeHidden();
    const removed = await active();
    expect(removed.id).not.toBe(renamed.id);
    expect(removed.bindings.some((binding) => binding.type === "browser")).toBe(
      false,
    );
    expect(removed.bindings).toContainEqual({
      type: "version_metadata",
      name: "VERSION",
    });
    await page.route(
      `**/accounts/${accountId}/open-compute/capabilities`,
      async (route) => {
        await route.fulfill({
          json: {
            success: true,
            errors: [],
            messages: [],
            result: {
              ...capabilities,
              limits: Object.fromEntries(
                Object.entries(capabilities.limits).filter(
                  ([key]) => !key.startsWith("browser."),
                ),
              ),
            },
          },
        });
      },
    );
    await page.reload();
    await page.getByRole("tab", { name: "Settings" }).click();
    await page
      .locator("#bindings")
      .getByRole("button", { name: "Add binding", exact: true })
      .click();
    const gallery = page.getByRole("dialog", { name: "Add binding" });
    await expect(
      gallery.getByRole("option", { name: "Browser Run", exact: true }),
    ).toHaveCount(0);
    await expect(
      gallery.getByRole("option", { name: "KV namespace", exact: true }),
    ).toBeVisible();
  } finally {
    if (created)
      await api.workers.scripts.delete(worker, { account_id: accountId });
  }
});
