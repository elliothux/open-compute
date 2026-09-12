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
  const { wrangler, cloudflareEnv, openComputeEnv, journalPath } = context;
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
      });
    }
  }

  const cfWorker = ownership.cloudflareOwned
    ? await cleanupCloudflare(name, configs.cloudflare, wrangler, cloudflareEnv)
    : notCreated(ownership.cloudflareAbsent);
  const cfBindings: JsonRecord[] = [];
  for (const namespace of [...resources.kvNamespaces].reverse()) {
    cfBindings.push(
      namespace.cloudflareOwned
        ? await cleanupCloudflareKv(
            namespace.name,
            namespace.cloudflareId,
            configs.cloudflarePreflight,
            wrangler,
            cloudflareEnv,
          )
        : notCreated(namespace.cloudflareAbsent),
    );
  }
  const cfD1Bindings: JsonRecord[] = [];
  for (const database of [...resources.d1Databases].reverse()) {
    cfD1Bindings.push(
      database.cloudflareOwned
        ? await cleanupCloudflareD1(
            database.name,
            database.cloudflareId,
            configs.cloudflarePreflight,
            wrangler,
            cloudflareEnv,
          )
        : notCreated(database.cloudflareAbsent),
    );
  }
  const cfR2Bindings: JsonRecord[] = [];
  for (const bucket of [...resources.r2Buckets].reverse()) {
    cfR2Bindings.push(
      bucket.cloudflareOwned
        ? await cleanupCloudflareR2(
            bucket.name,
            configs.cloudflarePreflight,
            wrangler,
            cloudflareEnv,
          )
        : notCreated(bucket.cloudflareAbsent),
    );
  }
  const cfQueueBindings: JsonRecord[] = [];
  for (const queue of [...resources.queues].reverse()) {
    cfQueueBindings.push(
      queue.cloudflareOwned
        ? await cleanupQueue(
            queue.name,
            configs.cloudflarePreflight,
            wrangler,
            cloudflareEnv,
          )
        : notCreated(queue.cloudflareAbsent),
    );
  }
  const cfDoBindings: JsonRecord[] = resources.durableObjectNamespaces.map(
    (namespace) => ({
      deleted: cfWorker.deleted === true,
      status:
        cfWorker.deleted === true
          ? "absent-with-owner-worker"
          : "owner-worker-still-present",
      binding: namespace.binding,
      className: namespace.className,
      owner: name,
    }),
  );
  const cfWorkflowBindings: JsonRecord[] = [];
  for (const workflow of [...resources.workflows].reverse()) {
    cfWorkflowBindings.push(
      workflow.cloudflareOwned
        ? await cleanupWorkflow(
            workflow.name,
            configs.cloudflarePreflight,
            wrangler,
            cloudflareEnv,
          )
        : notCreated(workflow.cloudflareAbsent),
    );
  }

  const ocWorker = ownership.openComputeOwned
    ? await cleanupCloudflare(
        name,
        configs.openCompute,
        wrangler,
        openComputeEnv,
      )
    : notCreated(ownership.openComputeAbsent);
  const ocBindings: JsonRecord[] = [];
  for (const namespace of [...resources.kvNamespaces].reverse()) {
    ocBindings.push(
      namespace.openComputeOwned
        ? await cleanupCloudflareKv(
            namespace.name,
            namespace.openComputeId,
            configs.openComputePreflight,
            wrangler,
            openComputeEnv,
          )
        : notCreated(namespace.openComputeAbsent),
    );
  }
  const ocD1Bindings: JsonRecord[] = [];
  for (const database of [...resources.d1Databases].reverse()) {
    ocD1Bindings.push(
      database.openComputeOwned
        ? await cleanupCloudflareD1(
            database.name,
            database.openComputeId,
            configs.openComputePreflight,
            wrangler,
            openComputeEnv,
          )
        : notCreated(database.openComputeAbsent),
    );
  }
  const ocR2Bindings: JsonRecord[] = [];
  for (const bucket of [...resources.r2Buckets].reverse()) {
    ocR2Bindings.push(
      bucket.openComputeOwned
        ? await cleanupCloudflareR2(
            bucket.name,
            configs.openComputePreflight,
            wrangler,
            openComputeEnv,
          )
        : notCreated(bucket.openComputeAbsent),
    );
  }
  const ocQueueBindings: JsonRecord[] = [];
  for (const queue of [...resources.queues].reverse()) {
    ocQueueBindings.push(
      queue.openComputeOwned
        ? await cleanupQueue(
            queue.name,
            configs.openComputePreflight,
            wrangler,
            openComputeEnv,
          )
        : notCreated(queue.openComputeAbsent),
    );
  }
  const ocDoBindings: JsonRecord[] = resources.durableObjectNamespaces.map(
    (namespace) => ({
      deleted: namespace.openComputeOwned ? ocWorker.deleted === true : true,
      status: namespace.openComputeOwned
        ? ocWorker.deleted === true
          ? "absent-with-owner-worker"
          : "owner-worker-still-present"
        : "not-created",
      binding: namespace.binding,
      className: namespace.className,
      owner: name,
    }),
  );
  const ocWorkflowBindings: JsonRecord[] = [];
  for (const workflow of [...resources.workflows].reverse()) {
    ocWorkflowBindings.push(
      workflow.openComputeOwned
        ? await cleanupWorkflow(
            workflow.name,
            configs.openComputePreflight,
            wrangler,
            openComputeEnv,
          )
        : notCreated(workflow.openComputeAbsent),
    );
  }

  const cfDeleted =
    cfWorker.deleted === true &&
    allDeleted(cfBindings) &&
    allDeleted(cfD1Bindings) &&
    allDeleted(cfR2Bindings) &&
    allDeleted(cfQueueBindings) &&
    allDeleted(cfDoBindings) &&
    allDeleted(cfWorkflowBindings);
  const ocDeleted =
    ocWorker.deleted === true &&
    allDeleted(ocBindings) &&
    allDeleted(ocD1Bindings) &&
    allDeleted(ocR2Bindings) &&
    allDeleted(ocQueueBindings) &&
    allDeleted(ocDoBindings) &&
    allDeleted(ocWorkflowBindings);
  const cloudflare: JsonRecord = {
    deleted: cfDeleted,
    worker: cfWorker,
    bindings: [
      ...cfBindings,
      ...cfD1Bindings,
      ...cfR2Bindings,
      ...cfQueueBindings,
      ...cfDoBindings,
      ...cfWorkflowBindings,
    ],
  };
  const openCompute: JsonRecord = {
    deleted: ocDeleted,
    worker: ocWorker,
    bindings: [
      ...ocBindings,
      ...ocD1Bindings,
      ...ocR2Bindings,
      ...ocQueueBindings,
      ...ocDoBindings,
      ...ocWorkflowBindings,
    ],
  };
  await recordOwnership(journalPath, {
    target: "cleanup",
    kind: fixture.id,
    name,
    result: { cloudflare, openCompute },
  });
  return {
    cloudflare,
    openCompute,
    deleted: cfDeleted && ocDeleted,
  };
}
