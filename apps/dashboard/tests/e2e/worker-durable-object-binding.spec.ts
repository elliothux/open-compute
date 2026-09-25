import { resolve } from "node:path";
import { createOpenComputeClient } from "@open-compute/sdk";
import { expect, test } from "./fixtures";
import { adminToken, signIn } from "./helpers";

test("Durable Object binding deploys only classes owned by this Worker", async ({
  page,
}) => {
  test.setTimeout(120_000);
  const root =
    process.env.OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL ??
    "http://127.0.0.1:8787/operator/";
  const client = createOpenComputeClient({
    apiToken: adminToken,
    baseURL: new URL("/client/v4", root).href,
    maxRetries: 0,
  });
  const accountId = (await client.accounts.list()).result[0]?.id;
  if (!accountId) throw new Error("dashboard E2E account is missing");
  const worker = `pw-do-binding-${crypto.randomUUID().slice(0, 8)}`;
  const other = `pw-do-other-${crypto.randomUUID().slice(0, 8)}`;
  const create = (name: string, classes: string[]) =>
    client.workers.scripts.update(name, {
      account_id: accountId,
      metadata: {
        main_module: "index.js",
        compatibility_date: "2026-09-08",
        migrations: {
          new_tag: "v1",
          steps: [{ new_sqlite_classes: classes }],
        },
      },
      files: [
        new File(
          [
            `import { DurableObject } from 'cloudflare:workers';\n${classes.map((className) => `export class ${className} extends DurableObject { async fetch() { return new Response('ok'); } }`).join("\n")}\nexport default { fetch() { return new Response('ok'); } };`,
          ],
          "index.js",
          { type: "application/javascript+module" },
        ),
      ],
    });
  const active = async () => {
    const deployments = await client.workers.scripts.deployments.list(worker, {
      account_id: accountId,
    });
    const id = deployments.deployments[0]?.versions[0]?.version_id;
    if (!id) throw new Error("active Worker Version is missing");
    const version = await client.workers.scripts.versions.get(id, {
      account_id: accountId,
      script_name: worker,
    });
    return {
      id,
      binding: (version.resources.bindings ?? []).find(
        (item) =>
          item.type === "durable_object_namespace" && item.name === "OBJECTS",
      ),
    };
  };
  try {
    await create(other, ["External"]);
    await create(worker, ["Counter", "Other"]);
    const namespaces = (
      await client.openCompute.durableObjects.list(accountId, {
        query: { per_page: 100 },
      })
    ).items;
    const counter = namespaces.find(
      (item) => item.script_name === worker && item.class_name === "Counter",
    );
    const replacement = namespaces.find(
      (item) => item.script_name === worker && item.class_name === "Other",
    );
    if (!counter || !replacement)
      throw new Error("Durable Object namespaces were not created");

    await signIn(page);
    await page.goto(`./workers/${worker}`);
    await page.getByRole("tab", { name: "Settings" }).click();
    await page
      .getByRole("button", { name: "Add binding", exact: true })
      .click();
    const gallery = page.getByRole("dialog", { name: "Add binding" });
    await gallery.getByRole("option", { name: "Durable Object" }).click();
    await gallery.getByRole("button", { name: "Add binding" }).click();
    const dialog = page.getByRole("dialog").filter({
      has: page.getByRole("heading", { name: "Add Durable Object binding" }),
    });
    await dialog.getByRole("textbox", { name: "Binding name" }).fill("OBJECTS");
    const select = dialog.getByRole("combobox", {
      name: "Durable Object",
      exact: true,
    });
    await expect(select.locator('option[value="Counter"]')).toHaveCount(1);
    await expect(select.locator('option[value="Other"]')).toHaveCount(1);
    await expect(select.locator('option[value="External"]')).toHaveCount(0);
    await select.selectOption("Counter");
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_DO_BINDING === "1") {
      await page.setViewportSize({ width: 1808, height: 1169 });
      await dialog.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-do-binding-add-desktop.png",
        ),
      });
      await page.setViewportSize({ width: 390, height: 844 });
      await dialog.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-do-binding-add-mobile.png",
        ),
      });
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
        .toBe(390);
    }
    await dialog.getByRole("button", { name: "Add binding" }).click();
    await expect(dialog).toBeHidden();
    const added = await active();
    expect(added.binding).toMatchObject({
      class_name: "Counter",
      namespace_id: counter.id,
    });

    await page.getByRole("button", { name: "Edit OBJECTS" }).click();
    const edit = page.getByRole("dialog").filter({
      has: page.getByRole("heading", { name: "Edit Durable Object binding" }),
    });
    await edit
      .getByRole("combobox", { name: "Durable Object", exact: true })
      .selectOption("Other");
    await edit.getByRole("button", { name: "Deploy" }).click();
    await expect(edit).toBeHidden();
    const edited = await active();
    expect(edited.id).not.toBe(added.id);
    expect(edited.binding).toMatchObject({
      class_name: "Other",
      namespace_id: replacement.id,
    });

    await page.getByRole("button", { name: "Delete OBJECTS" }).click();
    const confirmation = page.getByRole("alertdialog", {
      name: "Delete Durable Object binding?",
    });
    await confirmation
      .getByRole("button", { name: "Delete and deploy" })
      .click();
    await expect(confirmation).toBeHidden();
    const removed = await active();
    expect(removed.id).not.toBe(edited.id);
    expect(removed.binding).toBeUndefined();
  } finally {
    await client.workers.scripts
      .delete(worker, { account_id: accountId })
      .catch(() => {});
    await client.workers.scripts
      .delete(other, { account_id: accountId })
      .catch(() => {});
  }
});
