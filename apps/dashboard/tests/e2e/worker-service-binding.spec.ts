import { resolve } from "node:path";
import Cloudflare from "cloudflare";
import { expect, test } from "./fixtures";
import { adminToken, signIn } from "./helpers";

test("Service binding selects another Worker and preserves its target across versions", async ({
  page,
}) => {
  test.setTimeout(120_000);
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
  const suffix = crypto.randomUUID().slice(0, 8);
  const caller = `pw-service-caller-${suffix}`;
  const target = `pw-service-target-${suffix}`;
  const replacement = `pw-service-next-${suffix}`;
  const created: string[] = [];
  const create = async (name: string) => {
    await page.goto("./workers");
    await page.getByRole("button", { name: "Create Worker" }).first().click();
    await page.getByRole("button", { name: "Start with Hello World!" }).click();
    await page.getByLabel("Worker name").fill(name);
    await page.getByRole("button", { name: "Deploy", exact: true }).click();
    await expect(page.getByRole("heading", { name, level: 1 })).toBeVisible();
    created.push(name);
  };
  const binding = async () => {
    const deployments = await client.workers.scripts.deployments.list(caller, {
      account_id: accountId,
    });
    const id = deployments.deployments[0]?.versions[0]?.version_id;
    if (!id) throw new Error("active Worker Version is missing");
    const version = await client.workers.scripts.versions.get(id, {
      account_id: accountId,
      script_name: caller,
    });
    return {
      id,
      service: (version.resources.bindings ?? []).find(
        (item) => item.type === "service" && item.name === "TARGET",
      ),
    };
  };
  try {
    await signIn(page);
    await create(target);
    await create(replacement);
    await create(caller);
    await page.getByRole("tab", { name: "Settings" }).click();
    await page
      .getByRole("button", { name: "Add binding", exact: true })
      .click();
    const gallery = page.getByRole("dialog", { name: "Add binding" });
    await gallery.getByRole("option", { name: "Service binding" }).click();
    await gallery.getByRole("button", { name: "Add binding" }).click();
    const dialog = page.getByRole("dialog", {
      name: "Service binding",
      exact: true,
    });
    await dialog.getByRole("textbox", { name: "Binding name" }).fill("TARGET");
    const select = dialog.getByRole("combobox", {
      name: "Service",
      exact: true,
    });
    await expect(select.locator(`option[value="${caller}"]`)).toHaveCount(1);
    await select.selectOption(target);
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_SERVICE_BINDING === "1") {
      await page.setViewportSize({ width: 1808, height: 1169 });
      await dialog.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-service-binding-add-desktop.png",
        ),
      });
      await page.setViewportSize({ width: 390, height: 844 });
      await dialog.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-service-binding-add-mobile.png",
        ),
      });
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
        .toBe(390);
    }
    await dialog.getByRole("button", { name: "Deploy" }).click();
    await expect(dialog).toBeHidden();
    const added = await binding();
    expect(added.service).toMatchObject({ type: "service", service: target });
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_SERVICE_BINDING === "1")
      await expect(page.getByText("Service binding deployed.")).toBeHidden({
        timeout: 10_000,
      });

    await page.getByRole("button", { name: "Edit TARGET" }).click();
    const edit = page.getByRole("dialog", {
      name: "Service binding",
      exact: true,
    });
    await edit.getByRole("button", { name: "Advanced options" }).click();
    await edit
      .getByRole("textbox", { name: "Named entrypoint" })
      .fill("bad-name");
    await expect(edit.getByRole("button", { name: "Deploy" })).toBeDisabled();
    await edit.getByRole("textbox", { name: "Named entrypoint" }).fill("");
    await edit.getByRole("textbox", { name: "Service props" }).fill("[1]");
    await expect(edit.getByRole("button", { name: "Deploy" })).toBeDisabled();
    await edit
      .getByRole("textbox", { name: "Service props" })
      .fill('{"tenant":"example","options":{"enabled":true}}');
    await edit
      .getByRole("combobox", { name: "Service", exact: true })
      .selectOption(replacement);
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_SERVICE_BINDING === "1") {
      await page.setViewportSize({ width: 1808, height: 1169 });
      await edit.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-service-binding-advanced-desktop.png",
        ),
      });
      await page.setViewportSize({ width: 390, height: 844 });
      await edit.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-service-binding-advanced-mobile.png",
        ),
      });
      await edit.evaluate((element) => {
        element.scrollTop = element.scrollHeight;
      });
      await edit.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-service-binding-advanced-mobile-footer.png",
        ),
      });
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
        .toBe(390);
    }
    await edit.getByRole("button", { name: "Deploy" }).click();
    await expect(edit).toBeHidden();
    const edited = await binding();
    expect(edited.id).not.toBe(added.id);
    expect(edited.service).toMatchObject({
      type: "service",
      service: replacement,
      props: { tenant: "example", options: { enabled: true } },
    });

    await page.getByRole("button", { name: "Delete TARGET" }).click();
    const confirmation = page.getByRole("alertdialog", {
      name: "Delete Service binding?",
    });
    await confirmation
      .getByRole("button", { name: "Delete and deploy" })
      .click();
    await expect(confirmation).toBeHidden();
    const removed = await binding();
    expect(removed.id).not.toBe(edited.id);
    expect(removed.service).toBeUndefined();
  } finally {
    for (const name of created.reverse())
      await client.workers.scripts.delete(name, { account_id: accountId });
  }
});
