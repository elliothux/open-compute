import {
  createCloudflareD1,
  createCloudflareKv,
  createCloudflareR2,
  ensureCloudflareD1Absent,
  ensureCloudflareKvAbsent,
  ensureCloudflareR2Absent,
} from "./cloudflare-resources.ts";
import { recordOwnership } from "./evidence.ts";
import type { OwnedResources } from "./owned-resources.ts";
import {
  createQueue,
  ensureQueueAbsent,
  ensureWorkflowAbsent,
} from "./product-resources.ts";
import type { DifferentialContext } from "./run-context.ts";

export interface DifferentialConfigs {
  readonly cloudflarePreflight: string;
  readonly cloudflare: string;
  readonly openComputePreflight: string;
  readonly openCompute: string;
}

export async function provisionResources(
  resources: OwnedResources,
  configs: DifferentialConfigs,
  context: DifferentialContext,
): Promise<void> {
  const { wrangler, cloudflareEnv, openComputeEnv, journalPath } = context;
  for (const workflow of resources.workflows) {
    await ensureWorkflowAbsent(
      workflow.name,
      configs.cloudflarePreflight,
      wrangler,
      cloudflareEnv,
    );
    workflow.cloudflareAbsent = true;
    await ensureWorkflowAbsent(
      workflow.name,
      configs.openComputePreflight,
      wrangler,
      openComputeEnv,
    );
    workflow.openComputeAbsent = true;
  }
  for (const namespace of resources.kvNamespaces) {
    await ensureCloudflareKvAbsent(
      namespace.name,
      configs.cloudflarePreflight,
      wrangler,
      cloudflareEnv,
    );
    namespace.cloudflareAbsent = true;
    await ensureCloudflareKvAbsent(
      namespace.name,
      configs.openComputePreflight,
      wrangler,
      openComputeEnv,
    );
    namespace.openComputeAbsent = true;
    namespace.cloudflareOwned = true;
    namespace.cloudflareId = await createCloudflareKv(
      namespace.name,
      configs.cloudflarePreflight,
      wrangler,
      cloudflareEnv,
    );
    await recordOwnership(journalPath, {
      target: "cloudflare",
      kind: "kv_namespace",
      name: namespace.name,
      binding: namespace.binding,
      id: namespace.cloudflareId,
    });
    namespace.openComputeOwned = true;
    namespace.openComputeId = await createCloudflareKv(
      namespace.name,
      configs.openComputePreflight,
      wrangler,
      openComputeEnv,
    );
    await recordOwnership(journalPath, {
      target: "open-compute",
      kind: "kv_namespace",
      name: namespace.name,
      binding: namespace.binding,
      id: namespace.openComputeId,
    });
  }
  for (const database of resources.d1Databases) {
    await ensureCloudflareD1Absent(
      database.name,
      configs.cloudflarePreflight,
      wrangler,
      cloudflareEnv,
    );
    database.cloudflareAbsent = true;
    await ensureCloudflareD1Absent(
      database.name,
      configs.openComputePreflight,
      wrangler,
      openComputeEnv,
    );
    database.openComputeAbsent = true;
    database.cloudflareOwned = true;
    database.cloudflareId = await createCloudflareD1(
      database.name,
      configs.cloudflarePreflight,
      wrangler,
      cloudflareEnv,
    );
    await recordOwnership(journalPath, {
      target: "cloudflare",
      kind: "d1_database",
      name: database.name,
      binding: database.binding,
      id: database.cloudflareId,
    });
    database.openComputeOwned = true;
    database.openComputeId = await createCloudflareD1(
      database.name,
      configs.openComputePreflight,
      wrangler,
      openComputeEnv,
    );
    await recordOwnership(journalPath, {
      target: "open-compute",
      kind: "d1_database",
      name: database.name,
      binding: database.binding,
      id: database.openComputeId,
    });
  }
  for (const bucket of resources.r2Buckets) {
    await ensureCloudflareR2Absent(
      bucket.name,
      configs.cloudflarePreflight,
      wrangler,
      cloudflareEnv,
    );
    bucket.cloudflareAbsent = true;
    await ensureCloudflareR2Absent(
      bucket.name,
      configs.openComputePreflight,
      wrangler,
      openComputeEnv,
    );
    bucket.openComputeAbsent = true;
    bucket.cloudflareOwned = true;
    await createCloudflareR2(
      bucket.name,
      configs.cloudflarePreflight,
      wrangler,
      cloudflareEnv,
    );
    await recordOwnership(journalPath, {
      target: "cloudflare",
      kind: "r2_bucket",
      name: bucket.name,
      binding: bucket.binding,
    });
    bucket.openComputeOwned = true;
    await createCloudflareR2(
      bucket.name,
      configs.openComputePreflight,
      wrangler,
      openComputeEnv,
    );
    await recordOwnership(journalPath, {
      target: "open-compute",
      kind: "r2_bucket",
      name: bucket.name,
      binding: bucket.binding,
    });
  }
  for (const queue of resources.queues) {
    await ensureQueueAbsent(
      queue.name,
      configs.cloudflarePreflight,
      wrangler,
      cloudflareEnv,
    );
    queue.cloudflareAbsent = true;
    await ensureQueueAbsent(
      queue.name,
      configs.openComputePreflight,
      wrangler,
      openComputeEnv,
    );
    queue.openComputeAbsent = true;
    queue.cloudflareOwned = true;
    await createQueue(
      queue.name,
      configs.cloudflarePreflight,
      wrangler,
      cloudflareEnv,
    );
    await recordOwnership(journalPath, {
      target: "cloudflare",
      kind: "queue_producer",
      name: queue.name,
      binding: queue.binding,
    });
    queue.openComputeOwned = true;
    await createQueue(
      queue.name,
      configs.openComputePreflight,
      wrangler,
      openComputeEnv,
    );
    await recordOwnership(journalPath, {
      target: "open-compute",
      kind: "queue_producer",
      name: queue.name,
      binding: queue.binding,
    });
  }
}
