import { execFileSync } from "node:child_process";
import { resolve } from "node:path";
import { expect, test } from "@playwright/test";
import {
  createOperatorClient,
  parseAccountId,
  parseResourceId,
  parseWorkerId,
  parseWorkflowId,
} from "@open-compute/operator-sdk";
import { adminToken, signIn } from "./helpers";

const workerSource = `
import { DurableObject, WorkflowEntrypoint } from "cloudflare:workers";

export class Counter extends DurableObject {
  async fetch() {
    const value = Number((await this.ctx.storage.get("value")) ?? 0) + 1;
    await this.ctx.storage.put("value", value);
    return new Response(String(value));
  }
}

export class Scratch extends DurableObject {
  async fetch() {
    return new Response("scratch");
  }
}

export class Flow extends WorkflowEntrypoint {
  async run(event, step) {
    await step.sleep("browser hold", "30 seconds");
    return { instanceId: event.instanceId };
  }
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname.endsWith("/do")) {
      return env.OBJECTS.getByName("browser-object").fetch(request);
    }
    if (url.pathname.endsWith("/create")) {
      const instance = await env.FLOW.create({ id: url.searchParams.get("id"), params: {} });
      return Response.json({ id: instance.id });
    }
    return new Response("open-compute browser lifecycle");
  },
};
`;

function canonicalBundle(): Buffer {
  const input = JSON.stringify({
    schemaVersion: 1,
    mainModule: "index.js",
    modules: [{
      name: "index.js",
      type: "esModule",
      bytesBase64: Buffer.from(workerSource).toString("base64"),
    }],
  });
  return execFileSync(resolve(process.cwd(), "../../target/debug/ocd"), ["worker", "bundle"], {
    input,
    maxBuffer: 4 * 1024 * 1024,
  });
}

function liveClient() {
  const root = process.env.OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL ?? "http://127.0.0.1:8787/operator/";
  return createOperatorClient({
    baseUrl: new URL("api/v1/", root),
    getAccessToken: () => adminToken,
  });
}

