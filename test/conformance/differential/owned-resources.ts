import type { PortableFixture } from "../adapters/types.ts";

export interface OwnedKvNamespace {
  readonly binding: string;
  readonly name: string;
  cloudflareAbsent: boolean;
  cloudflareOwned: boolean;
  cloudflareId?: string;
  openComputeAbsent: boolean;
  openComputeOwned: boolean;
  openComputeId?: string;
}

export interface OwnedD1Database {
  readonly binding: string;
  readonly name: string;
  cloudflareAbsent: boolean;
  cloudflareOwned: boolean;
  cloudflareId?: string;
  openComputeAbsent: boolean;
  openComputeOwned: boolean;
  openComputeId?: string;
}

export interface OwnedR2Bucket {
  readonly binding: string;
  readonly name: string;
  cloudflareAbsent: boolean;
  cloudflareOwned: boolean;
  openComputeAbsent: boolean;
  openComputeOwned: boolean;
}

export interface OwnedQueue {
  readonly binding: string;
  readonly name: string;
  cloudflareAbsent: boolean;
  cloudflareOwned: boolean;
  openComputeAbsent: boolean;
  openComputeOwned: boolean;
}

export interface OwnedDurableObjectNamespace {
  readonly binding: string;
  readonly className: string;
  openComputeOwned: boolean;
}

export interface OwnedWorkflow {
  readonly binding: string;
  readonly className: string;
  readonly name: string;
  cloudflareAbsent: boolean;
  cloudflareOwned: boolean;
  openComputeAbsent: boolean;
  openComputeOwned: boolean;
}

export interface OwnedResources {
  readonly kvNamespaces: OwnedKvNamespace[];
  readonly d1Databases: OwnedD1Database[];
  readonly r2Buckets: OwnedR2Bucket[];
  readonly queues: OwnedQueue[];
  readonly durableObjectNamespaces: OwnedDurableObjectNamespace[];
  readonly workflows: OwnedWorkflow[];
}

export function ownedResources(
  fixture: PortableFixture,
  name: string,
): OwnedResources {
  const entries = Object.entries(fixture.bindings);
  return {
    kvNamespaces: entries
      .filter(([, value]) => value.type === "kv_namespace")
      .map(([binding], index) => ({
        binding,
        name: `${name}-kv-${index}`,
        cloudflareAbsent: false,
        cloudflareOwned: false,
        openComputeAbsent: false,
        openComputeOwned: false,
      })),
    d1Databases: entries
      .filter(([, value]) => value.type === "d1_database")
      .map(([binding], index) => ({
        binding,
        name: `${name}-d1-${index}`,
        cloudflareAbsent: false,
        cloudflareOwned: false,
        openComputeAbsent: false,
        openComputeOwned: false,
      })),
    r2Buckets: entries
      .filter(([, value]) => value.type === "r2_bucket")
      .map(([binding], index) => ({
        binding,
        name: `${name}-r2-${index}`,
        cloudflareAbsent: false,
        cloudflareOwned: false,
        openComputeAbsent: false,
        openComputeOwned: false,
      })),
    queues: entries
      .filter(([, value]) => value.type === "queue_producer")
      .map(([binding], index) => ({
        binding,
        name: `${name}-queue-${index}`,
        cloudflareAbsent: false,
        cloudflareOwned: false,
        openComputeAbsent: false,
        openComputeOwned: false,
      })),
    durableObjectNamespaces: entries.flatMap(([binding, value]) =>
      value.type === "do_namespace"
        ? [{ binding, className: value.className, openComputeOwned: false }]
        : [],
    ),
    workflows: entries.flatMap(([binding, value], index) =>
      value.type === "workflow"
        ? [
            {
              binding,
              className: value.className,
              name: `${name}-workflow-${index}`,
              cloudflareAbsent: false,
              cloudflareOwned: false,
              openComputeAbsent: false,
              openComputeOwned: false,
            },
          ]
        : [],
    ),
  };
}

export function bindingIds(
  resources: OwnedResources,
  target: "cloudflare" | "open-compute",
): Record<string, string> {
  return Object.fromEntries([
    ...resources.kvNamespaces.map((item) => [
      item.binding,
      target === "cloudflare" ? item.cloudflareId! : item.openComputeId!,
    ]),
    ...resources.d1Databases.map((item) => [
      item.binding,
      target === "cloudflare" ? item.cloudflareId! : item.openComputeId!,
    ]),
  ]);
}

export function bindingNames(
  resources: OwnedResources,
): Record<string, string> {
  return Object.fromEntries([
    ...resources.d1Databases.map((item) => [item.binding, item.name]),
    ...resources.r2Buckets.map((item) => [item.binding, item.name]),
    ...resources.queues.map((item) => [item.binding, item.name]),
    ...resources.workflows.map((item) => [item.binding, item.name]),
  ]);
}

export function resourceCounts(fixtures: readonly PortableFixture[]): {
  readonly kv: number;
  readonly d1: number;
  readonly r2: number;
  readonly queues: number;
  readonly durableObjects: number;
  readonly workflows: number;
} {
  const count = (type: string): number =>
    fixtures.reduce(
      (total, fixture) =>
        total +
        Object.values(fixture.bindings).filter(
          (binding) => binding.type === type,
        ).length,
      0,
    );
  return {
    kv: count("kv_namespace"),
    d1: count("d1_database"),
    r2: count("r2_bucket"),
    queues: count("queue_producer"),
    durableObjects: count("do_namespace"),
    workflows: count("workflow"),
  };
}
