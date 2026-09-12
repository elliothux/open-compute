import { cp, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { command } from "../adapters/command.ts";
import { observe } from "../adapters/observations.ts";
import {
  cloudflareBaseProject,
  cloudflareProject,
  openComputeBaseProject,
  openComputeProject,
} from "../adapters/projects.ts";
import { cloudflareDeploymentUrl } from "../adapters/transport.ts";
import type { JsonRecord, PortableFixture } from "../adapters/types.ts";
import { ensureCloudflareAbsent } from "./cloudflare-resources.ts";
import { recordOwnership } from "./evidence.ts";
import {
  bindingIds,
  bindingNames,
  ownedResources,
  type OwnedResources,
} from "./owned-resources.ts";
import { verifyWorkflowCreated } from "./product-resources.ts";
import {
  cleanupFixtureResources,
  type WorkerOwnership,
} from "./resource-cleanup.ts";
import {
  provisionResources,
  type DifferentialConfigs,
} from "./resource-provisioning.ts";
import { sanitizedError, type DifferentialContext } from "./run-context.ts";

export interface FixtureRunResult {
  readonly result: JsonRecord;
  readonly failure?: string;
  readonly cleanup: () => Promise<{
    readonly record: JsonRecord;
    readonly deleted: boolean;
  }>;
}

async function prepareFixtureFiles(
  context: DifferentialContext,
  fixture: PortableFixture,
  index: number,
  name: string,
): Promise<{
  readonly projectRoot: string;
  readonly configs: DifferentialConfigs;
}> {
  const projectRoot = join(context.directory, String(index));
  await cp(fixture.root, projectRoot, { recursive: true });
  const configs: DifferentialConfigs = {
    cloudflarePreflight: join(
      projectRoot,
      "wrangler-cloudflare-preflight.jsonc",
    ),
    cloudflare: join(projectRoot, "wrangler-cloudflare.jsonc"),
    openComputePreflight: join(
      projectRoot,
      "wrangler-open-compute-preflight.jsonc",
    ),
    openCompute: join(projectRoot, "wrangler-open-compute.jsonc"),
  };
  await writeFile(
    join(projectRoot, "tsconfig.json"),
    `${JSON.stringify(
      {
        extends: join(context.root, "tsconfig.json"),
        compilerOptions: { types: ["@open-compute/workers-types"] },
        include: ["src/**/*.ts"],
      },
      null,
      2,
    )}\n`,
    { mode: 0o600 },
  );
  await writeFile(
    configs.cloudflarePreflight,
    `${JSON.stringify(
      cloudflareBaseProject(fixture, name, context.accountId),
      null,
      2,
    )}\n`,
    { mode: 0o600 },
  );
  await writeFile(
    configs.openComputePreflight,
    `${JSON.stringify(
      openComputeBaseProject(fixture, name, context.openComputeAccount),
      null,
      2,
    )}\n`,
    { mode: 0o600 },
  );
  return { projectRoot, configs };
}

async function writeDeploymentConfigs(
  context: DifferentialContext,
  fixture: PortableFixture,
  name: string,
  resources: OwnedResources,
  configs: DifferentialConfigs,
): Promise<void> {
  const names = bindingNames(resources);
  await writeFile(
    configs.cloudflare,
    `${JSON.stringify(
      cloudflareProject(
        fixture,
        name,
        context.accountId,
        bindingIds(resources, "cloudflare"),
        names,
      ),
      null,
      2,
    )}\n`,
    { mode: 0o600 },
  );
  await writeFile(
    configs.openCompute,
    `${JSON.stringify(
      openComputeProject(
        fixture,
        name,
        context.openComputeAccount,
        bindingIds(resources, "open-compute"),
        names,
      ),
      null,
      2,
    )}\n`,
    { mode: 0o600 },
  );
}

async function deployCloudflare(
  context: DifferentialContext,
  projectRoot: string,
  configs: DifferentialConfigs,
  resources: OwnedResources,
  ownership: WorkerOwnership,
  name: string,
): Promise<string> {
  ownership.cloudflareOwned = true;
  for (const workflow of resources.workflows) workflow.cloudflareOwned = true;
  const deployed = await command(
    context.wrangler,
    [
      "deploy",
      "--name",
      name,
      "--config",
      configs.cloudflare,
      "--latest=false",
      "--strict",
    ],
    {
      cwd: projectRoot,
      env: context.cloudflareEnv,
      timeout: 300_000,
    },
  );
  await recordOwnership(context.journalPath, {
    target: "cloudflare",
    kind: "worker",
    name,
  });
  for (const workflow of resources.workflows) {
    await verifyWorkflowCreated(
      workflow.name,
      configs.cloudflarePreflight,
      context.wrangler,
      context.cloudflareEnv,
    );
    await recordOwnership(context.journalPath, {
      target: "cloudflare",
      kind: "workflow",
      name: workflow.name,
      binding: workflow.binding,
    });
  }
  return cloudflareDeploymentUrl(
    `${deployed.stdout}\n${deployed.stderr}`,
    name,
  );
}

async function deployOpenCompute(
  context: DifferentialContext,
  projectRoot: string,
  configs: DifferentialConfigs,
  resources: OwnedResources,
  ownership: WorkerOwnership,
  name: string,
): Promise<void> {
  ownership.openComputeOwned = true;
  for (const namespace of resources.durableObjectNamespaces)
    namespace.openComputeOwned = true;
  for (const workflow of resources.workflows) workflow.openComputeOwned = true;
  await command(
    context.wrangler,
    [
      "deploy",
      "--name",
      name,
      "--config",
      configs.openCompute,
      "--latest=false",
      "--strict",
    ],
    {
      cwd: projectRoot,
      env: context.openComputeEnv,
      timeout: 300_000,
    },
  );
  await recordOwnership(context.journalPath, {
    target: "open-compute",
    kind: "worker",
    name,
  });
  for (const namespace of resources.durableObjectNamespaces) {
    await recordOwnership(context.journalPath, {
      target: "open-compute",
      kind: "durable_object_namespace",
      name: namespace.binding,
      binding: namespace.binding,
      className: namespace.className,
      parent: name,
    });
  }
  for (const workflow of resources.workflows) {
    await verifyWorkflowCreated(
      workflow.name,
      configs.openComputePreflight,
      context.wrangler,
      context.openComputeEnv,
    );
    await recordOwnership(context.journalPath, {
      target: "open-compute",
      kind: "workflow",
      name: workflow.name,
      binding: workflow.binding,
      parent: name,
    });
  }
}

export async function runFixture(
  context: DifferentialContext,
  fixture: PortableFixture,
  index: number,
): Promise<FixtureRunResult> {
  const name = `${context.prefix}-${index}`;
  const { projectRoot, configs } = await prepareFixtureFiles(
    context,
    fixture,
    index,
    name,
  );
  const resources = ownedResources(fixture, name);
  const ownership: WorkerOwnership = {
    cloudflareAbsent: false,
    cloudflareOwned: false,
    openComputeAbsent: false,
    openComputeOwned: false,
    cloudflareUrl: undefined,
    openComputeUrl: new URL(
      `/__workers/${context.openComputeInternalAccount}/${name}/`,
      context.endpoint,
    ).href,
  };
  let result: JsonRecord;
  let failure: string | undefined;
  try {
    await ensureCloudflareAbsent(
      name,
      configs.cloudflarePreflight,
      context.wrangler,
      context.cloudflareEnv,
    );
    ownership.cloudflareAbsent = true;
    await ensureCloudflareAbsent(
      name,
      configs.openComputePreflight,
      context.wrangler,
      context.openComputeEnv,
    );
    ownership.openComputeAbsent = true;
    await provisionResources(resources, configs, context);
    await writeDeploymentConfigs(context, fixture, name, resources, configs);
    ownership.cloudflareUrl = await deployCloudflare(
      context,
      projectRoot,
      configs,
      resources,
      ownership,
      name,
    );
    await deployOpenCompute(
      context,
      projectRoot,
      configs,
      resources,
      ownership,
      name,
    );
    const cloudflare = await observe(
      ownership.cloudflareUrl,
      fixture,
      "cloudflare",
    );
    const openCompute = await observe(
      ownership.openComputeUrl,
      fixture,
      "open-compute",
      { connection: "close" },
    );
    if (JSON.stringify(cloudflare) !== JSON.stringify(openCompute))
      throw new Error(`${fixture.id}: normalized observations differ`);
    result = {
      id: fixture.id,
      status: "passed",
      sourceSha256: fixture.sourceSha256,
      cloudflare,
      openCompute,
    };
  } catch (error) {
    failure = sanitizedError(error, [context.token, context.adminToken]);
    result = {
      id: fixture.id,
      status: "failed",
      sourceSha256: fixture.sourceSha256,
      error: failure,
    };
  }
  return {
    result,
    cleanup: async () => {
      const cleanup = await cleanupFixtureResources({
        fixture,
        name,
        resources,
        configs,
        ownership,
        context,
      });
      return {
        record: {
          id: fixture.id,
          cloudflare: cleanup.cloudflare,
          openCompute: cleanup.openCompute,
        },
        deleted: cleanup.deleted,
      };
    },
    ...(failure === undefined ? {} : { failure }),
  };
}
