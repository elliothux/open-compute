import { createOpenComputeClient } from "@open-compute/sdk";
import { expect, test } from "./fixtures";
import { adminToken, signIn } from "./helpers";

function liveClient() {
  const dashboardRoot =
    process.env.OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL ??
    "http://127.0.0.1:8787/operator/";
  return createOpenComputeClient({
    apiToken: adminToken,
    baseURL: new URL("/client/v4", dashboardRoot).href,
    maxRetries: 0,
  });
}
test.describe("Cloudflare v4 dashboard consumers", () => {
  test.beforeEach(async ({ page }) => {
    await signIn(page);
  });

  test("catalog navigation uses only the v4 management root", async ({
    page,
  }) => {
    const managementRequests: string[] = [];
    page.on("request", (request) => {
      if (request.url().includes("/client/")) {
        managementRequests.push(new URL(request.url()).pathname);
      }
    });
    for (const path of [
      "/",
      "/workers",
      "/kv",
      "/d1",
      "/r2",
      "/durable-objects",
      "/queues",
      "/workflows",
      "/platform",
    ]) {
      await page.goto(new URL(path.slice(1), page.url()).href);
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
    }
    expect(managementRequests.length).toBeGreaterThan(0);
    expect(
      managementRequests.every((path) => path.startsWith("/client/v4/")),
    ).toBe(true);
  });

  test("capability-scoped SDK shares authentication and transport", async () => {
    const client = liveClient();
    const accounts = await client.accounts.list();
    const instanceID = accounts.result[0]?.id;
    expect(instanceID).toBeTruthy();
    const capabilities = await client.openCompute.capabilities.get();
    expect(capabilities.wrangler_version).toBe("4.138.0");

    const title = `pw-kv-${crypto.randomUUID().replaceAll("-", "")}`;
    const namespace = await client.kv.namespaces.create({
      account_id: instanceID!,
      title,
    });
    try {
      const page = await client.kv.namespaces.list({
        account_id: instanceID!,
      });
      expect(
        page.result.some(
          (item) => item.id === namespace.id && item.title === title,
        ),
      ).toBe(true);
    } finally {
      await client.kv.namespaces.delete(namespace.id, {
        account_id: instanceID!,
      });
    }
  });
});
