import { defineConfig, devices } from "@playwright/test";

const baseURL = process.env.OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL ?? "http://127.0.0.1:8787/operator/";
const adminToken = process.env.OPEN_COMPUTE_ADMIN_TOKEN ?? "dev-admin-token";

export default defineConfig({
  testDir: "./tests/e2e",
  fullyParallel: false,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  workers: 1,
  reporter: [["list"]],
  use: {
    baseURL,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    extraHTTPHeaders: {},
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
  metadata: {
    adminToken,
  },
});
