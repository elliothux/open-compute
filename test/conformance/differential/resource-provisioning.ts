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
  const { cf, journalPath } = context;
  for (const target of context.local
    ? (["open-compute"] as const)
    : (["cloudflare", "open-compute"] as const)) {
    const cloudflare = target === "cloudflare";
    const config = cloudflare
      ? configs.cloudflarePreflight
      : configs.openComputePreflight;
    const env = cloudflare ? context.cloudflareEnv : context.openComputeEnv;
    const absent = cloudflare ? "cloudflareAbsent" : "openComputeAbsent";
    const owned = cloudflare ? "cloudflareOwned" : "openComputeOwned";
    const id = cloudflare ? "cloudflareId" : "openComputeId";
    for (const workflow of resources.workflows) {
      await ensureWorkflowAbsent(workflow.name, config, cf, env);
      workflow[absent] = true;
    }
    for (const namespace of resources.kvNamespaces) {
      await ensureCloudflareKvAbsent(namespace.name, config, cf, env);
      namespace[absent] = true;
      namespace[owned] = true;
      namespace[id] = await createCloudflareKv(namespace.name, config, cf, env);
      await recordOwnership(journalPath, {
        target,
        kind: "kv_namespace",
        name: namespace.name,
        binding: namespace.binding,
        id: namespace[id],
      });
    }
    for (const database of resources.d1Databases) {
      await ensureCloudflareD1Absent(database.name, config, cf, env);
      database[absent] = true;
      database[owned] = true;
      database[id] = await createCloudflareD1(database.name, config, cf, env);
      await recordOwnership(journalPath, {
        target,
        kind: "d1_database",
        name: database.name,
        binding: database.binding,
        id: database[id],
      });
    }
    for (const bucket of resources.r2Buckets) {
      await ensureCloudflareR2Absent(bucket.name, config, cf, env);
      bucket[absent] = true;
      bucket[owned] = true;
      await createCloudflareR2(bucket.name, config, cf, env);
      await recordOwnership(journalPath, {
        target,
        kind: "r2_bucket",
        name: bucket.name,
        binding: bucket.binding,
      });
    }
    for (const queue of resources.queues) {
      await ensureQueueAbsent(queue.name, config, cf, env);
      queue[absent] = true;
      queue[owned] = true;
      await createQueue(queue.name, config, cf, env);
      await recordOwnership(journalPath, {
        target,
        kind: "queue_producer",
        name: queue.name,
        binding: queue.binding,
      });
    }
  }
}
