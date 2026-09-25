import { resolve } from "node:path";
import Cloudflare from "cloudflare";
import { expect, test } from "./fixtures";
import { adminToken, signIn } from "./helpers";

test("Dynamic Workers binding adds, edits and deletes an exact deployed Version", async ({
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
  const worker = `pw-worker-loader-${crypto.randomUUID().slice(0, 8)}`;
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
    const gallery = page.getByRole("dialog", { name: "Add binding" });
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

    await openBinding("Dynamic Workers");
    const drawer = page.getByRole("dialog", {
      name: "Dynamic Workers",
      exact: true,
    });
    await expect(drawer.getByRole("button", { name: "Deploy" })).toBeDisabled();
    await drawer
      .getByRole("textbox", { name: "Binding name" })
      .fill("A".repeat(65));
    await expect(drawer.getByRole("button", { name: "Deploy" })).toBeDisabled();
    await drawer.getByRole("textbox", { name: "Binding name" }).fill("LOADER");
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_WORKER_LOADER === "1") {
      await page.setViewportSize({ width: 1808, height: 1169 });
      await page.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-dynamic-workers-binding-add-desktop.png",
        ),
      });
      await page.setViewportSize({ width: 390, height: 844 });
      await page.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-dynamic-workers-binding-add-mobile.png",
        ),
      });
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
        .toBe(390);
      await page.setViewportSize({ width: 1808, height: 1169 });
    }
    await drawer.getByRole("button", { name: "Deploy" }).click();
    await expect(drawer).toBeHidden();
    const added = await activeVersion();
    expect(added.id).not.toBe(sibling.id);
    expect(added.bindings).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ type: "images", name: "IMAGES" }),
        expect.objectContaining({ type: "worker_loader", name: "LOADER" }),
      ]),
    );

    await page.getByRole("button", { name: "Edit LOADER" }).click();
    const edit = page.getByRole("dialog", {
      name: "Dynamic Workers",
      exact: true,
    });
    await edit
      .getByRole("textbox", { name: "Binding name" })
      .fill("NEXT_LOADER");
    await edit.getByRole("button", { name: "Deploy" }).click();
    await expect(edit).toBeHidden();
    const edited = await activeVersion();
    expect(edited.id).not.toBe(added.id);
    expect(edited.bindings).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ type: "images", name: "IMAGES" }),
        expect.objectContaining({ type: "worker_loader", name: "NEXT_LOADER" }),
      ]),
    );
    expect(edited.bindings.some((item) => item.name === "LOADER")).toBe(false);

    await page.getByRole("button", { name: "Delete NEXT_LOADER" }).click();
    const confirmation = page.getByRole("alertdialog", {
      name: "Delete Dynamic Workers binding?",
    });
    await confirmation
      .getByRole("button", { name: "Delete and deploy" })
      .click();
    await expect(confirmation).toBeHidden();
    const removed = await activeVersion();
    expect(removed.id).not.toBe(edited.id);
    expect(removed.bindings.some((item) => item.type === "worker_loader")).toBe(
      false,
    );
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
