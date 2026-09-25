import { resolve } from "node:path";
import Cloudflare from "cloudflare";
import { expect, test } from "./fixtures";
import { adminToken, signIn } from "./helpers";

test.use({ actionTimeout: 15_000 });

test("Worker KV, R2, D1, Queue, Vectorize, and Images bindings deploy the exact saved version", async ({
  page,
}) => {
  test.setTimeout(150_000);
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
  const suffix = crypto.randomUUID().replaceAll("-", "");
  const worker = `pw-kv-binding-${suffix}`;
  const first = await client.kv.namespaces.create({
    account_id: accountId,
    title: `pw-kv-a-${suffix}`,
  });
  const second = await client.kv.namespaces.create({
    account_id: accountId,
    title: `pw-kv-b-${suffix}`,
  });
  const firstBucket = `pw-binding-r2-a-${suffix}`;
  const secondBucket = `pw-binding-r2-b-${suffix}`;
  const firstDatabase = await client.d1.database.create({
    account_id: accountId,
    name: `pw-binding-d1-a-${suffix}`,
  });
  const secondDatabase = await client.d1.database.create({
    account_id: accountId,
    name: `pw-binding-d1-b-${suffix}`,
  });
  if (!firstDatabase.uuid || !secondDatabase.uuid)
    throw new Error("D1 database IDs are missing");
  const firstQueue = await client.queues.create({
    account_id: accountId,
    queue_name: `pw-binding-queue-a-${suffix}`,
  });
  const secondQueue = await client.queues.create({
    account_id: accountId,
    queue_name: `pw-binding-queue-b-${suffix}`,
  });
  if (!firstQueue.queue_id || !secondQueue.queue_id)
    throw new Error("Queue IDs are missing");
  const firstIndex = `pw-binding-vector-a-${suffix}`;
  const secondIndex = `pw-binding-vector-b-${suffix}`;
  await client.vectorize.indexes.create({
    account_id: accountId,
    name: firstIndex,
    config: { dimensions: 5, metric: "cosine" },
  });
  await client.vectorize.indexes.create({
    account_id: accountId,
    name: secondIndex,
    config: { dimensions: 5, metric: "cosine" },
  });
  let bucketsCreated = 0;
  let workerCreated = false;
  const activeVersion = async () => {
    const deployments = await client.workers.scripts.deployments.list(worker, {
      account_id: accountId,
    });
    const id = deployments.deployments[0]?.versions[0]?.version_id;
    if (!id) throw new Error("active Worker Version is missing");
    return id;
  };
  const boundNamespace = async (versionId: string) => {
    const version = await client.workers.scripts.versions.get(versionId, {
      account_id: accountId,
      script_name: worker,
    });
    const binding = version.resources.bindings?.find(
      (item) => item.type === "kv_namespace" && item.name === "AUDIT_KV",
    );
    return binding?.type === "kv_namespace" ? binding.namespace_id : null;
  };
  const boundBucket = async (versionId: string) => {
    const version = await client.workers.scripts.versions.get(versionId, {
      account_id: accountId,
      script_name: worker,
    });
    const binding = version.resources.bindings?.find(
      (item) => item.type === "r2_bucket" && item.name === "AUDIT_R2",
    );
    return binding?.type === "r2_bucket" ? binding.bucket_name : null;
  };
  const boundDatabase = async (versionId: string) => {
    const version = await client.workers.scripts.versions.get(versionId, {
      account_id: accountId,
      script_name: worker,
    });
    const binding = version.resources.bindings?.find(
      (item) => item.type === "d1" && item.name === "AUDIT_D1",
    );
    return binding?.type === "d1" ? binding.database_id : null;
  };
  const boundQueue = async (versionId: string) => {
    const version = await client.workers.scripts.versions.get(versionId, {
      account_id: accountId,
      script_name: worker,
    });
    const binding = version.resources.bindings?.find(
      (item) => item.type === "queue" && item.name === "AUDIT_QUEUE",
    );
    return binding?.type === "queue" ? binding.queue_name : null;
  };
  const boundIndex = async (versionId: string) => {
    const version = await client.workers.scripts.versions.get(versionId, {
      account_id: accountId,
      script_name: worker,
    });
    const binding = version.resources.bindings?.find(
      (item) => item.type === "vectorize" && item.name === "AUDIT_VECTORIZE",
    );
    return binding?.type === "vectorize" ? binding.index_name : null;
  };
  const boundImages = async (versionId: string) => {
    const version = await client.workers.scripts.versions.get(versionId, {
      account_id: accountId,
      script_name: worker,
    });
    return (version.resources.bindings ?? [])
      .filter((item) => item.type === "images")
      .map((item) => item.name);
  };
  const openBinding = async (label: string) => {
    await page
      .getByRole("button", { name: "Add binding", exact: true })
      .click();
    const gallery = page.getByRole("dialog", { name: "Add binding" });
    await gallery.getByRole("option", { name: label, exact: true }).click();
    if (
      label === "Vectorize index" &&
      process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_KV_BINDING === "1"
    ) {
      await gallery.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-vectorize-binding-gallery-desktop.png",
        ),
      });
    }
    await gallery.getByRole("button", { name: "Add binding" }).click();
    await expect(gallery).toBeHidden();
  };
  try {
    await client.r2.buckets.create({
      account_id: accountId,
      name: firstBucket,
    });
    bucketsCreated = 1;
    await client.r2.buckets.create({
      account_id: accountId,
      name: secondBucket,
    });
    bucketsCreated = 2;
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
    workerCreated = true;
    const initialVersion = await activeVersion();
    await page.getByRole("tab", { name: "Settings" }).click();
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_KV_BINDING === "1") {
      await page.setViewportSize({ width: 1808, height: 1169 });
    }
    await page
      .getByRole("button", { name: "Add binding", exact: true })
      .click();
    const gallery = page.getByRole("dialog", { name: "Add binding" });
    await expect(gallery.getByRole("option")).toHaveCount(13);
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_KV_BINDING === "1") {
      await gallery.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-binding-gallery-desktop.png",
        ),
      });
      await page.setViewportSize({ width: 390, height: 844 });
      await gallery.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-binding-gallery-mobile.png",
        ),
      });
      await page.setViewportSize({ width: 1808, height: 1169 });
    }
    await gallery.getByRole("button", { name: "Add binding" }).click();
    await expect(gallery).toBeHidden();
    const dialog = page.getByRole("dialog", {
      name: "Add KV namespace binding",
    });
    await dialog
      .getByRole("textbox", { name: "Binding name" })
      .fill("AUDIT_KV");
    await dialog
      .getByRole("combobox", { name: "KV namespace", exact: true })
      .click();
    await page.getByRole("option", { name: `pw-kv-a-${suffix}` }).click();
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_KV_BINDING === "1") {
      await dialog.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-kv-binding-add-final-desktop.png",
        ),
      });
    }
    await dialog.getByRole("button", { name: "Add binding" }).click();
    await expect(dialog).toBeHidden();
    await expect(
      page.getByRole("link", { name: `pw-kv-a-${suffix}` }),
    ).toBeVisible();
    const addedVersion = await activeVersion();
    expect(addedVersion).not.toBe(initialVersion);
    expect(await boundNamespace(addedVersion)).toBe(first.id);
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_KV_BINDING === "1") {
      await page.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-kv-binding-table-final-desktop.png",
        ),
        fullPage: true,
      });
    }

    await page.getByRole("button", { name: "Edit AUDIT_KV" }).click();
    const edit = page.getByRole("dialog", { name: "KV namespace" });
    await expect(edit.getByRole("button", { name: "Deploy" })).toBeDisabled();
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_KV_BINDING === "1") {
      await edit.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-kv-binding-edit-final-desktop.png",
        ),
      });
      await page.setViewportSize({ width: 390, height: 844 });
      await edit.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-kv-binding-edit-final-mobile.png",
        ),
      });
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
        .toBe(390);
      await page.setViewportSize({ width: 1808, height: 1169 });
    }
    await edit
      .getByRole("combobox", { name: "KV namespace", exact: true })
      .selectOption(second.id);
    await edit.getByRole("button", { name: "Deploy" }).click();
    await expect(edit).toBeHidden();
    const editedVersion = await activeVersion();
    expect(editedVersion).not.toBe(addedVersion);
    expect(await boundNamespace(editedVersion)).toBe(second.id);

    await page.getByRole("button", { name: "Delete AUDIT_KV" }).click();
    const confirmation = page.getByRole("alertdialog", {
      name: "Delete KV binding?",
    });
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_KV_BINDING === "1") {
      await expect
        .poll(() =>
          confirmation.evaluate((element) => getComputedStyle(element).opacity),
        )
        .toBe("1");
      await confirmation.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-kv-binding-delete-final-desktop.png",
        ),
      });
    }
    await confirmation
      .getByRole("button", { name: "Delete and deploy" })
      .click();
    await expect(confirmation).toBeHidden();
    await expect(
      page.getByRole("button", { name: "Edit AUDIT_KV" }),
    ).toHaveCount(0);
    const deletedVersion = await activeVersion();
    expect(deletedVersion).not.toBe(editedVersion);
    expect(await boundNamespace(deletedVersion)).toBeNull();
    expect(
      (await client.kv.namespaces.get(first.id, { account_id: accountId })).id,
    ).toBe(first.id);

    await openBinding("R2 bucket");
    const r2 = page.getByRole("dialog", { name: "R2 bucket", exact: true });
    await expect(r2.getByRole("button", { name: "Deploy" })).toBeDisabled();
    await r2.getByRole("textbox", { name: "Binding name" }).fill("AUDIT_R2");
    await r2
      .getByRole("combobox", { name: "R2 bucket" })
      .selectOption(firstBucket);
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_KV_BINDING === "1") {
      await r2.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-r2-binding-add-desktop.png",
        ),
      });
      await page.setViewportSize({ width: 390, height: 844 });
      await r2.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-r2-binding-add-mobile.png",
        ),
      });
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
        .toBe(390);
      await page.setViewportSize({ width: 1808, height: 1169 });
    }
    await r2.getByRole("button", { name: "Deploy" }).click();
    await expect(r2).toBeHidden();
    await expect(page.getByRole("link", { name: firstBucket })).toBeVisible();
    const r2AddedVersion = await activeVersion();
    expect(await boundBucket(r2AddedVersion)).toBe(firstBucket);

    await page.getByRole("button", { name: "Edit AUDIT_R2" }).click();
    await r2
      .getByRole("combobox", { name: "R2 bucket" })
      .selectOption(secondBucket);
    await r2.getByRole("button", { name: "Deploy" }).click();
    await expect(r2).toBeHidden();
    const r2EditedVersion = await activeVersion();
    expect(r2EditedVersion).not.toBe(r2AddedVersion);
    expect(await boundBucket(r2EditedVersion)).toBe(secondBucket);

    await page.getByRole("button", { name: "Delete AUDIT_R2" }).click();
    const r2Delete = page.getByRole("alertdialog", {
      name: "Delete R2 binding?",
    });
    await r2Delete.getByRole("button", { name: "Delete and deploy" }).click();
    await expect(r2Delete).toBeHidden();
    expect(await boundBucket(await activeVersion())).toBeNull();
    expect(
      (await client.r2.buckets.get(firstBucket, { account_id: accountId }))
        .name,
    ).toBe(firstBucket);

    await openBinding("D1 database");
    const d1 = page.getByRole("dialog", {
      name: "Add D1 database binding",
    });
    await d1.getByRole("textbox", { name: "Binding name" }).fill("AUDIT_D1");
    await d1
      .getByRole("combobox", { name: "D1 database", exact: true })
      .selectOption(firstDatabase.uuid);
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_KV_BINDING === "1") {
      await d1.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-d1-binding-add-desktop.png",
        ),
      });
      await page.setViewportSize({ width: 390, height: 844 });
      await d1.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-d1-binding-add-mobile.png",
        ),
      });
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
        .toBe(390);
      await page.setViewportSize({ width: 1808, height: 1169 });
    }
    await d1.getByRole("button", { name: "Add binding" }).click();
    await expect(d1).toBeHidden();
    const d1AddedVersion = await activeVersion();
    expect(await boundDatabase(d1AddedVersion)).toBe(firstDatabase.uuid);

    await page.getByRole("button", { name: "Edit AUDIT_D1" }).click();
    const d1Edit = page.getByRole("dialog", {
      name: "Edit D1 database binding",
    });
    await d1Edit
      .getByRole("combobox", { name: "D1 database", exact: true })
      .selectOption(secondDatabase.uuid);
    await d1Edit.getByRole("button", { name: "Deploy" }).click();
    await expect(d1Edit).toBeHidden();
    const d1EditedVersion = await activeVersion();
    expect(d1EditedVersion).not.toBe(d1AddedVersion);
    expect(await boundDatabase(d1EditedVersion)).toBe(secondDatabase.uuid);

    await page.getByRole("button", { name: "Delete AUDIT_D1" }).click();
    const d1Delete = page.getByRole("alertdialog", {
      name: "Delete D1 binding?",
    });
    await d1Delete.getByRole("button", { name: "Delete and deploy" }).click();
    await expect(d1Delete).toBeHidden();
    expect(await boundDatabase(await activeVersion())).toBeNull();
    expect(
      (
        await client.d1.database.get(firstDatabase.uuid, {
          account_id: accountId,
        })
      ).uuid,
    ).toBe(firstDatabase.uuid);

    await openBinding("Queue");
    const queueDialog = page.getByRole("dialog", {
      name: "Add Queue binding",
    });
    await queueDialog
      .getByRole("textbox", { name: "Binding name" })
      .fill("AUDIT_QUEUE");
    await queueDialog
      .getByRole("combobox", { name: "Queue", exact: true })
      .selectOption(firstQueue.queue_name);
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_KV_BINDING === "1") {
      await queueDialog.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-queue-binding-add-desktop.png",
        ),
      });
      await page.setViewportSize({ width: 390, height: 844 });
      await queueDialog.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-queue-binding-add-mobile.png",
        ),
      });
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
        .toBe(390);
      await page.setViewportSize({ width: 1808, height: 1169 });
    }
    await queueDialog.getByRole("button", { name: "Add binding" }).click();
    await expect(queueDialog).toBeHidden();
    const queueAddedVersion = await activeVersion();
    expect(await boundQueue(queueAddedVersion)).toBe(firstQueue.queue_name);

    await page.getByRole("button", { name: "Edit AUDIT_QUEUE" }).click();
    const queueEdit = page.getByRole("dialog", {
      name: "Edit Queue binding",
    });
    await queueEdit
      .getByRole("combobox", { name: "Queue", exact: true })
      .selectOption(secondQueue.queue_name);
    await queueEdit.getByRole("button", { name: "Deploy" }).click();
    await expect(queueEdit).toBeHidden();
    const queueEditedVersion = await activeVersion();
    expect(queueEditedVersion).not.toBe(queueAddedVersion);
    expect(await boundQueue(queueEditedVersion)).toBe(secondQueue.queue_name);

    await page.getByRole("button", { name: "Delete AUDIT_QUEUE" }).click();
    const queueDelete = page.getByRole("alertdialog", {
      name: "Delete Queue binding?",
    });
    await queueDelete
      .getByRole("button", { name: "Delete and deploy" })
      .click();
    await expect(queueDelete).toBeHidden();
    expect(await boundQueue(await activeVersion())).toBeNull();
    expect(
      (await client.queues.get(firstQueue.queue_id, { account_id: accountId }))
        .queue_name,
    ).toBe(firstQueue.queue_name);

    await openBinding("Vectorize index");
    const vectorize = page.getByRole("dialog", {
      name: "Add Vectorize index binding",
    });
    await vectorize
      .getByRole("textbox", { name: "Binding name" })
      .fill("AUDIT_VECTORIZE");
    await vectorize
      .getByRole("combobox", { name: "Vectorize index", exact: true })
      .selectOption(firstIndex);
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_KV_BINDING === "1") {
      await vectorize.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-vectorize-binding-add-desktop.png",
        ),
      });
      await page.setViewportSize({ width: 390, height: 844 });
      await vectorize.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-vectorize-binding-add-mobile.png",
        ),
      });
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
        .toBe(390);
      await page.setViewportSize({ width: 1808, height: 1169 });
    }
    await vectorize.getByRole("button", { name: "Add binding" }).click();
    await expect(vectorize).toBeHidden();
    const vectorAddedVersion = await activeVersion();
    expect(await boundIndex(vectorAddedVersion)).toBe(firstIndex);

    await page.getByRole("button", { name: "Edit AUDIT_VECTORIZE" }).click();
    const vectorEdit = page.getByRole("dialog", {
      name: "Edit Vectorize index binding",
    });
    await vectorEdit
      .getByRole("combobox", { name: "Vectorize index", exact: true })
      .selectOption(secondIndex);
    await vectorEdit.getByRole("button", { name: "Deploy" }).click();
    await expect(vectorEdit).toBeHidden();
    const vectorEditedVersion = await activeVersion();
    expect(vectorEditedVersion).not.toBe(vectorAddedVersion);
    expect(await boundIndex(vectorEditedVersion)).toBe(secondIndex);

    await page.getByRole("button", { name: "Delete AUDIT_VECTORIZE" }).click();
    const vectorDelete = page.getByRole("alertdialog", {
      name: "Delete Vectorize index binding?",
    });
    await vectorDelete
      .getByRole("button", { name: "Delete and deploy" })
      .click();
    await expect(vectorDelete).toBeHidden();
    expect(await boundIndex(await activeVersion())).toBeNull();
    expect(
      (
        await client.vectorize.indexes.get(firstIndex, {
          account_id: accountId,
        })
      ).name,
    ).toBe(firstIndex);

    const beforeImages = await activeVersion();
    await openBinding("Images");
    const imagesDialog = page.getByRole("dialog", {
      name: "Add Images binding",
    });
    await imagesDialog.getByRole("button", { name: "Back" }).click();
    await expect(imagesDialog).toBeHidden();
    expect(await activeVersion()).toBe(beforeImages);
    const imagesGallery = page.getByRole("dialog", { name: "Add binding" });
    await imagesGallery.getByRole("button", { name: "Add binding" }).click();
    await expect(imagesGallery).toBeHidden();
    await imagesDialog
      .getByRole("textbox", { name: "Binding name" })
      .fill("AUDIT_IMAGES");
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_KV_BINDING === "1") {
      await imagesDialog.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-images-binding-add-desktop.png",
        ),
      });
      await page.setViewportSize({ width: 390, height: 844 });
      await imagesDialog.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-images-binding-add-mobile.png",
        ),
      });
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
        .toBe(390);
      await page.setViewportSize({ width: 1808, height: 1169 });
    }
    await imagesDialog.getByRole("button", { name: "Add binding" }).click();
    await expect(imagesDialog).toBeHidden();
    const imagesAddedVersion = await activeVersion();
    expect(imagesAddedVersion).not.toBe(beforeImages);
    expect(await boundImages(imagesAddedVersion)).toEqual(["AUDIT_IMAGES"]);

    await page.getByRole("button", { name: "Edit AUDIT_IMAGES" }).click();
    const imagesEdit = page.getByRole("dialog", {
      name: "Edit Images binding",
    });
    await imagesEdit
      .getByRole("textbox", { name: "Binding name" })
      .fill("AUDIT_IMAGES_2");
    await imagesEdit.getByRole("button", { name: "Deploy" }).click();
    await expect(imagesEdit).toBeHidden();
    const imagesEditedVersion = await activeVersion();
    expect(imagesEditedVersion).not.toBe(imagesAddedVersion);
    expect(await boundImages(imagesEditedVersion)).toEqual(["AUDIT_IMAGES_2"]);

    await page.getByRole("button", { name: "Delete AUDIT_IMAGES_2" }).click();
    const imagesDelete = page.getByRole("alertdialog", {
      name: "Delete Images binding?",
    });
    await imagesDelete
      .getByRole("button", { name: "Delete and deploy" })
      .click();
    await expect(imagesDelete).toBeHidden();
    expect(await boundImages(await activeVersion())).toEqual([]);
  } finally {
    if (workerCreated)
      await client.workers.scripts.delete(worker, { account_id: accountId });
    await client.kv.namespaces.delete(first.id, { account_id: accountId });
    await client.kv.namespaces.delete(second.id, { account_id: accountId });
    if (bucketsCreated >= 1)
      await client.r2.buckets.delete(firstBucket, { account_id: accountId });
    if (bucketsCreated >= 2)
      await client.r2.buckets.delete(secondBucket, { account_id: accountId });
    await client.d1.database.delete(firstDatabase.uuid, {
      account_id: accountId,
    });
    await client.d1.database.delete(secondDatabase.uuid, {
      account_id: accountId,
    });
    await client.queues.delete(firstQueue.queue_id, { account_id: accountId });
    await client.queues.delete(secondQueue.queue_id, { account_id: accountId });
    await client.vectorize.indexes.delete(firstIndex, {
      account_id: accountId,
    });
    await client.vectorize.indexes.delete(secondIndex, {
      account_id: accountId,
    });
  }
});
