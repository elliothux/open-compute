import Cloudflare from "cloudflare";
import { createOpenComputeExtension } from "@open-compute/cloudflare-extension";
import { expect, test } from "@playwright/test";
import { adminToken, signIn } from "./helpers";

function liveClient() {
  const dashboardRoot = process.env.OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL ?? "http://127.0.0.1:8787/operator/";
  const cloudflare = new Cloudflare({
    apiToken: adminToken,
    baseURL: new URL("/client/v4", dashboardRoot).href,
    maxRetries: 0,
  });
  return { cloudflare, openCompute: createOpenComputeExtension(cloudflare) };
}
test.describe("Cloudflare v4 dashboard consumers", () => {
  test.beforeEach(async ({ page }) => {
    await signIn(page);
  });

  test("catalog navigation uses only the v4 management root", async ({ page }) => {
    const managementRequests: string[] = [];
    page.on("request", request => {
      if (request.url().includes("/client/")) {
        managementRequests.push(new URL(request.url()).pathname);
      }
    });
    for (const path of ["/", "/workers", "/kv", "/d1", "/r2", "/durable-objects", "/queues", "/workflows", "/platform"]) {
      await page.goto(new URL(path.slice(1), page.url()).href);
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
    }
    expect(managementRequests.length).toBeGreaterThan(0);
    expect(managementRequests.every(path => path.startsWith("/client/v4/"))).toBe(true);
  });

  test("official SDK and extension share authentication and transport", async () => {
    const { cloudflare, openCompute } = liveClient();
    const accounts = await cloudflare.accounts.list({ per_page: 2 });
    const accountID = accounts.result[0]?.id;
    expect(accountID).toBeTruthy();
    const capabilities = await openCompute.capabilities.get();
    expect(capabilities.wrangler_version).toBe("4.127.1");

    const title = `pw-kv-${Date.now()}`;
    const namespace = await cloudflare.kv.namespaces.create({ account_id: accountID!, title });
    try {
      const page = await cloudflare.kv.namespaces.list({ account_id: accountID! });
      expect(page.result.some(item => item.id === namespace.id && item.title === title)).toBe(true);
    } finally {
      await cloudflare.kv.namespaces.delete(namespace.id, { account_id: accountID! });
    }
  });
});
