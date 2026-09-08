import Cloudflare from "cloudflare";
import { expect, test } from "./fixtures";
import { adminToken, expectNoLoadErrors, signIn } from "./helpers";

function liveClient() {
  const dashboardRoot = process.env.OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL ?? "http://127.0.0.1:8787/operator/";
  return new Cloudflare({
    apiToken: adminToken,
    baseURL: new URL("/client/v4", dashboardRoot).href,
    maxRetries: 0,
  });
}

async function accountId(client: Cloudflare): Promise<string> {
  const accounts = await client.accounts.list();
  const id = accounts.result[0]?.id;
  if (id === undefined) throw new Error("dashboard E2E account is missing");
  return id;
}

test.describe("operator dashboard detail pages", () => {
  test("Worker detail opens for a Worker uploaded through the official SDK", async ({ page }) => {
    const client = liveClient();
    const accountID = await accountId(client);
    const name = `pw-worker-${Date.now()}`;
    await client.workers.scripts.update(name, {
      account_id: accountID,
      metadata: {
        main_module: "index.js",
        compatibility_date: "2026-08-30",
      },
      files: [new File([
        "export default { fetch() { return new Response('dashboard e2e'); } };",
      ], "index.js", { type: "application/javascript+module" })],
    });
    try {
      await signIn(page);
      await page.getByRole("navigation").getByRole("link", { name: "Workers", exact: true }).click();
      await page.getByRole("link", { name, exact: true }).click();
      await expect(page).toHaveURL(/\/operator\/workers\//);
      await expect(page.getByRole("heading", { name, level: 1 })).toBeVisible();
      await expectNoLoadErrors(page);
      await expect(page.getByRole("button", { name: "Delete Worker" })).toBeVisible();
    } finally {
      await client.workers.scripts.delete(name, { account_id: accountID });
    }
  });

  test("KV namespace detail opens from the catalog name", async ({ page }) => {
    const client = liveClient();
    const accountID = await accountId(client);
    const name = `PW_KV_${Date.now()}`;
    const namespace = await client.kv.namespaces.create({ account_id: accountID, title: name });
    try {
      await signIn(page);
      await page.getByRole("navigation").getByRole("link", { name: "KV", exact: true }).click();
      await page.getByRole("link", { name, exact: true }).click();
      await expect(page).toHaveURL(/\/operator\/kv\//);
      await expectNoLoadErrors(page);
      await expect(page.getByRole("heading", { name: "KV namespace", level: 1 })).toBeVisible();
      await expect(page.getByRole("heading", { name: "Put value", level: 2 })).toBeVisible();
    } finally {
      await client.kv.namespaces.delete(namespace.id, { account_id: accountID });
    }
  });
});
