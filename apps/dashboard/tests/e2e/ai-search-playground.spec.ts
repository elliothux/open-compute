import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import Cloudflare from "cloudflare";
import { expect, test } from "./fixtures";
import { adminToken, signIn } from "./helpers";

test("AI Search Playground sends CF-shaped search and renders streamed chat", async ({
  page,
}) => {
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
  const suffix = crypto.randomUUID().slice(0, 8);
  const namespace = `pw-playground-${suffix}`;
  const instance = `pw-playground-instance-${suffix}`;
  const chunk = {
    id: "chunk-1",
    type: "text",
    text: "The reference document describes folder uploads.",
    score: 0.535,
    item: { key: "docs/reference.txt", timestamp: 1_700_000_000 },
    scoring_details: { vector_score: 0.535 },
  };
  const capture = process.env.OPEN_COMPUTE_CAPTURE_PLAYGROUND === "1";
  const screenshots = resolve(
    import.meta.dirname,
    "../../../../dashboard-refactor/implementation-screenshots",
  );
  if (capture) await mkdir(screenshots, { recursive: true });
  if (capture) await page.setViewportSize({ width: 1808, height: 1113 });
  await api.aiSearch.namespaces.create({
    account_id: accountId,
    name: namespace,
  });
  try {
    await api.aiSearch.namespaces.instances.create(namespace, {
      account_id: accountId,
      id: instance,
      custom_metadata: [{ field_name: "folder", data_type: "text" }],
    });
    const requests: Array<Record<string, unknown>> = [];
    await page.route(
      `**/ai-search/namespaces/${namespace}/instances/${instance}/search`,
      async (route) => {
        requests.push(
          route.request().postDataJSON() as Record<string, unknown>,
        );
        await route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            success: true,
            errors: [],
            messages: [],
            result: {
              query_kind: "text",
              search_query: "reference",
              chunks: [chunk],
            },
          }),
        });
      },
    );
    await page.route(
      `**/ai-search/namespaces/${namespace}/instances/${instance}/chat/completions`,
      async (route) => {
        requests.push(
          route.request().postDataJSON() as Record<string, unknown>,
        );
        await route.fulfill({
          status: 200,
          contentType: "text/event-stream",
          body: [
            `event: chunks\ndata: ${JSON.stringify([chunk])}\n\n`,
            'data: {"choices":[{"delta":{"content":"Folder "}}]}\n\n',
            'data: {"choices":[{"delta":{"content":"uploads."}}]}\n\n',
            "data: [DONE]\n\n",
          ].join(""),
        });
      },
    );
    await signIn(page);
    await page.goto(`./ai-search/${namespace}/${instance}?tab=playground`);
    await expect(page.getByRole("tab", { name: "Search" })).toBeVisible();
    await page.waitForLoadState("networkidle");
    if (capture)
      await page.screenshot({
        path: resolve(
          screenshots,
          "ai-search-instance-playground-search-empty-desktop.png",
        ),
        fullPage: true,
      });
    await expect(
      page.getByRole("button", { name: "Search", exact: true }),
    ).toBeDisabled();
    await page
      .getByRole("textbox", { name: "Search your documents" })
      .fill("reference");
    await page.getByRole("button", { name: "Search", exact: true }).click();
    await expect(page.getByText(chunk.text)).toBeVisible();
    if (capture)
      await page.screenshot({
        path: resolve(
          screenshots,
          "ai-search-instance-playground-search-result-desktop.png",
        ),
        fullPage: true,
      });
    await page.getByRole("button", { name: /Score 0\.535/ }).click();
    await expect(page.getByText("Vector score")).toBeVisible();
    await expect(requests[0]).toMatchObject({
      messages: [{ role: "user", content: "reference" }],
      ai_search_options: {
        retrieval: { match_threshold: 0.4, max_num_results: 10 },
      },
    });
    await page.getByRole("tab", { name: "Chat" }).click();
    await page
      .getByRole("textbox", { name: "Enter your message" })
      .fill("What is in the reference?");
    await page.getByRole("button", { name: "Send message" }).click();
    await expect(page.getByText("Folder uploads.")).toBeVisible();
    await page.getByRole("button", { name: "Sources (1)" }).click();
    await expect(page.getByText(chunk.text)).toBeVisible();
    if (capture)
      await page.screenshot({
        path: resolve(
          screenshots,
          "ai-search-instance-playground-chat-result-desktop.png",
        ),
        fullPage: true,
      });
    await expect(requests[1]).toMatchObject({
      stream: true,
      messages: [{ role: "user", content: "What is in the reference?" }],
    });
    await page.getByRole("button", { name: "Clear conversation" }).click();
    await expect(page.getByText("Ask your documents")).toBeVisible();
    if (capture) {
      await page.getByRole("button", { name: "Edit" }).click();
      await expect(
        page
          .getByRole("dialog")
          .getByRole("heading", { name: "Metadata filters" }),
      ).toBeVisible();
      await page.waitForFunction(() => {
        const dialog = document.querySelector('[role="dialog"]');
        return (
          dialog !== null && Number(getComputedStyle(dialog).opacity) >= 0.99
        );
      });
      await page.screenshot({
        path: resolve(
          screenshots,
          "ai-search-instance-playground-filters-desktop.png",
        ),
        fullPage: false,
      });
      await page
        .getByRole("dialog")
        .getByRole("button", { name: "Cancel" })
        .click();
      await page.setViewportSize({ width: 390, height: 844 });
      await page.reload();
      await expect(page.getByRole("tab", { name: "Search" })).toBeVisible();
      await page.waitForLoadState("networkidle");
      await page.screenshot({
        path: resolve(
          screenshots,
          "ai-search-instance-playground-search-empty-mobile.png",
        ),
        fullPage: true,
      });
    }
  } finally {
    await api.aiSearch.namespaces.instances
      .delete(instance, { account_id: accountId, name: namespace })
      .catch(() => {});
    await api.aiSearch.namespaces
      .delete(namespace, { account_id: accountId })
      .catch(() => {});
  }
});
