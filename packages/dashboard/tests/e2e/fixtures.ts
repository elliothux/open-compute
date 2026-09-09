import { writeFile } from "node:fs/promises";
import { test as base, expect } from "@playwright/test";

/** Browser fixture that retains client/server contract diagnostics for every E2E case. */
const test = base.extend({
  page: async ({ page }, provide, testInfo) => {
    const consoleErrors: string[] = [];
    const failedRequests: {
      method: string;
      url: string;
      error: string | null;
    }[] = [];
    const errorResponses: { method: string; url: string; status: number }[] =
      [];
    page.on("console", (message) => {
      if (message.type() === "error") consoleErrors.push(message.text());
    });
    page.on("pageerror", (error) => consoleErrors.push(error.message));
    page.on("requestfailed", (request) => {
      failedRequests.push({
        method: request.method(),
        url: request.url(),
        error: request.failure()?.errorText ?? null,
      });
    });
    page.on("response", (response) => {
      if (response.status() >= 400) {
        errorResponses.push({
          method: response.request().method(),
          url: response.url(),
          status: response.status(),
        });
      }
    });
    await provide(page);
    const path = testInfo.outputPath("browser-contract-diagnostics.json");
    await writeFile(
      path,
      JSON.stringify(
        { consoleErrors, failedRequests, errorResponses },
        null,
        2,
      ),
    );
    await testInfo.attach("browser-contract-diagnostics", {
      path,
      contentType: "application/json",
    });
  },
});

export { expect, test };
