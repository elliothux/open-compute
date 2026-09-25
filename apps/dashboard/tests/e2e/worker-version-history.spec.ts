import { resolve } from "node:path";
import { expect, test } from "./fixtures";
import { signIn } from "./helpers";

test("saved Worker Version stays offline until explicitly promoted", async ({
  page,
}) => {
  test.setTimeout(120_000);
  const worker = `pw-version-${crypto.randomUUID().replaceAll("-", "")}`;
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

  try {
    await page.getByRole("tab", { name: "Deployments" }).click();
    const history = page.locator("section").filter({
      has: page.getByRole("heading", { name: "Version history" }),
    });
    await expect(history.getByText("Active", { exact: true })).toHaveCount(1);
    const originalVersion = (await history
      .getByRole("button", { name: /^Copy version ID / })
      .first()
      .getAttribute("aria-label"))!.replace("Copy version ID ", "");

    await page.getByRole("tab", { name: "Settings" }).click();
    await page
      .getByRole("button", { name: "Add variable", exact: true })
      .click();
    const variableDialog = page.getByRole("dialog", { name: "Add a variable" });
    await variableDialog.getByLabel("Variable name").fill("VERSION_PROBE");
    await variableDialog
      .getByLabel("Value", { exact: true })
      .fill("saved-only");
    await variableDialog.getByRole("button", { name: "Save version" }).click();
    await expect(variableDialog).toBeHidden();

    await page.getByRole("tab", { name: "Deployments" }).click();
    await expect(history.getByText("Saved", { exact: true })).toHaveCount(1);
    await expect(
      history.getByText("Add variable: VERSION_PROBE"),
    ).toBeVisible();
    await expect(history.getByText("Active", { exact: true })).toHaveCount(1);
    const savedAction = history.getByRole("button", {
      name: /^Actions for version /,
    });
    const savedId = (await savedAction.getAttribute("aria-label"))!.replace(
      "Actions for version ",
      "",
    );
    expect(savedId).not.toBe(originalVersion);

    await savedAction.click();
    await page.getByRole("menuitem", { name: "Promote version" }).click();
    const promotion = page.getByRole("dialog", { name: "Promote version" });
    await expect(
      promotion.getByText(
        `${originalVersion.slice(0, 8)}…${originalVersion.slice(-6)}`,
      ),
    ).toBeVisible();
    await expect(
      promotion.getByText(`${savedId.slice(0, 8)}…${savedId.slice(-6)}`),
    ).toBeVisible();
    await promotion.getByRole("button", { name: "Cancel" }).click();
    await expect(promotion).toBeHidden();
    await expect(history.getByText("Saved", { exact: true })).toHaveCount(1);

    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VERSIONS === "1") {
      await page.setViewportSize({ width: 1808, height: 900 });
      await page.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-version-history-desktop.png",
        ),
        fullPage: true,
      });
    }

    await savedAction.click();
    await page.getByRole("menuitem", { name: "Promote version" }).click();
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VERSIONS === "1") {
      await promotion.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-version-promote-confirmation.png",
        ),
      });
    }
    const deploymentRequest = page.waitForRequest(
      (request) =>
        request.method() === "POST" &&
        request.url().endsWith(`/workers/scripts/${worker}/deployments`),
    );
    await promotion.getByRole("button", { name: "Promote version" }).click();
    const body = (await deploymentRequest).postDataJSON();
    expect(body.versions).toEqual([{ version_id: savedId, percentage: 100 }]);
    await expect(promotion).toBeHidden();
    await expect(
      history.getByRole("button", { name: `Actions for version ${savedId}` }),
    ).toHaveCount(0);
    await expect(history.getByText("Active", { exact: true })).toHaveCount(1);

    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VERSIONS === "1") {
      await page.setViewportSize({ width: 390, height: 844 });
      await expect(page.getByText("Version promoted.")).toBeHidden({
        timeout: 15_000,
      });
      await page.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-version-history-mobile.png",
        ),
        fullPage: true,
      });
      expect(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= window.innerWidth,
        ),
      ).toBe(true);
    }
  } finally {
    await page.goto("./workers");
    await page.getByRole("button", { name: `Delete ${worker}` }).click();
    await page
      .getByRole("alertdialog")
      .getByLabel("Resource name")
      .fill(worker);
    await page
      .getByRole("alertdialog")
      .getByRole("button", { name: "Delete", exact: true })
      .click();
  }
});
