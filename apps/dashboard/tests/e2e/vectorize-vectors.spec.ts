import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import Cloudflare from "cloudflare";
import { expect, test } from "./fixtures";
import { adminToken, signIn } from "./helpers";

test("Vectorize vector upload, query, inspect and delete", async ({ page }) => {
  const root =
    process.env.OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL ??
    "http://127.0.0.1:8787/operator/";
  const api = new Cloudflare({
    apiToken: adminToken,
    baseURL: new URL("/client/v4", root).href,
    maxRetries: 0,
  });
  const accountId = (await api.accounts.list()).result[0]?.id;
  if (!accountId) throw new Error("Missing isolated test account.");
  const name = `pw-vectorize-${crypto.randomUUID().slice(0, 8)}`;
  const capture = process.env.OPEN_COMPUTE_CAPTURE_VECTORIZE === "1";
  const screenshots = resolve(
    import.meta.dirname,
    "../../../../dashboard-refactor/implementation-screenshots",
  );
  if (capture) {
    await mkdir(screenshots, { recursive: true });
    await page.setViewportSize({ width: 1440, height: 900 });
  }
  await api.vectorize.indexes.create({
    account_id: accountId,
    name,
    config: { dimensions: 3, metric: "cosine" },
  });
  try {
    await signIn(page);
    if (capture) {
      await page.goto("./vectorize");
      await expect(
        page.getByRole("heading", { name: "Vectorize", level: 1 }),
      ).toBeVisible();
      await page.getByRole("button", { name: "Create index" }).first().click();
      await expect(
        page
          .getByRole("dialog")
          .getByRole("heading", { name: "Create Vectorize index" }),
      ).toBeVisible();
      await page.waitForFunction(() => {
        const dialog = document.querySelector('[role="dialog"]');
        return (
          dialog !== null && Number(getComputedStyle(dialog).opacity) >= 0.99
        );
      });
      await page.screenshot({
        path: resolve(screenshots, "vectorize-create-desktop.png"),
      });
      await page
        .getByRole("dialog")
        .getByRole("button", { name: "Cancel" })
        .click();
      await page.goto(`./vectorize/${name}?tab=overview`);
      await expect(page.getByText("Index details")).toBeVisible();
      await page.waitForLoadState("networkidle");
      await page.screenshot({
        path: resolve(screenshots, "vectorize-overview-desktop.png"),
        fullPage: true,
      });
    }
    await page.goto(`./vectorize/${name}?tab=metadata`);
    await expect(page.getByText("No metadata indexes")).toBeVisible();
    await page.getByRole("button", { name: "Create metadata index" }).click();
    await expect(
      page
        .getByRole("dialog")
        .getByRole("heading", { name: "Create metadata index" }),
    ).toBeVisible();
    if (capture) {
      await page.waitForFunction(() => {
        const dialog = document.querySelector('[role="dialog"]');
        return (
          dialog !== null && Number(getComputedStyle(dialog).opacity) >= 0.99
        );
      });
      await page.screenshot({
        path: resolve(screenshots, "vectorize-metadata-create-desktop.png"),
      });
    }
    await page
      .getByRole("dialog")
      .getByRole("textbox", { name: "Property name" })
      .fill("category");
    await page
      .getByRole("dialog")
      .getByRole("button", { name: "Create" })
      .click();
    await expect(page.getByText("category")).toBeVisible();
    if (capture) {
      await expect(
        page.getByRole("dialog", { name: "Create metadata index" }),
      ).toBeHidden();
      await expect(
        page.getByRole("dialog", { name: "Metadata index created." }),
      ).toBeHidden({ timeout: 10_000 });
      await page.screenshot({
        path: resolve(screenshots, "vectorize-metadata-desktop.png"),
        fullPage: true,
      });
    }
    await page.goto(`./vectorize/${name}?tab=vectors`);
    await expect(page.getByRole("heading", { name, level: 1 })).toBeVisible();
    await expect(page.getByText("No vectors")).toBeVisible();
    await page.getByRole("button", { name: "Upload vectors" }).click();
    await expect(
      page.getByRole("dialog").getByRole("heading", { name: "Upload vectors" }),
    ).toBeVisible();
    if (capture) {
      await page.waitForFunction(() => {
        const dialog = document.querySelector('[role="dialog"]');
        return (
          dialog !== null && Number(getComputedStyle(dialog).opacity) >= 0.99
        );
      });
      await page.screenshot({
        path: resolve(screenshots, "vectorize-upload-desktop.png"),
      });
    }
    await page.locator('input[type="file"]').setInputFiles({
      name: "vectors.ndjson",
      mimeType: "application/x-ndjson",
      buffer: Buffer.from(
        `${JSON.stringify({ id: "reference-1", values: [0.1, 0.2, 0.3], metadata: { category: "docs" } })}\n`,
      ),
    });
    await page
      .getByRole("dialog")
      .getByRole("button", { name: "Upload" })
      .click();
    await expect(page.getByText("Vector mutation queued.")).toBeVisible();
    await expect
      .poll(async () => {
        const result = await api.vectorize.indexes.listVectors(name, {
          account_id: accountId,
        });
        return result?.vectors.some((vector) => vector.id === "reference-1");
      })
      .toBe(true);
    await page.reload();
    await expect(page.getByText("reference-1")).toBeVisible();
    if (capture) {
      await page.screenshot({
        path: resolve(screenshots, "vectorize-vectors-desktop.png"),
        fullPage: true,
      });
      await page.setViewportSize({ width: 390, height: 844 });
      await page.screenshot({
        path: resolve(screenshots, "vectorize-vectors-mobile.png"),
        fullPage: true,
      });
      await page.setViewportSize({ width: 1440, height: 900 });
    }
    await page.getByRole("button", { name: "View" }).click();
    await expect(
      page.getByRole("dialog").getByRole("heading", { name: "reference-1" }),
    ).toBeVisible();
    await expect(page.getByRole("dialog").getByText("category")).toBeVisible();
    if (capture) {
      await page.waitForFunction(() => {
        const dialog = document.querySelector('[role="dialog"]');
        return (
          dialog !== null && Number(getComputedStyle(dialog).opacity) >= 0.99
        );
      });
      await page.screenshot({
        path: resolve(screenshots, "vectorize-vector-detail-desktop.png"),
      });
    }
    await page
      .getByRole("dialog")
      .getByRole("button", { name: "Close" })
      .click();
    await page.getByRole("button", { name: "Upload vectors" }).click();
    await page
      .getByRole("dialog")
      .getByLabel("Operation")
      .selectOption("upsert");
    await page.locator('input[type="file"]').setInputFiles({
      name: "updated.ndjson",
      mimeType: "application/x-ndjson",
      buffer: Buffer.from(
        `${JSON.stringify({ id: "reference-1", values: [0.1, 0.2, 0.3], metadata: { category: "updated" } })}\n`,
      ),
    });
    await page
      .getByRole("dialog")
      .getByRole("button", { name: "Upload" })
      .click();
    await expect
      .poll(async () =>
        JSON.stringify(
          await api.vectorize.indexes.getByIDs(name, {
            account_id: accountId,
            ids: ["reference-1"],
          }),
        ),
      )
      .toContain("updated");
    await page.getByRole("textbox", { name: "Vector" }).fill("0.1, 0.2, 0.3");
    await page.getByRole("spinbutton", { name: "Top K" }).fill("1");
    await page
      .getByRole("textbox", { name: "Metadata filter (JSON)" })
      .fill('{"category":"updated"}');
    await page.getByRole("checkbox", { name: "Return vector values" }).check();
    await page.getByRole("button", { name: "Run query" }).click();
    await expect(page.getByText(/"id": "reference-1"/)).toBeVisible();
    await expect(page.getByText(/"category": "updated"/)).toBeVisible();
    if (capture) {
      await expect(
        page.getByRole("dialog", { name: "Vector mutation queued." }),
      ).toBeHidden({ timeout: 10_000 });
      await page.screenshot({
        path: resolve(screenshots, "vectorize-query-desktop.png"),
        fullPage: true,
      });
    }
    await page.getByRole("button", { name: "View" }).click();
    await page
      .getByRole("dialog")
      .getByRole("button", { name: "Delete" })
      .click();
    await expect(
      page.getByRole("alertdialog", { name: "Delete reference-1" }),
    ).toBeVisible();
    await page
      .getByRole("alertdialog")
      .getByRole("textbox", { name: "Resource name" })
      .fill("reference-1");
    if (capture) {
      await page.waitForFunction(() => {
        const dialog = document.querySelector('[role="alertdialog"]');
        return (
          dialog !== null && Number(getComputedStyle(dialog).opacity) >= 0.99
        );
      });
      await page.screenshot({
        path: resolve(screenshots, "vectorize-vector-delete-desktop.png"),
      });
    }
    await page
      .getByRole("alertdialog")
      .getByRole("button", { name: "Delete" })
      .click();
    await expect(page.getByText("Vector deletion queued.")).toBeVisible();
    await expect
      .poll(async () => {
        const result = await api.vectorize.indexes.listVectors(name, {
          account_id: accountId,
        });
        return result?.vectors.length;
      })
      .toBe(0);
    await page.goto(`./vectorize/${name}?tab=metadata`);
    await expect(page.getByText("category")).toBeVisible();
    await page.getByRole("button", { name: "Delete" }).last().click();
    await expect(page.getByText("No metadata indexes")).toBeVisible();
  } finally {
    await api.vectorize.indexes.delete(name, { account_id: accountId });
  }
});
