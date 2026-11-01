import type { JsonRecord, PortableFixture } from "../adapters/types.ts";
import {
  cleanupCloudflare,
  cleanupCloudflareD1,
  cleanupCloudflareKv,
  cleanupCloudflareR2,
} from "./cloudflare-resources.ts";
import { bestEffortFixtureCleanup, recordOwnership } from "./evidence.ts";
import type { OwnedResources } from "./owned-resources.ts";
import { cleanupQueue, cleanupWorkflow } from "./product-resources.ts";
import type { DifferentialConfigs } from "./resource-provisioning.ts";
import type { DifferentialContext } from "./run-context.ts";

export interface WorkerOwnership {
  cloudflareAbsent: boolean;
  cloudflareOwned: boolean;
  openComputeAbsent: boolean;
  openComputeOwned: boolean;
  cloudflareUrl: string | undefined;
  readonly openComputeUrl: string;
}

interface CleanupInput {
  readonly fixture: PortableFixture;
  readonly name: string;
  readonly resources: OwnedResources;
  readonly configs: DifferentialConfigs;
  readonly ownership: WorkerOwnership;
  readonly context: DifferentialContext;
}

interface CleanupOutcome {
  readonly cloudflare: JsonRecord;
  readonly openCompute: JsonRecord;
  readonly deleted: boolean;
}

function notCreated(absent: boolean): JsonRecord {
  return {
    deleted: absent,
    status: absent ? "not-created" : "preflight-did-not-prove-absence",
  };
}

function allDeleted(items: readonly JsonRecord[]): boolean {
  return items.every((item) => item.deleted === true);
}

export async function cleanupFixtureResources({
  fixture,
  name,
  resources,
  configs,
  ownership,
  context,
}: CleanupInput): Promise<CleanupOutcome> {
  const { cf, cloudflareEnv, openComputeEnv, journalPath } = context;
  if (
    resources.r2Buckets.length > 0 ||
    resources.durableObjectNamespaces.length > 0 ||
    resources.workflows.length > 0
  ) {
    if (ownership.cloudflareUrl !== undefined) {
      await bestEffortFixtureCleanup(ownership.cloudflareUrl, fixture, {});
    }
    if (ownership.openComputeOwned) {
      await bestEffortFixtureCleanup(ownership.openComputeUrl, fixture, {
        connection: "close",
        host: `${name}.${context.openComputeInternalAccount}.localhost`,
      });
    }
  }

  const outcomes: Record<string, JsonRecord> = {};
  for (const target of context.local
    ? (["open-compute"] as const)
    : (["cloudflare", "open-compute"] as const)) {
    const cloudflare = target === "cloudflare";
    const absent = cloudflare ? "cloudflareAbsent" : "openComputeAbsent";
    const owned = cloudflare ? "cloudflareOwned" : "openComputeOwned";
    const id = cloudflare ? "cloudflareId" : "openComputeId";
    const config = cloudflare
      ? configs.cloudflarePreflight
      : configs.openComputePreflight;
    const env = cloudflare ? cloudflareEnv : openComputeEnv;
    const worker = (
      cloudflare ? ownership.cloudflareOwned : ownership.openComputeOwned
    )
      ? await cleanupCloudflare(
          name,
          cloudflare ? configs.cloudflare : configs.openCompute,
          cf,
          env,
        )
      : notCreated(
          cloudflare ? ownership.cloudflareAbsent : ownership.openComputeAbsent,
        );
    const bindings: JsonRecord[] = [];
    for (const namespace of [...resources.kvNamespaces].reverse()) {
      bindings.push(
        namespace[owned]
          ? await cleanupCloudflareKv(
              namespace.name,
              namespace[id],
              config,
              cf,
              env,
            )
          : notCreated(namespace[absent]),
      );
    }
    for (const database of [...resources.d1Databases].reverse()) {
      bindings.push(
        database[owned]
          ? await cleanupCloudflareD1(
              database.name,
              database[id],
              config,
              cf,
              env,
            )
          : notCreated(database[absent]),
      );
    }
    for (const bucket of [...resources.r2Buckets].reverse()) {
      bindings.push(
        bucket[owned]
          ? await cleanupCloudflareR2(bucket.name, config, cf, env)
          : notCreated(bucket[absent]),
      );
    }
    for (const queue of [...resources.queues].reverse()) {
      bindings.push(
        queue[owned]
          ? await cleanupQueue(queue.name, config, cf, env)
          : notCreated(queue[absent]),
      );
    }
    for (const namespace of resources.durableObjectNamespaces) {
      bindings.push({
        deleted:
          cloudflare || namespace.openComputeOwned
            ? worker.deleted === true
            : true,
        status:
          cloudflare || namespace.openComputeOwned
            ? worker.deleted === true
              ? "absent-with-owner-worker"
              : "owner-worker-still-present"
            : "not-created",
        binding: namespace.binding,
        className: namespace.className,
        owner: name,
      });
    }
    for (const workflow of [...resources.workflows].reverse()) {
      bindings.push(
        workflow[owned]
          ? await cleanupWorkflow(workflow.name, config, cf, env)
          : notCreated(workflow[absent]),
      );
    }
    outcomes[target] = {
      deleted: worker.deleted === true && allDeleted(bindings),
      worker,
      bindings,
    };
  }
  const cloudflare = outcomes.cloudflare ?? { status: "not-selected" };
  const openCompute = outcomes["open-compute"]!;
  await recordOwnership(journalPath, {
    target: "cleanup",
    kind: fixture.id,
    name,
    result: { cloudflare, openCompute },
  });
  return {
    cloudflare,
    openCompute,
    deleted: Object.values(outcomes).every(
      (outcome) => outcome.deleted === true,
    ),
  };
}
