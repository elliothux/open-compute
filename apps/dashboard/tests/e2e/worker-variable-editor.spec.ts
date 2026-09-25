import { resolve } from "node:path";
import { expect, test } from "./fixtures";
import { signIn } from "./helpers";

test("Worker Production variables add, edit, reject duplicate, preserve secret, and delete", async ({
  page,
}) => {
  test.setTimeout(120_000);
  const worker = `pw-var-${crypto.randomUUID().replaceAll("-", "")}`;
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
    await page.getByRole("tab", { name: "Settings" }).click();
    await expect(
      page.getByRole("button", { name: "Add variable", exact: true }),
    ).toBeVisible();
    await page.getByRole("button", { name: "Add secret" }).click();
    const secretDialog = page.getByRole("dialog", { name: "Add secret" });
    await secretDialog.getByLabel("Variable name").fill("UNCHANGED_SECRET");
    await secretDialog.getByLabel("Secret value").fill("do-not-display-this");
    await secretDialog.getByRole("button", { name: "Save" }).click();
    await expect(page.getByText("UNCHANGED_SECRET")).toBeVisible();

    await page
      .getByRole("button", { name: "Add variable", exact: true })
      .click();
    const dialog = page.getByRole("dialog", {
      name: /^(Add a variable|Edit variable)/,
    });
    await expect(
      dialog.getByRole("heading", { name: "Add a variable" }),
    ).toBeVisible();
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VARIABLES === "1") {
      await page.setViewportSize({ width: 1808, height: 900 });
      await expect(dialog).not.toHaveAttribute("data-starting-style", "");
      await dialog.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-variable-add-desktop.png",
        ),
      });
      await expect(
        page.getByRole("dialog", { name: "Secret saved." }),
      ).toHaveCount(0, { timeout: 15_000 });
      await page.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-variable-add-page-desktop.png",
        ),
      });
    }
    await dialog.getByLabel("Variable name").fill("GREETING");
    await dialog.getByLabel("Value", { exact: true }).fill("hello");
    await dialog.getByRole("button", { name: "Save version" }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByText("hello", { exact: true })).toBeVisible();
    await expect(page.getByText("UNCHANGED_SECRET")).toBeVisible();
    await expect(page.getByText("do-not-display-this")).toHaveCount(0);

    await page
      .getByRole("button", { name: "Add variable", exact: true })
      .click();
    await dialog.getByLabel("Variable name").fill("EDITOR_SECRET");
    await dialog.getByRole("checkbox", { name: "Secret" }).check();
    await dialog.getByLabel("Secret value").fill("another-hidden-value");
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VARIABLES === "1") {
      await dialog.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-variable-secret-draft-desktop.png",
        ),
      });
    }
    await dialog.getByRole("button", { name: "Save version" }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByText("EDITOR_SECRET")).toBeVisible();
    await expect(page.getByText("another-hidden-value")).toHaveCount(0);

    await page
      .getByRole("button", { name: "Add variable", exact: true })
      .click();
    await dialog.getByLabel("Variable name").fill("GREETING");
    await dialog.getByLabel("Value", { exact: true }).fill("duplicate");
    await expect(
      dialog.getByText("A binding with this name already exists."),
    ).toBeVisible();
    await expect(
      dialog.getByRole("button", { name: "Save version" }),
    ).toBeDisabled();
    await dialog.getByRole("button", { name: "Cancel" }).click();
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VARIABLES === "1") {
      const confirmation = page.getByRole("alertdialog");
      await expect(confirmation).not.toHaveAttribute("data-starting-style", "");
      await confirmation.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-variable-discard-desktop.png",
        ),
      });
    }
    await page
      .getByRole("alertdialog")
      .getByRole("button", { name: "Discard" })
      .click();
    await expect(dialog).toBeHidden();

    await page.getByRole("button", { name: "Edit GREETING" }).click();
    await expect(
      dialog.getByRole("heading", {
        name: "Edit variable GREETING in Production",
      }),
    ).toBeVisible();
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VARIABLES === "1") {
      await expect(dialog).not.toHaveAttribute("data-starting-style", "");
      await dialog.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-variable-edit-desktop.png",
        ),
      });
    }
    await dialog.getByLabel("Value", { exact: true }).fill("updated");
    await dialog.getByRole("button", { name: "Save version" }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByText("updated", { exact: true })).toBeVisible();
    await expect(page.getByText("UNCHANGED_SECRET")).toBeVisible();
    await expect(page.getByText("EDITOR_SECRET")).toBeVisible();

    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VARIABLES === "1") {
      await expect(
        page.getByRole("dialog", { name: /Worker version saved/ }),
      ).toHaveCount(0, { timeout: 15_000 });
      await page.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-variable-row-desktop.png",
        ),
        fullPage: true,
      });
      await page.setViewportSize({ width: 390, height: 844 });
      await page
        .getByRole("button", { name: "Add variable", exact: true })
        .click();
      await expect(dialog).not.toHaveAttribute("data-starting-style", "");
      await dialog.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-variable-add-mobile.png",
        ),
      });
      await page.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-variable-add-page-mobile.png",
        ),
      });
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
        .toBe(390);
      await dialog.getByRole("button", { name: "Cancel" }).click();
      await expect(dialog).toBeHidden();
      await page.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-variable-row-mobile.png",
        ),
        fullPage: true,
      });
      await page.setViewportSize({ width: 1280, height: 720 });
    }
    await page.getByRole("button", { name: "Delete GREETING" }).click();
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VARIABLES === "1") {
      const confirmation = page.getByRole("alertdialog");
      await expect(confirmation).not.toHaveAttribute("data-starting-style", "");
      await confirmation.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-variable-delete-desktop.png",
        ),
      });
    }
    await page
      .getByRole("alertdialog")
      .getByRole("button", { name: "Delete in new version" })
      .click();
    await expect(
      page.getByRole("button", { name: "Edit GREETING" }),
    ).toHaveCount(0);
    await expect(page.getByText("UNCHANGED_SECRET")).toBeVisible();
  } finally {
    await page.setViewportSize({ width: 1280, height: 720 });
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

test("Deleting the final binding saves an empty binding list", async ({
  page,
}) => {
  test.setTimeout(60_000);
  const worker = `pw-final-var-${crypto.randomUUID().replaceAll("-", "")}`;
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
    await page.getByRole("tab", { name: "Settings" }).click();
    await page
      .getByRole("button", { name: "Add variable", exact: true })
      .click();
    const dialog = page.getByRole("dialog", { name: "Add a variable" });
    await dialog.getByLabel("Variable name").fill("ONLY_BINDING");
    await dialog.getByLabel("Value", { exact: true }).fill("retained");
    await dialog.getByRole("button", { name: "Save version" }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByText("retained", { exact: true })).toBeVisible();

    await page.getByRole("button", { name: "Delete ONLY_BINDING" }).click();
    const confirmation = page.getByRole("alertdialog");
    await confirmation
      .getByRole("button", { name: "Delete in new version" })
      .click();
    await expect(confirmation).toBeHidden();
    await expect(page.getByText("retained", { exact: true })).toHaveCount(0);
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VARIABLES === "1") {
      await expect(
        page.getByRole("dialog", { name: /Worker version saved/ }),
      ).toHaveCount(0, { timeout: 15_000 });
      await page.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-variable-final-binding-deleted.png",
        ),
        fullPage: true,
      });
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