async function createWorkerFromCatalog(page: import("@playwright/test").Page, name: string) {
  await page.locator('a[href="/workers"]').first().click();
  await page.getByRole("button", { name: "Create Worker" }).first().click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Worker name").fill(name);
  await dialog.getByRole("button", { name: "Create Worker" }).click();
  await expect(page.getByText(name, { exact: true })).toBeVisible({ timeout: 15_000 });
  await page.getByRole("button", { name: `Actions for ${name}` }).click();
  await page.getByRole("menuitem", { name: "Open" }).click();
  const workerId = page.url().split("/workers/")[1]?.split(/[?#]/)[0];
  if (!workerId) throw new Error("Worker detail URL did not include an ID");
  return parseWorkerId(workerId);
}

async function uploadBundle(
  page: import("@playwright/test").Page,
  bundle: Buffer,
  promote: boolean,
) {
  await page.getByRole("tab", { name: "Upload" }).click();
  await page.locator("#worker-bundle-file").setInputFiles({
    name: "browser-worker.bundle",
    mimeType: "application/octet-stream",
    buffer: bundle,
  });
  const checkbox = page.getByRole("checkbox", { name: "Promote immediately after upload" });
  if (promote) await checkbox.check();
  else await checkbox.uncheck();
  await page.getByRole("button", { name: "Upload deployment" }).click();
  await expect(page.getByRole("tab", { name: "Deployments", selected: true })).toBeVisible({ timeout: 20_000 });
  await expect(page.getByRole("cell", { name: "ready", exact: true }).first()).toBeVisible({ timeout: 20_000 });
}

test.describe("operator dashboard product lifecycle", () => {
  test.beforeEach(async ({ page }) => {
    await signIn(page);
  });

  test("Worker upload, traffic, promotion, rollback, route, cache, and deletion guards are live", async ({ page }) => {
    test.setTimeout(90_000);
    const client = liveClient();
    const accountId = parseAccountId((await client.system.account()).accountId);
    const name = `pw-runtime-${Date.now()}`;
    const workerId = await createWorkerFromCatalog(page, name);
    const bundle = canonicalBundle();

    await uploadBundle(page, bundle, true);
    const worker = await client.workers.get({ accountId, workerId });
    const firstDeploymentId = worker.worker.activeDeploymentId;
    expect(firstDeploymentId).toBeTruthy();

    const hostname = `browser-${Date.now()}.example.test`;
    await page.getByRole("tab", { name: "Routes" }).click();
    await page.getByLabel("Hostname").fill(hostname);
    await page.getByLabel("Path prefix").fill("/");
    await page.getByRole("button", { name: "Create route" }).click();
    await expect(page.getByRole("cell", { name: `${hostname}/`, exact: true })).toBeVisible({ timeout: 15_000 });

    const routes = await client.workers.listRoutes({ accountId, workerId });
    const defaultRoute = routes.routes.find(route => route.kind === "platform_path");
    const customRoute = routes.routes.find(route => route.hostnameAscii === hostname);
    expect(defaultRoute).toBeTruthy();
    expect(customRoute).toBeTruthy();
    const response = await page.request.get(new URL(defaultRoute!.pathPrefix, page.url()).href);
    expect(response.status()).toBe(200);
    expect(await response.text()).toBe("open-compute browser lifecycle");
    await response.dispose();

    await page.getByRole("link", { name: "Back to Workers" }).click();
    const catalogRow = page.getByRole("row").filter({ hasText: name });
    await expect(catalogRow.getByRole("cell").nth(3)).toHaveText("1", { timeout: 15_000 });
    await catalogRow.getByRole("button", { name: `Actions for ${name}` }).click();
    await page.getByRole("menuitem", { name: "Open" }).click();

    await uploadBundle(page, bundle, false);
    const deployments = await client.workers.listDeployments({ accountId, workerId });
    const secondDeployment = deployments.deployments.find(deployment => deployment.id !== firstDeploymentId);
    expect(secondDeployment).toBeTruthy();
    const secondRow = page.getByRole("row").filter({ hasText: secondDeployment!.id });
    await secondRow.getByRole("button", { name: "Promote" }).click();
    let dialog = page.getByRole("alertdialog");
    await dialog.getByRole("textbox").fill(secondDeployment!.id);
    await dialog.getByRole("button", { name: "Promote" }).click();
    await expect(secondRow.getByText("active", { exact: true })).toBeVisible({ timeout: 20_000 });

    const firstRow = page.getByRole("row").filter({ hasText: firstDeploymentId! });
    await firstRow.getByRole("button", { name: "Rollback" }).click();
    dialog = page.getByRole("alertdialog");
    await dialog.getByRole("textbox").fill(firstDeploymentId!);
    await dialog.getByRole("button", { name: "Rollback" }).click();
    await expect(firstRow.getByText("active", { exact: true })).toBeVisible({ timeout: 20_000 });

    await secondRow.getByRole("button", { name: "Promote" }).click();
    dialog = page.getByRole("alertdialog");
    await dialog.getByRole("textbox").fill(secondDeployment!.id);
    await dialog.getByRole("button", { name: "Promote" }).click();
    await expect(secondRow.getByText("active", { exact: true })).toBeVisible({ timeout: 20_000 });

    await firstRow.getByRole("button", { name: "Delete" }).click();
    dialog = page.getByRole("alertdialog");
    await dialog.getByRole("textbox").fill(firstDeploymentId!);
    await dialog.getByRole("button", { name: "Delete deployment" }).click();
    await expect(dialog.getByRole("alert")).toContainText("still referenced", { timeout: 15_000 });
    await dialog.getByRole("button", { name: "Cancel" }).click();

    await page.getByRole("tab", { name: "Routes" }).click();
    const routeRow = page.getByRole("row").filter({ hasText: hostname });
    await routeRow.getByRole("button", { name: "Delete" }).click();
    dialog = page.getByRole("alertdialog");
    await dialog.getByRole("textbox").fill(customRoute!.id);
    await dialog.getByRole("button", { name: "Delete route" }).click();
    await expect(routeRow).toHaveCount(0);

    await page.getByRole("tab", { name: "Cache" }).click();
    await page.getByRole("button", { name: "Purge cache" }).click();
    dialog = page.getByRole("alertdialog");
    await dialog.getByRole("textbox").fill(name);
    await dialog.getByRole("button", { name: "Purge cache" }).click();
    await expect(page.getByText(/Purged \d+ cache entr/)).toBeVisible({ timeout: 15_000 });

    await page.getByRole("tab", { name: "Overview" }).click();
    await page.getByRole("button", { name: "Delete Worker" }).click();
    dialog = page.getByRole("alertdialog");
    await dialog.getByRole("textbox").fill(name);
    await dialog.getByRole("button", { name: "Delete" }).click();
    await expect(dialog.getByRole("alert")).toContainText("in-flight requests", { timeout: 15_000 });
    await dialog.getByRole("button", { name: "Cancel" }).click();

    const disposableName = `pw-delete-${Date.now()}`;
    const disposableWorkerId = await createWorkerFromCatalog(page, disposableName);
    await page.getByRole("button", { name: "Delete Worker" }).click();
    dialog = page.getByRole("alertdialog");
    await dialog.getByRole("textbox").fill(disposableName);
    await dialog.getByRole("button", { name: "Delete" }).click();
    await expect(page.getByRole("heading", { name: "Workers", level: 1 })).toBeVisible({ timeout: 20_000 });
    await expect.poll(
      () => client.workers.get({ accountId, workerId: disposableWorkerId })
        .then(result => result.worker.deletedAtMs === null ? "present" : "deleted")
        .catch(() => "deleted"),
    ).toBe("deleted");
  });

  test("Durable Object inventory deletion and Workflow instance controls use real runtime state", async ({ page }) => {
    test.setTimeout(120_000);
    const client = liveClient();
    const accountId = parseAccountId((await client.system.account()).accountId);
    const suffix = Date.now();
    const workerName = `pw-products-${suffix}`;
    const namespaceName = `pw-do-${suffix}`;
    const workflowName = `pw-flow-${suffix}`;
    const workerId = await createWorkerFromCatalog(page, workerName);
    const bundle = canonicalBundle();
    await uploadBundle(page, bundle, true);
    const initialDeploymentId = (await client.workers.get({ accountId, workerId })).worker.activeDeploymentId!;

    await page.locator('a[href="/durable-objects"]').first().click();
    await page.getByRole("button", { name: "Create namespace" }).first().click();
    const namespaceDialog = page.getByRole("dialog");
    await namespaceDialog.getByLabel("Namespace name").fill(namespaceName);
    await namespaceDialog.getByRole("combobox", { name: "Owner Worker" }).click();
    await page.getByRole("option", { name: new RegExp(workerName) }).click();
    await namespaceDialog.getByLabel("Class name").fill("Counter");
    await namespaceDialog.getByRole("button", { name: "Create namespace" }).click();
    const namespaceRow = page.getByRole("row").filter({ hasText: namespaceName });
    await expect(namespaceRow).toBeVisible({ timeout: 20_000 });
    const namespaceId = parseResourceId((await namespaceRow.locator("code").first().textContent())!.trim());

    await page.locator('a[href="/workflows"]').first().click();
    await page.getByRole("button", { name: "Create Workflow" }).first().click();
    const workflowDialog = page.getByRole("dialog");
    await workflowDialog.getByLabel("Workflow name").fill(workflowName);
    await workflowDialog.getByRole("button", { name: "Create Workflow" }).click();
    const workflowRow = page.getByRole("row").filter({ hasText: workflowName });
    await expect(workflowRow).toBeVisible({ timeout: 20_000 });
    const workflowId = parseWorkflowId((await workflowRow.locator("code").first().textContent())!.trim());
    await workflowRow.getByRole("button", { name: `Actions for ${workflowName}` }).click();
    await page.getByRole("menuitem", { name: "Open" }).click();
    await page.getByRole("tab", { name: "Versions" }).click();
    await page.getByRole("button", { name: "Create version" }).click();
    await page.getByRole("dialog").getByLabel("Deployment ID").fill(initialDeploymentId);
    await page.getByRole("dialog").getByLabel("Exported class name").fill("Flow");
    await page.getByRole("dialog").getByRole("button", { name: "Create version" }).click();
    await expect(page.getByRole("cell", { name: "Flow", exact: true })).toBeVisible({ timeout: 20_000 });

    const boundDeployment = await client.workers.createDeployment({
      accountId,
      workerId,
      bundle,
      metadata: JSON.stringify({
        mainModule: "index.js",
        vars: {},
        secrets: {},
        bindings: {
          OBJECTS: { type: "do_namespace", id: namespaceId },
          FLOW: { type: "workflow", id: workflowId },
        },
        services: {},
        promote: true,
      }),
      idempotencyKey: crypto.randomUUID(),
    });
    expect(boundDeployment.promoted).toBe(true);
    const defaultRoute = (await client.workers.listRoutes({ accountId, workerId })).routes.find(route => route.kind === "platform_path")!;
    const routeUrl = new URL(defaultRoute.pathPrefix, page.url());
    const objectResponse = await page.request.get(new URL("do", routeUrl).href);
    expect(objectResponse.status()).toBe(200);
    await objectResponse.dispose();
    const externalInstanceId = `browser-${suffix}`;
    const createUrl = new URL("create", routeUrl);
    createUrl.searchParams.set("id", externalInstanceId);
    const createResponse = await page.request.get(createUrl.href);
    expect(createResponse.status()).toBe(200);
    await createResponse.dispose();

    await page.locator('a[href="/durable-objects"]').first().click();
    const refreshedNamespaceRow = page.getByRole("row").filter({ hasText: namespaceName });
    await refreshedNamespaceRow.getByRole("button", { name: `Actions for ${namespaceName}` }).click();
    await page.getByRole("menuitem", { name: "View objects" }).click();
    const objectRow = page.getByRole("table").getByRole("row").filter({ hasText: "ready" }).first();
    await expect(objectRow).toBeVisible({ timeout: 20_000 });
    const objectId = (await objectRow.getByRole("cell").first().locator("code").textContent())!.trim();
    await objectRow.getByRole("button", { name: "Delete" }).click();
    let dialog = page.getByRole("alertdialog");
    await dialog.getByRole("textbox").fill(objectId);
    await dialog.getByRole("button", { name: "Delete object" }).click();
    await expect(objectRow).toHaveCount(0);

    await page.getByRole("link", { name: "Back to namespaces" }).click();
    const disposableNamespaceName = `pw-do-empty-${suffix}`;
    const renamedNamespaceName = `${disposableNamespaceName}-renamed`;
    await page.getByRole("button", { name: "Create namespace" }).first().click();
    const disposableDialog = page.getByRole("dialog");
    await disposableDialog.getByLabel("Namespace name").fill(disposableNamespaceName);
    await disposableDialog.getByRole("combobox", { name: "Owner Worker" }).click();
    await page.getByRole("option", { name: new RegExp(workerName) }).click();
    await disposableDialog.getByLabel("Class name").fill("Scratch");
    await disposableDialog.getByRole("button", { name: "Create namespace" }).click();
    await expect(page.getByRole("cell", { name: disposableNamespaceName, exact: true })).toBeVisible({ timeout: 20_000 });
    await page.getByRole("button", { name: `Actions for ${disposableNamespaceName}` }).click();
    await page.getByRole("menuitem", { name: "Rename" }).click();
    await page.getByRole("dialog").getByRole("textbox").fill(renamedNamespaceName);
    await page.getByRole("dialog").getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("cell", { name: renamedNamespaceName, exact: true })).toBeVisible({ timeout: 20_000 });
    await page.getByRole("button", { name: `Actions for ${renamedNamespaceName}` }).click();
    await page.getByRole("menuitem", { name: "Delete" }).click();
    dialog = page.getByRole("alertdialog");
    await dialog.getByRole("textbox").fill(renamedNamespaceName);
    await dialog.getByRole("button", { name: "Delete" }).click();
    await expect(page.getByRole("cell", { name: renamedNamespaceName, exact: true })).toHaveCount(0);

    await page.locator('a[href="/workflows"]').first().click();
    const refreshedWorkflowRow = page.getByRole("row").filter({ hasText: workflowName });
    await refreshedWorkflowRow.getByRole("button", { name: `Actions for ${workflowName}` }).click();
    await page.getByRole("menuitem", { name: "Open" }).click();
    await page.getByRole("tab", { name: "Instances" }).click();
    const instanceRow = page.getByRole("row").filter({ hasText: externalInstanceId });
    await expect(instanceRow).toBeVisible({ timeout: 20_000 });
    await instanceRow.getByRole("button", { name: "Inspect" }).click();
    await page.getByRole("button", { name: "Pause" }).click();
    await expect(page.getByText("paused", { exact: true })).toBeVisible({ timeout: 20_000 });
    await page.getByRole("button", { name: "Resume" }).click();
    await page.getByRole("button", { name: "Terminate" }).click();
    dialog = page.getByRole("alertdialog");
    const instanceId = page.url().match(/[?&]instance=([^&]+)/)?.[1];
    if (!instanceId) throw new Error("Workflow instance URL did not include an ID");
    await dialog.getByRole("textbox").fill(decodeURIComponent(instanceId));
    await dialog.getByRole("button", { name: "Terminate instance" }).click();
    await expect(page.getByText("terminated", { exact: true })).toBeVisible({ timeout: 20_000 });
    await page.getByRole("button", { name: "Restart", exact: true }).click();
    dialog = page.getByRole("alertdialog");
    await dialog.getByRole("textbox").fill(decodeURIComponent(instanceId));
    await dialog.getByRole("button", { name: "Restart instance" }).click();
    await expect(page.getByText(/generation 2/)).toBeVisible({ timeout: 20_000 });
  });
});
