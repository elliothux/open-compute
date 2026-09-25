import { resolve } from "node:path";
import Cloudflare from "cloudflare";
import { expect, test } from "./fixtures";
import { adminToken, signIn } from "./helpers";

test("Workers AI binding deploys only the supported built-in capability", async ({
  page,
}) => {
  test.setTimeout(90_000);
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
  const worker = `pw-ai-binding-${crypto.randomUUID().slice(0, 8)}`;
  let workerCreated = false;
  const activeVersion = async () => {
    const deployments = await client.workers.scripts.deployments.list(worker, {
      account_id: accountId,
    });
    const id = deployments.deployments[0]?.versions[0]?.version_id;
    if (!id) throw new Error("active Worker Version is missing");
    const version = await client.workers.scripts.versions.get(id, {
      account_id: accountId,
      script_name: worker,
    });
    return { id, bindings: version.resources.bindings ?? [] };
  };
  const openBinding = async (kind: string) => {
    await page
      .getByRole("button", { name: "Add binding", exact: true })
      .click();
    const gallery = page.getByRole("dialog", {
      name: "Add binding",
      exact: true,
    });
    await gallery.getByRole("option", { name: kind, exact: true }).click();
    await gallery.getByRole("button", { name: "Add binding" }).click();
    await expect(gallery).toBeHidden();
  };
  try {
    await signIn(page);
    await page.goto("./workers");
    await page.getByRole("button", { name: "Create Worker" }).first().click();
    await page.getByRole("button", { name: "Start with Hello World!" }).click();
    await page.getByLabel("Worker name").fill(worker);
    await page.getByRole("button", { name: "Deploy", exact: true }).click();
    await expect(
      page.getByRole("heading", { name: worker, level: 1 }),
    ).toBeVisible();
    workerCreated = true;
    await page.getByRole("tab", { name: "Settings" }).click();

    await openBinding("Images");
    const images = page.getByRole("dialog", { name: "Add Images binding" });
    await images.getByRole("textbox", { name: "Binding name" }).fill("IMAGES");
    await images.getByRole("button", { name: "Add binding" }).click();
    await expect(images).toBeHidden();
    const sibling = await activeVersion();

    await openBinding("Workers AI");
    const dialog = page.getByRole("dialog", {
      name: "Add Workers AI binding",
      exact: true,
    });
    await expect(
      dialog.getByRole("button", { name: "Add binding" }),
    ).toBeDisabled();
    await dialog
      .getByRole("textbox", { name: "Binding name" })
      .fill("A".repeat(65));
    await expect(
      dialog.getByRole("button", { name: "Add binding" }),
    ).toBeDisabled();
    await dialog.getByRole("textbox", { name: "Binding name" }).fill("AI");
    await expect(dialog.getByText("Markdown Conversion only")).toBeVisible();
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_AI_BINDING === "1") {
      await page.setViewportSize({ width: 1808, height: 1169 });
      await dialog.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-ai-binding-add-desktop.png",
        ),
      });
      await page.setViewportSize({ width: 390, height: 844 });
      await dialog.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-ai-binding-add-mobile.png",
        ),
      });
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
        .toBe(390);
      await page.setViewportSize({ width: 1808, height: 1169 });
    }
    await dialog.getByRole("button", { name: "Add binding" }).click();
    await expect(dialog).toBeHidden();
    const added = await activeVersion();
    expect(added.id).not.toBe(sibling.id);
    expect(added.bindings).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ type: "images", name: "IMAGES" }),
        expect.objectContaining({ type: "ai", name: "AI" }),
      ]),
    );

    await page.getByRole("button", { name: "Edit AI", exact: true }).click();
    const edit = page.getByRole("dialog", {
      name: "Edit Workers AI binding",
      exact: true,
    });
    await edit.getByRole("textbox", { name: "Binding name" }).fill("CONVERTER");
    await edit.getByRole("button", { name: "Deploy" }).click();
    await expect(edit).toBeHidden();
    const edited = await activeVersion();
    expect(edited.id).not.toBe(added.id);
    expect(edited.bindings).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ type: "images", name: "IMAGES" }),
        expect.objectContaining({ type: "ai", name: "CONVERTER" }),
      ]),
    );
    expect(edited.bindings.some((item) => item.name === "AI")).toBe(false);

    await page.getByRole("button", { name: "Delete CONVERTER" }).click();
    const confirmation = page.getByRole("alertdialog", {
      name: "Delete Workers AI binding?",
    });
    await confirmation
      .getByRole("button", { name: "Delete and deploy" })
      .click();
    await expect(confirmation).toBeHidden();
    const removed = await activeVersion();
    expect(removed.id).not.toBe(edited.id);
    expect(removed.bindings.some((item) => item.type === "ai")).toBe(false);
    expect(removed.bindings).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ type: "images", name: "IMAGES" }),
      ]),
    );
  } finally {
    if (workerCreated)
      await client.workers.scripts.delete(worker, { account_id: accountId });
  }
});
