import { resolve } from "node:path";
import Cloudflare from "cloudflare";
import { expect, test } from "./fixtures";
import { adminToken, signIn } from "./helpers";

test("Worker Queue trigger drafts, processing, save and removal", async ({
  page,
}) => {
  test.setTimeout(120_000);
  const dashboardRoot =
    process.env.OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL ??
    "http://127.0.0.1:8787/operator/";
  const client = new Cloudflare({
    apiToken: adminToken,
    baseURL: new URL("/client/v4", dashboardRoot).href,
    maxRetries: 0,
  });
  const account = (await client.accounts.list()).result[0]?.id;
  if (!account) throw new Error("Dashboard E2E account is missing");
  const suffix = crypto.randomUUID().replaceAll("-", "");
  const workerName = `pw-queue-trigger-worker-${suffix}`;
  const queueName = `pw-queue-trigger-${suffix}`;
  let queueId: string | undefined;
  try {
    await client.workers.scripts.update(workerName, {
      account_id: account,
      metadata: { main_module: "index.js", compatibility_date: "2026-09-08" },
      files: [
        new File(
          [
            "export default { fetch() { return new Response('ok'); }, async queue(batch) { for (const message of batch.messages) message.ack(); } };",
          ],
          "index.js",
          { type: "application/javascript+module" },
        ),
      ],
    });
    queueId = (
      await client.queues.create({ account_id: account, queue_name: queueName })
    ).queue_id;
    if (!queueId) throw new Error("Created Queue has no ID");
    await signIn(page);
    await page.goto(`./workers/${workerName}`);
    await page.getByRole("tab", { name: "Settings" }).click();
    const triggers = page.locator("#triggers");
    await expect(
      triggers.getByText("No Queue consumers configured."),
    ).toBeVisible();
    await triggers
      .getByRole("button", { name: "Add", exact: true })
      .last()
      .click();
    await expect(page.getByText("Unsaved changes")).toBeVisible();
    await page.getByRole("button", { name: "Discard" }).click();
    await expect(
      triggers.getByText("No Queue consumers configured."),
    ).toBeVisible();
    await triggers
      .getByRole("button", { name: "Add", exact: true })
      .last()
      .click();
    await triggers.getByRole("combobox", { name: "Queue" }).click();
    await page.getByRole("option", { name: queueName }).click();
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VISUAL === "1") {
      await page.setViewportSize({ width: 1808, height: 1169 });
      await triggers.scrollIntoViewIfNeeded();
      await page.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-queue-trigger-draft-desktop.png",
        ),
      });
      await page.setViewportSize({ width: 390, height: 844 });
      await page.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-queue-trigger-draft-mobile.png",
        ),
        fullPage: true,
      });
      const overflow = await page.evaluate(() => ({
        width: document.documentElement.scrollWidth,
        nodes: [...document.querySelectorAll("body *")]
          .filter((node) => node.getBoundingClientRect().right > 390)
          .slice(0, 10)
          .map((node) => ({
            tag: node.tagName,
            className: node.className?.toString().slice(0, 100),
            text: node.textContent?.slice(0, 30),
            right: node.getBoundingClientRect().right,
          })),
      }));
      expect(
        overflow.width,
        JSON.stringify(overflow.nodes),
      ).toBeLessThanOrEqual(390);
      await page.setViewportSize({ width: 1808, height: 1169 });
    }
    await triggers.getByRole("button", { name: "Message processing" }).click();
    const dialog = page.getByRole("dialog");
    await expect(dialog.getByText(queueName)).toBeVisible();
    await dialog.getByLabel("Message wait time (seconds)").fill("7");
    await dialog.getByLabel("Send to dead-letter queue").check();
    await expect(
      dialog.getByRole("combobox", { name: "Dead-letter queue" }),
    ).toBeVisible();
    await dialog.getByLabel("Drop permanently").check();
    if (process.env.OPEN_COMPUTE_DASHBOARD_CAPTURE_VISUAL === "1") {
      await page.screenshot({
        path: resolve(
          import.meta.dirname,
          "../../../../dashboard-refactor/implementation-screenshots/worker-queue-trigger-processing-desktop.png",
        ),
      });
    }
    await dialog.getByRole("button", { name: "Update" }).click();
    await page
      .getByText("Unsaved changes")
      .locator("..")
      .getByRole("button", { name: "Save", exact: true })
      .click();
    await expect(page.getByText("Unsaved changes")).toHaveCount(0);
    let list = await client.queues.consumers.list(queueId, {
      account_id: account,
    });
    expect(list.result).toHaveLength(1);
    expect(list.result[0]?.script_name).toBe(workerName);
    expect(list.result[0]?.settings?.max_wait_time_ms).toBe(7000);
    await triggers
      .getByRole("button", {
        name: new RegExp(`Remove Queue trigger ${queueId}`),
      })
      .click();
    await page
      .getByText("Unsaved changes")
      .locator("..")
      .getByRole("button", { name: "Save", exact: true })
      .click();
    await expect(
      triggers.getByText("No Queue consumers configured."),
    ).toBeVisible();
    list = await client.queues.consumers.list(queueId, { account_id: account });
    expect(list.result).toHaveLength(0);
  } finally {
    if (queueId) await client.queues.delete(queueId, { account_id: account });
    await client.workers.scripts.delete(workerName, { account_id: account });
  }
});
