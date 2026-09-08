import { defineConfig, devices } from "@playwright/test";

const baseURL =
  process.env.OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL ??
  "http://127.0.0.1:8787/operator/";
const adminToken = process.env.OPEN_COMPUTE_ADMIN_TOKEN ?? "dev-admin-token";
const browserChannel = process.env.OPEN_COMPUTE_DASHBOARD_E2E_BROWSER_CHANNEL;

export default defineConfig({
  testDir: "./tests/e2e",
  fullyParallel: false,
  forbidOnly: Boolean(process.env.CI),
  retries: 0,
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
      use: {
        ...devices["Desktop Chrome"],
        ...(browserChannel === undefined ? {} : { channel: browserChannel }),
      },
    },
  ],
  metadata: {
    adminToken,
  },
});
