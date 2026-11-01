import { cp, mkdir, symlink, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { command } from "../adapters/command.ts";
import { observe } from "../adapters/observations.ts";
import {
  cloudflareBaseProject,
  cloudflareProject,
  openComputeBaseProject,
  openComputeProject,
} from "../adapters/projects.ts";
import { CF_PROJECT_MANIFEST } from "../adapters/runtime-contract.ts";
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
      "cloudflare",
      "cloudflare.config.ts",
    ),
    cloudflare: join(projectRoot, "cloudflare", "cloudflare.config.ts"),
    openComputePreflight: join(
      projectRoot,
      "open-compute",
      "cloudflare.config.ts",
    ),
    openCompute: join(projectRoot, "open-compute", "cloudflare.config.ts"),
  };
  for (const target of context.local
    ? ["open-compute"]
    : ["cloudflare", "open-compute"]) {
    const directory = join(projectRoot, target);
    await mkdir(directory);
    await writeFile(
      join(directory, "package.json"),
      `${JSON.stringify(CF_PROJECT_MANIFEST)}\n`,
    );
    await symlink(
      join(context.root, "node_modules"),
      join(directory, "node_modules"),
    );
    await writeFile(
      join(directory, "vite.config.ts"),
      "import {cloudflare} from '@cloudflare/vite-plugin'; export default {plugins:[cloudflare({types:{generate:false}})]};\n",
    );
  }
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
  if (!context.local) {
    await writeFile(
      configs.cloudflarePreflight,
      `export default ${JSON.stringify(
        cloudflareBaseProject(
          {
            ...fixture,
            source: join(projectRoot, "src/index.ts"),
            root: dirname(configs.cloudflare),
          },
          name,
          context.accountId,
        ),
        null,
        2,
      )}\n`,
      { mode: 0o600 },
    );
  }
  await writeFile(
    configs.openComputePreflight,
    `export default ${JSON.stringify(
      openComputeBaseProject(
        {
          ...fixture,
          source: join(projectRoot, "src/index.ts"),
          root: dirname(configs.openCompute),
        },
        name,
        context.openComputeAccount,
      ),
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
  if (!context.local) {
    await writeFile(
      configs.cloudflare,
      `export default ${JSON.stringify(
        cloudflareProject(
          {
            ...fixture,
            source: join(dirname(configs.cloudflare), "../src/index.ts"),
            root: dirname(configs.cloudflare),
          },
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
  }
  await writeFile(
    configs.openCompute,
    `export default ${JSON.stringify(
      openComputeProject(
        {
          ...fixture,
          source: join(dirname(configs.openCompute), "../src/index.ts"),
          root: dirname(configs.openCompute),
        },
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
  configs: DifferentialConfigs,
  resources: OwnedResources,
  ownership: WorkerOwnership,
  name: string,
): Promise<string> {
  ownership.cloudflareOwned = true;
  for (const workflow of resources.workflows) workflow.cloudflareOwned = true;
  const deployed = await command(
    context.cf,
    ["deploy", "--mode", "production"],
    {
      cwd: dirname(configs.cloudflare),
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
      context.cf,
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
  configs: DifferentialConfigs,
  resources: OwnedResources,
  ownership: WorkerOwnership,
  name: string,
): Promise<void> {
  ownership.openComputeOwned = true;
  for (const namespace of resources.durableObjectNamespaces)
    namespace.openComputeOwned = true;
  for (const workflow of resources.workflows) workflow.openComputeOwned = true;
  await command(context.cf, ["deploy", "--mode", "production"], {
    cwd: dirname(configs.openCompute),
    env: context.openComputeEnv,
    timeout: 300_000,
  });
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
      context.cf,
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
  const { configs } = await prepareFixtureFiles(context, fixture, index, name);
  const resources = ownedResources(fixture, name);
  const ownership: WorkerOwnership = {
    cloudflareAbsent: false,
    cloudflareOwned: false,
    openComputeAbsent: false,
    openComputeOwned: false,
    cloudflareUrl: undefined,
    openComputeUrl: new URL("/", context.publicEndpoint).href,
  };
  let result: JsonRecord;
  let failure: string | undefined;
  try {
    if (!context.local) {
      await ensureCloudflareAbsent(
        name,
        configs.cloudflarePreflight,
        context.cf,
        context.cloudflareEnv,
      );
      ownership.cloudflareAbsent = true;
    }
    await ensureCloudflareAbsent(
      name,
      configs.openComputePreflight,
      context.cf,
      context.openComputeEnv,
    );
    ownership.openComputeAbsent = true;
    await provisionResources(resources, configs, context);
    await writeDeploymentConfigs(context, fixture, name, resources, configs);
    if (!context.local) {
      ownership.cloudflareUrl = await deployCloudflare(
        context,
        configs,
        resources,
        ownership,
        name,
      );
    }
    await deployOpenCompute(context, configs, resources, ownership, name);
    const cloudflare = context.local
      ? undefined
      : await observe(
          ownership.cloudflareUrl!,
          fixture,
          "cloudflare",
          {},
          !context.refresh,
        );
    const expected =
      context.refresh && cloudflare
        ? {
            ...fixture,
            observations: fixture.observations.map((observation, index) => ({
              ...observation,
              expect: cloudflare[index] as typeof observation.expect,
            })),
          }
        : fixture;
    const openCompute = await observe(
      ownership.openComputeUrl,
      expected,
      "open-compute",
      {
        connection: "close",
        host: `${name}.${context.openComputeInternalAccount}.localhost`,
      },
    );
    if (
      cloudflare &&
      JSON.stringify(cloudflare) !== JSON.stringify(openCompute)
    )
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
