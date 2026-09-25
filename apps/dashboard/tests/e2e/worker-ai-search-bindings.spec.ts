import { resolve } from "node:path";
import Cloudflare from "cloudflare";
import { expect, test } from "./fixtures";
import { adminToken, signIn } from "./helpers";

test("AI Search instance and namespace bindings preserve each other across Version deployments", async ({
  page,
}) => {
  test.setTimeout(180_000);
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
  const worker = `pw-search-binding-${suffix}`;
  const namespaces = [`pw-search-a-${suffix}`, `pw-search-b-${suffix}`];
  const instances = [`pw-instance-a-${suffix}`, `pw-instance-b-${suffix}`];
  let workerCreated = false;
  let namespaceCount = 0;
  let instanceCount = 0;
  const capture = process.env.OPEN_COMPUTE_CAPTURE_AI_SEARCH_BINDINGS === "1";
  const screenshot = (name: string) =>
    resolve(
      import.meta.dirname,
      "../../../../dashboard-refactor/implementation-screenshots",
      name,
    );
  const active = async () => {
    const deployment = await api.workers.scripts.deployments.list(worker, {
      account_id: accountId,
    });
    const versionId = deployment.deployments[0]?.versions[0]?.version_id;
    if (!versionId) throw new Error("Active Worker Version is missing.");
    const version = await api.workers.scripts.versions.get(versionId, {
      account_id: accountId,
      script_name: worker,
    });
    return { versionId, bindings: version.resources.bindings ?? [] };
  };
  const assertBinding = (
    bindings: Awaited<ReturnType<typeof active>>["bindings"],
    type: "ai_search" | "ai_search_namespace",
    name: string,
    resource: string,
  ) => {
    const binding = bindings.find(
      (item) => item.type === type && item.name === name,
    );
    expect(binding?.type).toBe(type);
    if (binding?.type === "ai_search")
      expect(binding.instance_name).toBe(resource);
    else if (binding?.type === "ai_search_namespace")
      expect(binding.namespace).toBe(resource);
  };
  const open = async (label: string) => {
    await page
      .getByRole("button", { name: "Add binding", exact: true })
      .click();
    const gallery = page.getByRole("dialog", { name: "Add binding" });
    await gallery.getByRole("option", { name: label, exact: true }).click();
    await gallery.getByRole("button", { name: "Add binding" }).click();
    await expect(gallery).toBeHidden();
    return page.getByRole("dialog", { name: `Add ${label} binding` });
  };
  try {
    for (let index = 0; index < 2; index++) {
      await api.aiSearch.namespaces.create({
        account_id: accountId,
        name: namespaces[index]!,
      });
      namespaceCount++;
      await api.aiSearch.namespaces.instances.create(namespaces[index]!, {
        account_id: accountId,
        id: instances[index]!,
        embedding_model: "@cf/qwen/qwen3-embedding-0.6b",
        index_method: { vector: false, keyword: true },
      });
      instanceCount++;
    }
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
    workerCreated = true;
    await page.getByRole("tab", { name: "Settings" }).click();
    if (capture) await page.setViewportSize({ width: 1808, height: 1169 });
    const initial = await active();

    const namespaceForm = await open("AI Search namespace");
    await namespaceForm
      .getByRole("textbox", { name: "Binding name" })
      .fill("SEARCH_NS");
    await namespaceForm
      .getByRole("combobox", { name: "AI Search namespace", exact: true })
      .selectOption(namespaces[0]!);
    if (capture) {
      await namespaceForm.screenshot({
        path: screenshot("worker-ai-search-namespace-binding-add-desktop.png"),
      });
      await page.setViewportSize({ width: 390, height: 844 });
      await namespaceForm.screenshot({
        path: screenshot("worker-ai-search-namespace-binding-add-mobile.png"),
      });
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
        .toBe(390);
      await page.setViewportSize({ width: 1808, height: 1169 });
    }
    await namespaceForm.getByRole("button", { name: "Add binding" }).click();
    await expect(namespaceForm).toBeHidden();
    const withNamespace = await active();
    expect(withNamespace.versionId).not.toBe(initial.versionId);
    assertBinding(
      withNamespace.bindings,
      "ai_search_namespace",
      "SEARCH_NS",
      namespaces[0]!,
    );

    const instanceForm = await open("AI Search");
    await instanceForm
      .getByRole("textbox", { name: "Binding name" })
      .fill("SEARCH");
    await instanceForm
      .getByRole("combobox", { name: "AI Search", exact: true })
      .selectOption(instances[0]!);
    if (capture) {
      await instanceForm.screenshot({
        path: screenshot("worker-ai-search-binding-add-desktop.png"),
      });
      await page.setViewportSize({ width: 390, height: 844 });
      await instanceForm.screenshot({
        path: screenshot("worker-ai-search-binding-add-mobile.png"),
      });
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth))
        .toBe(390);
      await page.setViewportSize({ width: 1808, height: 1169 });
    }
    await instanceForm.getByRole("button", { name: "Add binding" }).click();
    await expect(instanceForm).toBeHidden();
    const withBoth = await active();
    expect(withBoth.versionId).not.toBe(withNamespace.versionId);
    assertBinding(withBoth.bindings, "ai_search", "SEARCH", instances[0]!);
    assertBinding(
      withBoth.bindings,
      "ai_search_namespace",
      "SEARCH_NS",
      namespaces[0]!,
    );

    await page.getByRole("button", { name: "Edit SEARCH_NS" }).click();
    const editNamespace = page.getByRole("dialog", {
      name: "Edit AI Search namespace binding",
    });
    await editNamespace
      .getByRole("combobox", { name: "AI Search namespace", exact: true })
      .selectOption(namespaces[1]!);
    await editNamespace.getByRole("button", { name: "Deploy" }).click();
    await expect(editNamespace).toBeHidden();
    const editedNamespace = await active();
    assertBinding(
      editedNamespace.bindings,
      "ai_search_namespace",
      "SEARCH_NS",
      namespaces[1]!,
    );
    assertBinding(
      editedNamespace.bindings,
      "ai_search",
      "SEARCH",
      instances[0]!,
    );

    await page
      .getByRole("button", { name: "Edit SEARCH", exact: true })
      .click();
    const editInstance = page.getByRole("dialog", {
      name: "Edit AI Search binding",
    });
    await editInstance
      .getByRole("combobox", { name: "AI Search", exact: true })
      .selectOption(instances[1]!);
    await editInstance.getByRole("button", { name: "Deploy" }).click();
    await expect(editInstance).toBeHidden();
    const editedBoth = await active();
    assertBinding(
      editedBoth.bindings,
      "ai_search_namespace",
      "SEARCH_NS",
      namespaces[1]!,
    );
    assertBinding(editedBoth.bindings, "ai_search", "SEARCH", instances[1]!);

    for (const [name, type] of [
      ["SEARCH", "ai_search"],
      ["SEARCH_NS", "ai_search_namespace"],
    ] as const) {
      await page
        .getByRole("button", { name: `Delete ${name}`, exact: true })
        .click();
      const confirmation = page.getByRole("alertdialog", {
        name: `Delete ${type === "ai_search" ? "AI Search" : "AI Search namespace"} binding?`,
      });
      await confirmation
        .getByRole("button", { name: "Delete and deploy" })
        .click();
      await expect(confirmation).toBeHidden();
      expect(
        (await active()).bindings.some((binding) => binding.name === name),
      ).toBe(false);
    }
  } finally {
    if (workerCreated)
      await api.workers.scripts.delete(worker, { account_id: accountId });
    for (let index = 0; index < instanceCount; index++)
      await api.aiSearch.namespaces.instances.delete(instances[index]!, {
        account_id: accountId,
        name: namespaces[index]!,
      });
    for (let index = 0; index < namespaceCount; index++)
      await api.aiSearch.namespaces.delete(namespaces[index]!, {
        account_id: accountId,
      });
  }
});
